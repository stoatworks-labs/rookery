//! What rookery can ask an instance to do, and how that becomes OSC.
//!
//! This is deliberately **exactly** WebLinked's OSC verb set and no more.
//! WebLinked's HTTP API is much wider — it can add and remove outputs,
//! reconcile whole source configurations, change per-output backgrounds — but
//! none of that is reachable over OSC, and inventing a `Command` variant that
//! has to fall back to HTTP would hide a real operational difference: OSC
//! commands are fire-and-forget and arrive in milliseconds, HTTP calls block
//! and can fail with a reason. A fleet tool that quietly mixes the two gives
//! an operator no way to reason about what happens when the network is bad.
//!
//! So: everything here goes out over OSC. Anything WebLinked only exposes
//! over HTTP stays out of this enum, and rookery's UI reaches it per-instance
//! through the HTTP client instead, where the failure is visible.

use serde::{Deserialize, Serialize};

use rookery_osc::{Arg, Message};

/// A command aimed at one source inside one instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "lowercase")]
pub enum Command {
    /// Navigate the page.
    Url { url: String },
    /// Reload. `ignore_cache` bypasses the HTTP cache — what you want after a
    /// designer has re-uploaded a graphic to the same address.
    Reload {
        #[serde(default)]
        ignore_cache: bool,
    },
    /// Run JavaScript in the page. The most useful verb in the set: a graphic
    /// that already defines its own functions needs no integration work.
    Script { script: String },
    /// Mute at the source.
    Mute { muted: bool },
    /// Change the raster/rate. Restarts every output on that source.
    Format { format: String },
    /// Start or stop one named output.
    Output { name: String, enabled: bool },
}

impl Command {
    /// A short label for logs and the UI activity feed.
    pub fn summary(&self) -> String {
        match self {
            Command::Url { url } => format!("url {url}"),
            Command::Reload { ignore_cache: true } => "reload (bypass cache)".to_string(),
            Command::Reload { .. } => "reload".to_string(),
            Command::Script { script } => {
                let trimmed: String = script.chars().take(60).collect();
                if script.chars().count() > 60 {
                    format!("script {trimmed}…")
                } else {
                    format!("script {trimmed}")
                }
            }
            Command::Mute { muted: true } => "mute".to_string(),
            Command::Mute { muted: false } => "unmute".to_string(),
            Command::Format { format } => format!("format {format}"),
            Command::Output { name, enabled } => {
                format!("output {name} {}", if *enabled { "on" } else { "off" })
            }
        }
    }

    /// True for commands that interrupt the picture on air.
    ///
    /// The UI uses this to require a second click when the target is a group
    /// rather than one instance: retargeting eight machines at once is a
    /// normal thing to want and a catastrophic thing to do by accident.
    pub fn is_disruptive(&self) -> bool {
        matches!(
            self,
            Command::Format { .. } | Command::Output { .. } | Command::Url { .. }
        )
    }

    /// The OSC verb path, without the prefix or any source selector.
    fn verb_path(&self) -> String {
        match self {
            Command::Url { .. } => "url".to_string(),
            Command::Reload { .. } => "reload".to_string(),
            Command::Script { .. } => "script".to_string(),
            Command::Mute { .. } => "mute".to_string(),
            Command::Format { .. } => "format".to_string(),
            // The output name lives in the address, not an argument — that is
            // the shape WebLinked parses, and it is what makes one Companion
            // button bind to one output and stay bound.
            Command::Output { name, .. } => format!("output/{name}"),
        }
    }

    fn args(&self) -> Vec<Arg> {
        match self {
            Command::Url { url } => vec![Arg::Str(url.clone())],
            // Sent explicitly rather than as a bare trigger. WebLinked treats
            // a no-argument /reload as "don't bypass the cache", so `i 0` is
            // equivalent — but an explicit argument is what shows up in an
            // OSC monitor when someone is working out why a button did the
            // wrong thing.
            Command::Reload { ignore_cache } => vec![Arg::Bool(*ignore_cache)],
            Command::Script { script } => vec![Arg::Str(script.clone())],
            Command::Mute { muted } => vec![Arg::Bool(*muted)],
            Command::Format { format } => vec![Arg::Str(format.clone())],
            Command::Output { enabled, .. } => vec![Arg::Bool(*enabled)],
        }
    }

