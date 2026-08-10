//! rookery's view of a WebLinked instance's state.
//!
//! These types deserialise WebLinked's `/api/state` and `/api/sources`. Two
//! rules govern how much they model:
//!
//! 1. **Every field is optional.** WebLinked's response varies by build —
//!    `displays` only appears where the screen backend is compiled in,
//!    `receivers` only on NDI/OMT outputs, `buffered_frames` only on
//!    DeckLink — and by version. A fleet tool that fails to parse a whole
//!    instance because one field is missing turns a cosmetic difference into
//!    an outage in the dashboard.
//! 2. **Unknown fields are ignored, never rejected.** Pointing an older
//!    rookery at a newer WebLinked must degrade to "shows less", not
//!    "shows nothing".
//!
//! What is deliberately *not* here: the settings-editing shapes. rookery
//! reads state and sends commands; it is not a second copy of WebLinked's
//! settings page.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceInfo {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub loaded_url: Option<String>,
    #[serde(default)]
    pub loading: Option<bool>,
    #[serde(default)]
    pub paints: Option<u64>,
    #[serde(default)]
    pub console_errors: Option<u64>,
    #[serde(default)]
    pub audio_muted: Option<bool>,
    #[serde(default)]
    pub popups: Option<u64>,
    #[serde(default)]
    pub pacing: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputInfo {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub running: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub frames: Option<u64>,
    /// NDI/OMT only.
    #[serde(default)]
    pub receivers: Option<u64>,
    /// DeckLink only. Steady means our clock and the card's agree.
    #[serde(default)]
    pub buffered_frames: Option<i64>,
    /// Screen only.
    #[serde(default)]
    pub presented: Option<u64>,
    #[serde(default)]
    pub dropped: Option<u64>,
    /// Present when the backend refused — a format the card cannot do, a
    /// device already claimed by another application.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PacingInfo {
    #[serde(default)]
    pub ticks: Option<u64>,
    #[serde(default)]
    pub repeated_frames: Option<u64>,
    /// Should stay 0. Anything else means the clock fell more than a frame
    /// behind, which is the number to put in front of an operator.
    #[serde(default)]
    pub dropped_ticks: Option<u64>,
    #[serde(default)]
    pub frames_published: Option<u64>,
    #[serde(default)]
    pub last_lateness_us: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioInfo {
    #[serde(default)]
    pub channels: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub underruns: Option<u64>,
    #[serde(default)]
    pub overruns: Option<u64>,
}

/// One source's state, the shape `/api/state` returns and `/api/sources`
/// repeats per pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceState {
    /// Present in `/api/sources` entries; absent from a bare `/api/state`,
    /// where the source is whichever one was addressed.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub running: Option<bool>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub outputs: Vec<OutputInfo>,
    #[serde(default)]
    pub compiled_backends: Vec<String>,
    #[serde(default)]
    pub source: SourceInfo,
    #[serde(default)]
    pub pacing: PacingInfo,
    #[serde(default)]
    pub audio: AudioInfo,

    /// True when `pacing.dropped_ticks` **rose between the last two polls**.
    ///
    /// Filled in by `rookery-fleet`, never by WebLinked — hence
    /// `skip_deserializing`, so a response can't spoof it.
    ///
    /// This exists because the cumulative counter is not a health signal, a
    /// fact established by pointing rookery at a real WebLinked 0.7.1 rather
    /// than reasoned from the docs. A perfectly healthy instance rendering a
    /// static page headless had `dropped_ticks: 346` against 2205 ticks:
    /// macOS throttles a backgrounded process, the clock falls behind, and
    /// the count climbs and then *stays* climbed forever. Treating "has ever
    /// dropped a tick" as degraded paints every long-running instance amber
    /// within minutes, and an indicator that is always amber tells an
    /// operator nothing.
    ///
    /// "Is it dropping *now*" is the question worth answering mid-show, so
    /// that is the one this answers.
    #[serde(default, skip_deserializing)]
    pub dropping: bool,
}

impl SourceState {
    /// The one-line health verdict the UI colours a row by.
    ///
    /// Deliberately conservative: anything rookery cannot see is `Unknown`,
    /// never `Ok`. A green light that means "no news" is worse than a grey
    /// one that means "no news".
    pub fn health(&self) -> Health {
        if self.running == Some(false) {
            return Health::Stopped;
        }
        if self.outputs.iter().any(|o| o.error.is_some()) {
            return Health::Fault;
        }
        // The delta, not the cumulative count — see `dropping`.
        if self.dropping {
            return Health::Degraded;
        }
        if self
            .outputs
            .iter()
            .any(|o| o.enabled == Some(true) && o.running == Some(false))
        {
            return Health::Degraded;
        }
        if self.running == Some(true) {
            Health::Ok
        } else {
            Health::Unknown
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Degraded,
    Fault,
    Stopped,
    Unknown,
}

/// `/api/sources`: every pipeline in one process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourcesState {
    #[serde(default)]
    pub primary: Option<String>,
    #[serde(default)]
    pub sources: Vec<SourceState>,
}

/// What rookery knows about one instance right now: either a snapshot, or
/// why there isn't one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceState {
    /// `None` while the first poll is still outstanding, or when polling is
    /// switched off for this instance.
    #[serde(default)]
    pub sources: Option<SourcesState>,
    /// Why the last poll failed, if it did. Kept alongside a stale snapshot
    /// rather than replacing it — "last seen 40s ago, now unreachable" is
    /// more use mid-show than a blank row.
    #[serde(default)]
    pub error: Option<String>,
    /// Milliseconds since the last successful poll.
    #[serde(default)]
    pub age_ms: Option<u64>,
    /// False when `Instance::poll` is off — the UI shows "not polled" rather
    /// than an error, because this is a choice, not a fault.
    #[serde(default)]
    pub polled: bool,
}

