//! OSC 1.0 codec and UDP transport.
//!
//! rookery both **sends** OSC (southbound, to every WebLinked instance it manages)
//! and **receives** it (northbound, from a desk or a cue stack), so the codec
//! has to work in both directions and the two halves live here together.
//!
//! ## The padding trap, stated once so nobody has to rediscover it
//!
//! An OSC string is NUL-terminated and *then* padded to a multiple of four,
//! and the terminator is not optional — a string whose length is already a
//! multiple of four still gets four NULs after it. So the wire size of an
//! `n`-character string is `(n + 4) & !3`, and that `+ 4` is what supplies
//! the mandatory terminator.
//!
//! This is worth being loud about because WebLinked — the thing on the other
//! end of this socket — shipped a decoder that counted the terminator twice
//! (`padded(textLength + 1)`). That over-advances by four bytes for any
//! string whose length is 3 mod 4, so the read ran off the end and the
//! *whole message* was discarded with no log line. `/weblinked/url` worked
//! for most addresses and silently did nothing for a quarter of them.
//!
//! The lesson generalises past that one bug: a length-residue error in an
//! OSC codec does not fail loudly or fail always, it fails for one input in
//! four. So [`tests::every_string_length_residue_round_trips`] sweeps all
//! four residues for both the address and the arguments, and any change here
//! must keep doing so.

mod decode;
mod encode;
mod receiver;
mod sender;

pub use decode::decode_packet;
pub use encode::{encode_bundle, encode_message};
pub use receiver::{OscReceiver, ReceivedMessage};
pub use sender::OscSender;

use serde::{Deserialize, Serialize};

/// The wire size of an OSC string of `text_len` characters, terminator
/// included. Pass the length of the *text* — never the length with the NUL
/// already added. See the module docs.
pub(crate) const fn padded(text_len: usize) -> usize {
    (text_len + 4) & !3
}

/// One OSC argument.
///
/// Deliberately narrow: these are the types WebLinked's own decoder acts on
/// (`i`, `f`, `s`, `T`, `F`). Anything wider would be encodable but not
/// actionable, which is a worse failure than not offering it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum Arg {
    Int(i32),
    Float(f32),
    Str(String),
    /// Encoded as an `i` of 1/0 rather than OSC's payload-free `T`/`F`.
    ///
    /// Both reach WebLinked's `firstBool` — its decoder pushes `T`/`F` into
    /// the same integer argument list `i` lands in — but `i` is the shape
    /// its documented API and its Companion examples use, and it is what a
    /// generic OSC monitor will render legibly while someone is debugging a
    /// show. Use [`Arg::Flag`] if a receiver genuinely needs `T`/`F`.
    Bool(bool),
    /// OSC's payload-free true/false. Here for receivers that discriminate
    /// on the type tag; rookery never emits it by default.
    Flag(bool),
}

impl Arg {
    pub fn type_tag(&self) -> u8 {
        match self {
            Arg::Int(_) | Arg::Bool(_) => b'i',
            Arg::Float(_) => b'f',
            Arg::Str(_) => b's',
            Arg::Flag(true) => b'T',
            Arg::Flag(false) => b'F',
        }
    }

