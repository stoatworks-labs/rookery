//! The northbound address grammar: what a desk, a cue stack or a Companion
//! button sends to rookery to move a whole group at once.
//!
//! ```text
//! /rookery/all/<verb>
//! /rookery/group/<tag>/<verb>
//! /rookery/instance/<name-or-id>/<verb>
//!
//! …each of which may name a pipeline before the verb:
//! /rookery/group/<tag>/source/<source-id>/<verb>
//! ```
//!
//! The verbs are WebLinked's own, unchanged: `url`, `reload`, `script`,
//! `mute`, `format`, `output/<name>`. That is deliberate — someone who has
//! already wired buttons straight at a WebLinked can retarget them at rookery
//! by editing the front of the address and nothing else.
//!
//! **The scope lives in the address, not in an argument.** Same reason
//! WebLinked puts its source id there: a desk sends a fixed address per
//! button, so one button binds to one group and *stays* bound, which is how
//! an operator expects a physical button to behave. A scope passed as an
//! argument would mean the same button could point anywhere depending on
//! state nobody can see from the front panel.

use rookery_core::{Command, Registry};

use crate::target::{Scope, Target};

pub const DEFAULT_NORTHBOUND_PREFIX: &str = "/rookery";

/// A parsed northbound message: who it is for and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct NorthboundCue {
    pub target: Target,
    pub command: Command,
}

/// Parses one northbound OSC address into a cue.
///
/// `registry` is needed only to turn an instance *name* into an id — a desk
/// operator should not have to type a UUID into a cue list. Resolution is
/// done here, at parse time, so an address naming something that does not
/// exist fails loudly rather than fanning out to nothing.
pub fn parse_northbound(
    prefix: &str,
    address: &str,
    args: &[rookery_osc::Arg],
    registry: &Registry,
) -> anyhow::Result<NorthboundCue> {
    let prefix = prefix.trim_end_matches('/');
    let rest = address
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow::anyhow!("address {address:?} is not under {prefix:?}"))?
        .trim_start_matches('/');

    let (scope, rest) = if let Some(tail) = rest.strip_prefix("group/") {
        let (tag, tail) = split_once_segment(tail)
            .ok_or_else(|| anyhow::anyhow!("{address:?} names a group but no verb"))?;
        (Scope::Group { tag }, tail)
    } else if let Some(tail) = rest.strip_prefix("instance/") {
        let (selector, tail) = split_once_segment(tail)
            .ok_or_else(|| anyhow::anyhow!("{address:?} names an instance but no verb"))?;
        let id = resolve_instance(&selector, registry)?;
        (Scope::Instance { id }, tail)
    } else if let Some(tail) = rest.strip_prefix("all/") {
        (Scope::All, tail.to_string())
    } else {
        anyhow::bail!(
            "{address:?} must start {prefix}/all/, {prefix}/group/<tag>/ or \
             {prefix}/instance/<name>/"
        );
    };

    // An optional pipeline selector, mirroring WebLinked's own
    // `source/<id>/<verb>` so the two grammars read the same way.
    let (source, verb) = match rest.strip_prefix("source/") {
        Some(tail) => {
            let (id, verb) = split_once_segment(tail)
                .ok_or_else(|| anyhow::anyhow!("{address:?} names a source but no verb"))?;
            anyhow::ensure!(!id.is_empty(), "{address:?} has an empty source id");
            (Some(id), verb)
        }
        None => (None, rest),
    };

    anyhow::ensure!(!verb.is_empty(), "{address:?} names no verb");
    let command =
        Command::from_osc(&verb, args).map_err(|e| anyhow::anyhow!("{address:?}: {e}"))?;

    Ok(NorthboundCue {
        target: Target { scope, source },
        command,
    })
}

/// Splits `a/b/c` into `("a", "b/c")`. `None` when there is no separator,
/// because every caller here needs something *after* the segment.
fn split_once_segment(text: &str) -> Option<(String, String)> {
    let (head, tail) = text.split_once('/')?;
    if head.is_empty() {
        return None;
    }
    Some((head.to_string(), tail.to_string()))
}

