//! Live pictures, and input back into them.
//!
//! ## Why the frame is expensive, and what to do about it
//!
//! WebLinked's `/api/preview` hands back a **raw BGRA buffer** — no JPEG, no
//! compression, no WebSocket. That is a sound choice for its own control page,
//! which is one client on loopback blitting into a canvas. It is a poor one for
//! a fleet panel, and the numbers are not marginal:
//!
//! | Preview factor | Raster (from 1080p) | Bytes per frame |
//! |---|---|---|
//! | 1 (the implicit default) | 1920x1080 | 8,294,400 |
//! | 4 | 480x270 | 518,400 |
//! | 8 | 240x135 | 129,600 |
//!
//! Measured against a real WebLinked 0.7.1, not inferred. An instance started
//! without an explicit `--preview` runs at **factor 1**, so naively polling a
//! fleet of eight at 4 fps would be 265 MB/s. The factor is what makes this
//! feature possible at all.
//!
//! ## Changing the factor is not free of consequence
//!
//! The factor belongs to the instance's own preview output, and WebLinked's own
//! control page reads that same output. Turning it down to suit rookery makes
//! the picture on *that machine's* control page smaller too. So rookery never
//! changes it silently: `Instance::preview_factor` is `None` by default, which
//! means "use whatever the instance already has", and setting it is a deliberate
//! per-instance decision the UI explains.

use serde::{Deserialize, Serialize};

/// The preview factor rookery asks for when an operator opts in.
///
/// 8 for a thumbnail in a wall of them, 4 for the pane you are actually looking
/// at. Not 1: at 8 MB a frame it is not a preview, it is the programme feed.
pub const WALL_FACTOR: u8 = 8;
pub const FOCUS_FACTOR: u8 = 4;

/// One decoded frame, straight off the wire.
#[derive(Clone)]
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    /// WebLinked's `X-Frame-Sequence`: an identifier for the **paint**, not the
    /// tick. It advances at the page's paint rate on an animated graphic and
    /// stays put on a static one — verified against a real instance, where a
    /// static scoreboard held sequence 0 while an animated clock climbed at
    /// ~50/s. That makes it exactly the right thing to hang an ETag on: a
    /// graphic that is not moving costs nothing to keep on screen.
    pub sequence: i64,
    /// Raw BGRA, `width * height * 4` bytes.
    pub bgra: Vec<u8>,
}

/// Hand-rolled so a failed assertion prints the shape of a frame and not its
/// contents. A derived `Debug` on an 8 MB buffer turns one panic into several
/// million lines of hex.
impl std::fmt::Debug for PreviewFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("sequence", &self.sequence)
            .field("bytes", &self.bgra.len())
            .finish()
    }
}

impl PreviewFrame {
    pub fn expected_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// True when the buffer is the size the headers claim.
    ///
    /// Worth checking rather than trusting: a short read here would be decoded
    /// as a picture rather than as an error, and a half-black preview of a
    /// graphic that is actually fine is worse than no preview.
    pub fn is_complete(&self) -> bool {
        self.bgra.len() == self.expected_len()
    }
}

/// Why an instance has no picture. Each of these is a normal state, not a
/// fault, and the UI says which rather than showing an empty box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewUnavailable {
    /// The instance was started with `--no-preview`. A deliberate choice on a
    /// machine that only needs to push SDI, and a 404 from `/api/preview`.
    NotConfigured,
    /// Running, but no frame has been produced yet — a 503. Normal for the
    /// second after a format change, when every output is reopening.
    NoFrameYet,
}