    /// The argument as a string, for the receiver side where a desk may send
    /// a value as whichever type was convenient.
    pub fn as_string(&self) -> String {
        match self {
            Arg::Int(v) => v.to_string(),
            Arg::Float(v) => v.to_string(),
            Arg::Str(v) => v.clone(),
            Arg::Bool(v) | Arg::Flag(v) => {
                if *v {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
        }
    }

    /// Any non-zero number, or a non-empty string, is true.
    pub fn as_bool(&self) -> bool {
        match self {
            Arg::Int(v) => *v != 0,
            Arg::Float(v) => *v != 0.0,
            Arg::Str(v) => !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"),
            Arg::Bool(v) | Arg::Flag(v) => *v,
        }
    }
}

/// A single OSC message: an address pattern and its arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub address: String,
    #[serde(default)]
    pub args: Vec<Arg>,
}

impl Message {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            args: Vec::new(),
        }
    }

    pub fn with(mut self, arg: Arg) -> Self {
        self.args.push(arg);
        self
    }

    pub fn with_str(self, value: impl Into<String>) -> Self {
        self.with(Arg::Str(value.into()))
    }

    pub fn with_bool(self, value: bool) -> Self {
        self.with(Arg::Bool(value))
    }

    /// First string argument, or empty — mirrors WebLinked's `firstString()`.
    pub fn first_string(&self) -> String {
        self.args
            .iter()
            .find_map(|a| match a {
                Arg::Str(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// First numeric argument as a bool, `fallback` when there are no
    /// arguments at all — mirrors WebLinked's `firstBool(fallback)`, which is
    /// how a bare trigger address is told apart from an explicit zero.
    pub fn first_bool(&self, fallback: bool) -> bool {
        match self.args.first() {
            Some(arg) => arg.as_bool(),
            None => fallback,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        encode_message(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_supplies_the_mandatory_terminator() {
        // The whole point: a length already a multiple of four still grows.
        assert_eq!(padded(0), 4);
        assert_eq!(padded(1), 4);
        assert_eq!(padded(2), 4);
        assert_eq!(padded(3), 4);
        assert_eq!(padded(4), 8);
        assert_eq!(padded(7), 8);
        assert_eq!(padded(8), 12);
    }

    /// The regression test this crate exists to keep passing.
    ///
    /// A residue error in string padding does not fail always — it fails for
    /// one input in four, which is exactly how it survives casual testing.
    /// So sweep every residue, in both the address and the arguments, and
    /// check the message survives a round trip intact.
    #[test]
    fn every_string_length_residue_round_trips() {
        for addr_extra in 0..8 {
            for arg_len in 0..8 {
                let address = format!("/rookery/{}", "a".repeat(addr_extra));
                let payload = "u".repeat(arg_len);
                let message = Message::new(&address)
                    .with_str(&payload)
                    .with(Arg::Int(7))
                    .with_str("tail");

                let bytes = message.encode();
                assert_eq!(
                    bytes.len() % 4,
                    0,
                    "packet for address {address:?} / arg len {arg_len} is not 4-byte aligned"
                );

                let mut decoded = Vec::new();
                decode_packet(&bytes, &mut |m| decoded.push(m));
                assert_eq!(
                    decoded.len(),
                    1,
                    "address {address:?} with a {arg_len}-char argument decoded to \
                     {} messages, not 1 — this is the residue bug",
                    decoded.len()
                );
                assert_eq!(decoded[0], message);
            }
        }
    }

    #[test]
    fn a_message_with_no_arguments_is_a_valid_trigger() {
        let message = Message::new("/rookery/reload");
        let bytes = message.encode();
        let mut decoded = Vec::new();
        decode_packet(&bytes, &mut |m| decoded.push(m));
        assert_eq!(decoded, vec![message]);
    }

    #[test]
    fn first_bool_distinguishes_a_bare_trigger_from_an_explicit_zero() {
        assert!(Message::new("/x").first_bool(true));
        assert!(!Message::new("/x").with_bool(false).first_bool(true));
        assert!(Message::new("/x").with_bool(true).first_bool(false));
    }

    #[test]
    fn bundles_carry_every_element_in_order() {
        let a = Message::new("/rookery/url").with_str("https://example.com/one");
        let b = Message::new("/rookery/reload").with(Arg::Int(1));
        let bytes = encode_bundle(&[a.clone(), b.clone()]);

        let mut decoded = Vec::new();
        decode_packet(&bytes, &mut |m| decoded.push(m));
        assert_eq!(decoded, vec![a, b]);
    }

    /// `Bool` is an encoder-side convenience, not a wire type: it goes out as
    /// a 4-byte `i`, which is exactly what WebLinked's `firstBool` reads and
    /// what its documented API asks for. So a round trip returns `Int`, and
    /// that asymmetry is deliberate — asserting it here stops someone
    /// "fixing" it into `T`/`F` later and quietly changing the type tag every
    /// receiver in the fleet is matched against.
    #[test]
    fn bool_goes_out_as_an_integer_and_comes_back_as_one() {
        let bytes = Message::new("/rookery/mute").with_bool(true).encode();
        let mut decoded = Vec::new();
        decode_packet(&bytes, &mut |m| decoded.push(m));
        assert_eq!(decoded[0].args, vec![Arg::Int(1)]);
        // Semantically unchanged, which is what actually matters.
        assert!(decoded[0].first_bool(false));

        // Flag is the opt-in payload-free form, and does survive intact.
        let bytes = Message::new("/rookery/mute").with(Arg::Flag(true)).encode();
        let mut decoded = Vec::new();
        decode_packet(&bytes, &mut |m| decoded.push(m));
        assert_eq!(decoded[0].args, vec![Arg::Flag(true)]);
    }

    #[test]
    fn a_truncated_packet_is_dropped_rather_than_half_applied() {
        let bytes = Message::new("/rookery/url")
            .with_str("https://example.com/one")
            .encode();
        for cut in 1..bytes.len() {
            let mut decoded = Vec::new();
            decode_packet(&bytes[..cut], &mut |m| decoded.push(m));
            for m in decoded {
                // Whatever survives must never be a message claiming an
                // argument it no longer has the bytes for.
                if let Some(Arg::Str(s)) = m.args.first() {
                    assert!(
                        "https://example.com/one".starts_with(s.as_str()) || s.is_empty(),
                        "truncation at {cut} produced a fabricated argument {s:?}"
                    );
                }
            }
        }
    }
}