/// Resolves an instance selector, which may be a UUID or a name.
///
/// Names are what a cue list should hold — nobody wants a UUID in a QLab cue
/// — but they are not unique by construction, so an ambiguous name is an
/// error rather than a guess. Firing a graphic change at the wrong machine
/// because two of them are called "gfx" is precisely the failure a fleet tool
/// must not invent.
fn resolve_instance(
    selector: &str,
    registry: &Registry,
) -> anyhow::Result<rookery_core::InstanceId> {
    if let Ok(id) = selector.parse::<rookery_core::InstanceId>() {
        anyhow::ensure!(
            registry.get(&id).is_some(),
            "no instance with id {selector}"
        );
        return Ok(id);
    }

    let matches: Vec<_> = registry
        .list()
        .into_iter()
        .filter(|i| i.name.eq_ignore_ascii_case(selector))
        .collect();

    match matches.len() {
        0 => anyhow::bail!("no instance called {selector:?}"),
        1 => Ok(matches[0].id),
        n => anyhow::bail!(
            "{n} instances are called {selector:?} — rename them or address one by id, \
             because guessing which you meant could put the wrong graphic on air"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rookery_core::Instance;
    use rookery_osc::Arg;

    fn registry_with(names: &[&str]) -> Registry {
        let dir = std::env::temp_dir().join(format!(
            "rookery-dispatch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let registry = Registry::load_or_new(dir.join("registry.json")).unwrap();
        for name in names {
            registry
                .upsert(Instance::new(*name, "127.0.0.1").with_tags(&["stage"]))
                .unwrap();
        }
        registry
    }

    fn parse(address: &str, args: &[Arg], registry: &Registry) -> anyhow::Result<NorthboundCue> {
        parse_northbound(DEFAULT_NORTHBOUND_PREFIX, address, args, registry)
    }

    #[test]
    fn a_group_cue_targets_the_tag_and_the_primary_source() {
        let registry = registry_with(&["gfx-1"]);
        let cue = parse(
            "/rookery/group/stage/url",
            &[Arg::Str("https://example.com".into())],
            &registry,
        )
        .unwrap();
        assert_eq!(
            cue.target,
            Target {
                scope: Scope::Group {
                    tag: "stage".into()
                },
                source: None
            }
        );
        assert_eq!(
            cue.command,
            Command::Url {
                url: "https://example.com".into()
            }
        );
    }

    #[test]
    fn a_source_selector_sits_between_the_scope_and_the_verb() {
        let registry = registry_with(&["gfx-1"]);
        let cue = parse(
            "/rookery/group/stage/source/lower-third/reload",
            &[Arg::Int(1)],
            &registry,
        )
        .unwrap();
        assert_eq!(cue.target.source.as_deref(), Some("lower-third"));
        assert_eq!(cue.command, Command::Reload { ignore_cache: true });
    }

    #[test]
    fn an_output_verb_keeps_its_name_including_a_space() {
        let registry = registry_with(&["gfx-1"]);
        let cue = parse(
            "/rookery/all/output/Programme Fill",
            &[Arg::Int(0)],
            &registry,
        )
        .unwrap();
        assert_eq!(cue.target.scope, Scope::All);
        assert_eq!(
            cue.command,
            Command::Output {
                name: "Programme Fill".into(),
                enabled: false
            }
        );
    }

    #[test]
    fn an_instance_can_be_named_rather_than_uuided() {
        let registry = registry_with(&["gfx-1", "gfx-2"]);
        let expected = registry
            .list()
            .into_iter()
            .find(|i| i.name == "gfx-2")
            .unwrap()
            .id;
        let cue = parse("/rookery/instance/gfx-2/reload", &[], &registry).unwrap();
        assert_eq!(cue.target.scope, Scope::Instance { id: expected });

        // …and by id, which is what the web UI sends.
        let cue = parse(
            &format!("/rookery/instance/{expected}/reload"),
            &[],
            &registry,
        )
        .unwrap();
        assert_eq!(cue.target.scope, Scope::Instance { id: expected });
    }

    #[test]
    fn an_ambiguous_name_is_refused_rather_than_guessed() {
        let registry = registry_with(&["gfx", "gfx"]);
        let err = parse("/rookery/instance/gfx/reload", &[], &registry).unwrap_err();
        assert!(err.to_string().contains("2 instances"), "{err}");
    }

    #[test]
    fn names_are_matched_case_insensitively() {
        let registry = registry_with(&["GFX-1"]);
        assert!(parse("/rookery/instance/gfx-1/reload", &[], &registry).is_ok());
    }

    #[test]
    fn malformed_addresses_are_refused_with_a_reason() {
        let registry = registry_with(&["gfx-1"]);
        for address in [
            "/rookery",
            "/rookery/",
            "/rookery/group/stage",       // no verb
            "/rookery/group//url",        // empty tag
            "/rookery/instance/nope/url", // unknown instance
            "/rookery/all/nonsense",      // unknown verb
            "/weblinked/all/reload",      // wrong prefix
            "/rookery/all/source/x",      // source but no verb
        ] {
            assert!(
                parse(address, &[Arg::Str("x".into())], &registry).is_err(),
                "{address:?} should not parse"
            );
        }
    }

    /// The point of keeping WebLinked's verbs unchanged: an existing button
    /// is retargeted by editing the front of the address only.
    #[test]
    fn a_weblinked_address_becomes_a_rookery_one_by_swapping_the_head() {
        let registry = registry_with(&["gfx-1"]);
        let weblinked = "/weblinked/source/lower-third/output/Graphic";
        let tail = weblinked.strip_prefix("/weblinked/").unwrap();
        let rookery = format!("/rookery/group/stage/{tail}");

        let cue = parse(&rookery, &[Arg::Int(1)], &registry).unwrap();
        assert_eq!(cue.target.source.as_deref(), Some("lower-third"));
        assert_eq!(
            cue.command,
            Command::Output {
                name: "Graphic".into(),
                enabled: true
            }
        );
    }
}