/// One input event, in WebLinked's own shape.
///
/// Positions are **normalised** 0..1 across the raster, which is what makes
/// this safe to drive from a thumbnail: rookery's canvas is whatever size the
/// layout gave it, and the instance scales to its own current raster. A format
/// change mid-drag therefore cannot send a click off the edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum InputEvent {
    Move {
        nx: f32,
        ny: f32,
    },
    Down {
        nx: f32,
        ny: f32,
        #[serde(default)]
        button: u8,
        #[serde(default = "one")]
        clicks: u8,
    },
    Up {
        nx: f32,
        ny: f32,
        #[serde(default)]
        button: u8,
    },
    Wheel {
        nx: f32,
        ny: f32,
        #[serde(default)]
        dx: f32,
        #[serde(default)]
        dy: f32,
    },
    Key {
        action: KeyAction,
        key_code: i32,
        /// The **character code**, not the key code — 104 is `h`, 72 is `H`.
        ///
        /// Both fields are required on the keydown as well as the char event.
        /// With only a virtual key code Chromium cannot tell which key was
        /// pressed and the page sees `e.key` as `"Unidentified"`, so a graphic
        /// listening for a specific key never fires. Verified: `key_code` 72
        /// with `character` 104 puts `h` in a real text field; `character` 72
        /// puts `H`.
        #[serde(default)]
        character: i32,
        #[serde(default)]
        modifiers: i32,
    },
    Focus {
        focused: bool,
    },
}

fn one() -> u8 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAction {
    Down,
    Char,
    Up,
}

impl InputEvent {
    /// True for anything that changes what the page is doing, as opposed to
    /// merely moving the pointer over it.
    ///
    /// The UI uses this to decide what needs the take-control arm: hovering a
    /// preview is harmless, clicking into a graphic that is on air is not.
    pub fn is_actuating(&self) -> bool {
        !matches!(self, InputEvent::Move { .. } | InputEvent::Focus { .. })
    }
}

/// A batch, which is how the UI sends pointer motion — one request per drag
/// rather than sixty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputBatch {
    pub events: Vec<InputEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialise_into_weblinkeds_documented_shape() {
        let json = serde_json::to_value(InputEvent::Down {
            nx: 0.5,
            ny: 0.25,
            button: 0,
            clicks: 1,
        })
        .unwrap();
        assert_eq!(json["type"], "down");
        assert_eq!(json["nx"], 0.5);
        assert_eq!(json["clicks"], 1);

        let json = serde_json::to_value(InputEvent::Key {
            action: KeyAction::Char,
            key_code: 72,
            character: 104,
            modifiers: 0,
        })
        .unwrap();
        assert_eq!(json["type"], "key");
        assert_eq!(json["action"], "char");
        assert_eq!(json["key_code"], 72);
        assert_eq!(json["character"], 104);
    }

    #[test]
    fn a_batch_round_trips() {
        let batch = InputBatch {
            events: vec![
                InputEvent::Focus { focused: true },
                InputEvent::Move { nx: 0.1, ny: 0.2 },
            ],
        };
        let raw = serde_json::to_string(&batch).unwrap();
        let back: InputBatch = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.events.len(), 2);
    }

    #[test]
    fn only_actuating_events_need_arming() {
        assert!(!InputEvent::Move { nx: 0.5, ny: 0.5 }.is_actuating());
        assert!(!InputEvent::Focus { focused: true }.is_actuating());
        assert!(InputEvent::Down {
            nx: 0.5,
            ny: 0.5,
            button: 0,
            clicks: 1
        }
        .is_actuating());
        assert!(InputEvent::Key {
            action: KeyAction::Down,
            key_code: 72,
            character: 104,
            modifiers: 0
        }
        .is_actuating());
        assert!(InputEvent::Wheel {
            nx: 0.5,
            ny: 0.5,
            dx: 0.0,
            dy: -240.0
        }
        .is_actuating());
    }

    #[test]
    fn a_short_buffer_is_not_a_picture() {
        let frame = PreviewFrame {
            width: 240,
            height: 135,
            sequence: 7,
            bgra: vec![0; 240 * 135 * 4],
        };
        assert!(frame.is_complete());

        let truncated = PreviewFrame {
            bgra: vec![0; 100],
            ..frame
        };
        assert!(!truncated.is_complete());
    }
}
