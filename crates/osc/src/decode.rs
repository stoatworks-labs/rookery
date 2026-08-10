//! OSC decoding, for the northbound receiver.
//!
//! Structured to mirror WebLinked's own decoder so the two agree about what
//! a valid packet is: a malformed message is dropped whole rather than
//! delivered with the arguments that happened to parse. Half a command is
//! worse than none — "go to this URL" with the URL missing is a black
//! graphic on air.

use crate::{padded, Arg, Message};

fn read_string(data: &[u8], offset: &mut usize) -> Option<String> {
    if *offset >= data.len() {
        return None;
    }
    let rest = &data[*offset..];
    let text_len = rest.iter().position(|b| *b == 0)?;
    let text = std::str::from_utf8(&rest[..text_len]).ok()?.to_string();
    // `padded` already counts the terminator — see the crate docs for what
    // passing `text_len + 1` here costs.
    *offset += padded(text_len);
    if *offset > data.len() {
        return None;
    }
    Some(text)
}

fn read_i32(data: &[u8], offset: &mut usize) -> Option<i32> {
    let end = offset.checked_add(4)?;
    if end > data.len() {
        return None;
    }
    let bytes: [u8; 4] = data[*offset..end].try_into().ok()?;
    *offset = end;
    Some(i32::from_be_bytes(bytes))
}

/// Decodes one datagram, calling `handler` for every message it contains.
///
/// `handler` takes `&mut` rather than being a plain `Fn` so a caller can
/// collect into a `Vec` without interior mutability.
pub fn decode_packet(data: &[u8], handler: &mut dyn FnMut(Message)) {
    if data.len() < 4 {
        return;
    }

    if data.len() >= 8 && &data[..7] == b"#bundle" {
        // "#bundle\0" plus an 8-byte timetag, then length-prefixed elements.
        let mut offset = 16;
        while offset + 4 <= data.len() {
            let mut size_offset = offset;
            let Some(size) = read_i32(data, &mut size_offset) else {
                return;
            };
            if size <= 0 {
                return;
            }
            offset = size_offset;
            let Some(end) = offset.checked_add(size as usize) else {
                return;
            };
            if end > data.len() {
                return;
            }
            decode_packet(&data[offset..end], handler);
            offset = end;
        }
        return;
    }

    let mut offset = 0;
    let Some(address) = read_string(data, &mut offset) else {
        return;
    };
    if !address.starts_with('/') {
        return;
    }

    // The type tag string is optional in OSC 1.0; a message without one
    // carries no arguments, which is a valid trigger.
    let mut tags = String::new();
    if data.get(offset) == Some(&b',') {
        match read_string(data, &mut offset) {
            Some(t) => tags = t,
            None => return,
        }
    }

    let mut args = Vec::new();
    for tag in tags.chars().skip(1) {
        match tag {
            'i' => match read_i32(data, &mut offset) {
                Some(v) => args.push(Arg::Int(v)),
                None => return,
            },
            'f' => match read_i32(data, &mut offset) {
                Some(bits) => args.push(Arg::Float(f32::from_bits(bits as u32))),
                None => return,
            },
            's' | 'S' => match read_string(data, &mut offset) {
                Some(v) => args.push(Arg::Str(v)),
                None => return,
            },
            'T' => args.push(Arg::Flag(true)),
            'F' => args.push(Arg::Flag(false)),
            'N' | 'I' => {}
            'b' => {
                // Skip a blob rather than failing the message: an unrelated
                // argument should not discard a valid command.
                let Some(size) = read_i32(data, &mut offset) else {
                    return;
                };
                if size < 0 {
                    return;
                }
                // Not `padded()`. A blob is padded to a 4-byte boundary but
                // has no terminator, so a 4-byte blob occupies 4 bytes —
                // whereas a 4-character string occupies 8. Using the string
                // rule here would over-advance by four on every blob whose
                // length is already a multiple of four.
                offset += (size as usize + 3) & !3;
                if offset > data.len() {
                    return;
                }
            }
            'd' | 'h' | 't' => {
                offset += 8;
                if offset > data.len() {
                    return;
                }
            }
            // An unknown tag means nothing after it can be located.
            _ => return,
        }
    }

    handler(Message { address, args });
}