impl Default for InstanceState {
    fn default() -> Self {
        Self {
            sources: None,
            error: None,
            age_ms: None,
            polled: true,
        }
    }
}

impl InstanceState {
    pub fn health(&self) -> Health {
        if !self.polled {
            return Health::Unknown;
        }
        if self.error.is_some() && self.sources.is_none() {
            return Health::Fault;
        }
        let Some(sources) = &self.sources else {
            return Health::Unknown;
        };
        // The worst source decides the instance. One black graphic out of
        // three is still a problem someone has to be told about.
        sources
            .sources
            .iter()
            .map(|s| s.health())
            .max_by_key(|h| match h {
                Health::Ok => 0,
                Health::Unknown => 1,
                Health::Degraded => 2,
                Health::Stopped => 3,
                Health::Fault => 4,
            })
            .unwrap_or(Health::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The response shape documented in WebLinked's docs/03-control-api.md,
    /// trimmed to one output. Parsing this is the contract.
    const REAL_STATE: &str = r##"{
      "version": "0.7.0", "running": true, "format": "1920x1080p50",
      "format_detail": { "width": 1920, "height": 1080,
                         "rate_numerator": 50, "rate_denominator": 1,
                         "interlaced": false },
      "outputs": [
        { "kind": "ndi", "name": "Graphic", "running": true, "enabled": true,
          "device_index": 0, "options": { "alpha": true },
          "background": "transparent", "background_colour": "#00b140",
          "pixel_format": "UYVY", "frames": 10464, "audio_frames": 10460,
          "receivers": 1, "library": "/Library/NDI SDK for Apple/lib/macOS/libndi.dylib" }
      ],
      "compiled_backends": ["preview", "ndi", "omt", "decklink", "screen"],
      "source": { "url": "https://example.com", "loaded_url": "https://example.com",
                  "loading": false, "paints": 10440, "console_errors": 0,
                  "audio_muted": false, "pacing": "external", "popups": 0 },
      "settings": { "matrix": "auto" },
      "pacing": { "ticks": 10463, "repeated_frames": 23, "dropped_ticks": 0,
                  "frames_published": 10440, "last_lateness_us": 3566 },
      "audio": { "channels": 2, "sample_rate": 48000, "underruns": 0, "overruns": 0 }
    }"##;

    #[test]
    fn parses_a_real_state_response_and_ignores_what_it_does_not_model() {
        let state: SourceState = serde_json::from_str(REAL_STATE).unwrap();
        assert_eq!(state.format.as_deref(), Some("1920x1080p50"));
        assert_eq!(state.outputs.len(), 1);
        assert_eq!(state.outputs[0].receivers, Some(1));
        assert_eq!(state.pacing.dropped_ticks, Some(0));
        assert_eq!(state.health(), Health::Ok);
    }

    #[test]
    fn a_minimal_response_from_a_stripped_build_still_parses() {
        // No screen backend, no audio block, no pacing block.
        let state: SourceState =
            serde_json::from_str(r#"{"running": true, "outputs": []}"#).unwrap();
        assert_eq!(state.health(), Health::Ok);
        assert!(state.compiled_backends.is_empty());
    }

    #[test]
    fn an_output_error_outranks_everything_else_being_fine() {
        let mut state: SourceState = serde_json::from_str(REAL_STATE).unwrap();
        state.outputs[0].error = Some("device in use".to_string());
        assert_eq!(state.health(), Health::Fault);
    }

    #[test]
    fn currently_dropping_ticks_degrades_an_otherwise_healthy_source() {
        let mut state: SourceState = serde_json::from_str(REAL_STATE).unwrap();
        state.dropping = true;
        assert_eq!(state.health(), Health::Degraded);
    }

    /// Measured against a real WebLinked 0.7.1: a healthy instance rendering
    /// a static page headless reported 346 dropped ticks out of 2205 and
    /// stayed that way. A cumulative count must not colour it amber.
    #[test]
    fn a_historic_dropped_tick_count_alone_is_not_degraded() {
        let mut state: SourceState = serde_json::from_str(REAL_STATE).unwrap();
        state.pacing.dropped_ticks = Some(346);
        state.pacing.ticks = Some(2205);
        state.dropping = false;
        assert_eq!(state.health(), Health::Ok);
    }

    /// `dropping` is rookery's own derived field. A WebLinked response must
    /// never be able to set it, or an instance could claim to be healthy
    /// while dropping frames.
    #[test]
    fn dropping_cannot_be_set_from_the_wire() {
        let state: SourceState =
            serde_json::from_str(r#"{"running": true, "dropping": true}"#).unwrap();
        assert!(!state.dropping);
    }

    #[test]
    fn an_unpolled_instance_is_unknown_not_ok() {
        let state = InstanceState {
            polled: false,
            ..Default::default()
        };
        assert_eq!(state.health(), Health::Unknown);
    }

    #[test]
    fn the_worst_source_decides_the_instance() {
        let good: SourceState = serde_json::from_str(REAL_STATE).unwrap();
        let mut bad = good.clone();
        bad.outputs[0].error = Some("no card".to_string());
        let state = InstanceState {
            sources: Some(SourcesState {
                primary: Some("main".to_string()),
                sources: vec![good, bad],
            }),
            ..Default::default()
        };
        assert_eq!(state.health(), Health::Fault);
    }
}
