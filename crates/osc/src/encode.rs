//! OSC encoding. See the crate docs for the padding rule this depends on.

use crate::{padded, Arg, Message};

/// Writes an OSC string: the text, a mandatory NUL, then NUL padding to a
/// 4-byte boundary.
fn write_string(out: &mut Vec<u8>, text: &str) {
    let start = out.len();
    out.extend_from_slice(text.as_bytes());
    // `padded` counts the terminator, so this resize supplies it along with
    // any further padding. Never pass `text.len() + 1` here.
    out.resize(start + padded(text.len()), 0);
}

pub fn encode_message(message: &Message) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    write_string(&mut out, &message.address);

    let mut tags = String::with_capacity(message.args.len() + 1);
    tags.push(',');
    for arg in &message.args {
        tags.push(arg.type_tag() as char);
    }
    write_string(&mut out, &tags);

    for arg in &message.args {
        match arg {
            Arg::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Bool(v) => out.extend_from_slice(&i32::from(*v).to_be_bytes()),
            Arg::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Str(v) => write_string(&mut out, v),
            // T and F carry their value in the type tag and nothing here.
            Arg::Flag(_) => {}
        }
    }
    out
}

/// Bundles several messages into one datagram.
///
/// The timetag is always "immediate" (1). WebLinked ignores timetags and
/// dispatches a bundle's contents straight away — a desk that wanted them
/// later would not have sent them now — so encoding anything else here would
/// promise scheduling that no receiver in this system honours.
pub fn encode_bundle(messages: &[Message]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + messages.len() * 64);
    write_string(&mut out, "#bundle");
    out.extend_from_slice(&1u64.to_be_bytes());
    for message in messages {
        let encoded = encode_message(message);
        out.extend_from_slice(&(encoded.len() as i32).to_be_bytes());
        out.extend_from_slice(&encoded);
    }
    out
}