    /// Parses a verb path and its arguments back into a `Command` — the
    /// northbound direction, where a desk sends rookery a cue.
    ///
    /// `verb` is what is left after the scope and any source selector have
    /// been peeled off, e.g. `url` or `output/Graphic`.
    ///
    /// Argument handling matches WebLinked's own: the first string for the
    /// string verbs, the first numeric argument as a bool for the flags, and
    /// a bare address with no arguments treated as a trigger rather than an
    /// explicit zero. A desk that can only send a bare address still gets a
    /// working reload button.
    pub fn from_osc(verb: &str, args: &[Arg]) -> anyhow::Result<Self> {
        let first_string = || {
            args.iter()
                .find_map(|a| match a {
                    Arg::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        };
        let first_bool = |fallback: bool| match args.first() {
            Some(arg) => arg.as_bool(),
            None => fallback,
        };

        if let Some(name) = verb.strip_prefix("output/") {
            anyhow::ensure!(!name.is_empty(), "output/ names no output");
            return Ok(Command::Output {
                name: name.to_string(),
                // Matches WebLinked: a bare `/output/<name>` means "on".
                enabled: first_bool(true),
            });
        }

        match verb {
            "url" => {
                let url = first_string();
                anyhow::ensure!(!url.is_empty(), "url takes a string argument");
                Ok(Command::Url { url })
            }
            "reload" => Ok(Command::Reload {
                ignore_cache: first_bool(false),
            }),
            "script" => {
                let script = first_string();
                anyhow::ensure!(!script.is_empty(), "script takes a string argument");
                Ok(Command::Script { script })
            }
            "mute" => Ok(Command::Mute {
                muted: first_bool(true),
            }),
            "format" => {
                let format = first_string();
                anyhow::ensure!(!format.is_empty(), "format takes a string argument");
                Ok(Command::Format { format })
            }
            other => anyhow::bail!("unknown verb {other:?}"),
        }
    }

    /// Builds the OSC message for this command.
    ///
    /// `source` names one of the instance's pipelines; `None` means the
    /// primary, which is the only one there is for any command-line launch
    /// and is what keeps a single-source instance addressable without knowing
    /// its id.
    pub fn to_osc(&self, prefix: &str, source: Option<&str>) -> Message {
        let prefix = prefix.trim_end_matches('/');
        let address = match source {
            Some(id) => format!("{prefix}/source/{id}/{}", self.verb_path()),
            None => format!("{prefix}/{}", self.verb_path()),
        };
        Message {
            address,
            args: self.args(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_match_weblinkeds_documented_shape() {
        let url = Command::Url {
            url: "https://example.com".to_string(),
        };
        assert_eq!(url.to_osc("/weblinked", None).address, "/weblinked/url");
        assert_eq!(
            url.to_osc("/weblinked", Some("lower-third")).address,
            "/weblinked/source/lower-third/url"
        );

        let output = Command::Output {
            name: "Graphic".to_string(),
            enabled: false,
        };
        assert_eq!(
            output.to_osc("/weblinked", None).address,
            "/weblinked/output/Graphic"
        );
        assert_eq!(
            output.to_osc("/weblinked", Some("clock")).address,
            "/weblinked/source/clock/output/Graphic"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_prefix_does_not_double_up() {
        let cmd = Command::Reload {
            ignore_cache: false,
        };
        assert_eq!(cmd.to_osc("/weblinked/", None).address, "/weblinked/reload");
    }

    #[test]
    fn bools_go_out_as_integers_which_is_what_first_bool_reads() {
        let msg = Command::Mute { muted: true }.to_osc("/weblinked", None);
        assert_eq!(msg.args, vec![Arg::Bool(true)]);
        assert_eq!(msg.args[0].type_tag(), b'i');
        // And the encoded form really does carry a 4-byte integer 1.
        let bytes = msg.encode();
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 1]);
    }

    /// A URL of the wrong length used to be silently dropped by WebLinked's
    /// decoder. Encoding is our side of that; check every residue produces a
    /// well-formed packet rather than trusting the codec's own test alone.
    #[test]
    fn urls_of_every_length_encode_to_aligned_packets() {
        for extra in 0..8 {
            let cmd = Command::Url {
                url: format!("https://example.com/{}", "a".repeat(extra)),
            };
            let bytes = cmd.to_osc("/weblinked", None).encode();
            assert_eq!(bytes.len() % 4, 0, "unaligned packet for extra={extra}");
        }
    }

    #[test]
    fn northbound_parsing_round_trips_every_verb() {
        for command in [
            Command::Url {
                url: "https://example.com".into(),
            },
            Command::Reload { ignore_cache: true },
            Command::Reload {
                ignore_cache: false,
            },
            Command::Script {
                script: "show()".into(),
            },
            Command::Mute { muted: true },
            Command::Mute { muted: false },
            Command::Format {
                format: "1080p50".into(),
            },
            Command::Output {
                name: "Graphic".into(),
                enabled: false,
            },
        ] {
            let msg = command.to_osc("/weblinked", None);
            let verb = msg.address.strip_prefix("/weblinked/").unwrap();
            assert_eq!(
                Command::from_osc(verb, &msg.args).unwrap(),
                command,
                "round trip failed for {}",
                command.summary()
            );
        }
    }

    #[test]
    fn a_bare_address_is_a_trigger_with_weblinkeds_own_defaults() {
        // No arguments at all — the most a minimal desk can send.
        assert_eq!(
            Command::from_osc("reload", &[]).unwrap(),
            Command::Reload {
                ignore_cache: false
            }
        );
        assert_eq!(
            Command::from_osc("mute", &[]).unwrap(),
            Command::Mute { muted: true }
        );
        assert_eq!(
            Command::from_osc("output/Graphic", &[]).unwrap(),
            Command::Output {
                name: "Graphic".into(),
                enabled: true
            }
        );
    }

    #[test]
    fn a_verb_that_needs_a_string_is_refused_without_one() {
        // Better a logged rejection than navigating a whole group to "".
        assert!(Command::from_osc("url", &[]).is_err());
        assert!(Command::from_osc("script", &[]).is_err());
        assert!(Command::from_osc("format", &[]).is_err());
        assert!(Command::from_osc("output/", &[]).is_err());
        assert!(Command::from_osc("nonsense", &[]).is_err());
    }

    #[test]
    fn group_disruptive_set_covers_what_changes_the_picture() {
        assert!(Command::Url { url: "x".into() }.is_disruptive());
        assert!(Command::Format {
            format: "1080p50".into()
        }
        .is_disruptive());
        assert!(!Command::Reload {
            ignore_cache: false
        }
        .is_disruptive());
        assert!(!Command::Mute { muted: true }.is_disruptive());
    }
}
