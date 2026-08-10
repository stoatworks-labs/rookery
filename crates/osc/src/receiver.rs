//! Northbound UDP receiver — the socket a lighting desk, QLab cue or
//! Companion button sends to when it wants to move a whole group at once.
//!
//! Off by default in the binary. An OSC receiver has no authentication —
//! that is the protocol, not a choice made here — so anything that can reach
//! the port can retarget every instance in the fleet.

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::{decode_packet, Message};

#[derive(Debug, Clone)]
pub struct ReceivedMessage {
    pub message: Message,
    pub from: SocketAddr,
}

pub struct OscReceiver {
    pub messages: mpsc::Receiver<ReceivedMessage>,
    pub local_addr: SocketAddr,
}

impl OscReceiver {
    /// Binds `bind` and spawns the receive loop. The returned channel yields
    /// every message decoded from every datagram, bundles flattened.
    pub async fn bind(bind: &str) -> anyhow::Result<Self> {
        let socket = UdpSocket::bind(bind).await?;
        let local_addr = socket.local_addr()?;
        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            // Comfortably over a jumbo frame; a single OSC command is a few
            // dozen bytes and the largest realistic one is a long script.
            let mut buffer = vec![0u8; 65536];
            loop {
                let (len, from) = match socket.recv_from(&mut buffer).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("osc receive failed: {e}");
                        break;
                    }
                };
                let mut batch = Vec::new();
                decode_packet(&buffer[..len], &mut |message| {
                    batch.push(ReceivedMessage { message, from })
                });
                for received in batch {
                    // A full channel means the consumer has stalled. Drop
                    // rather than block the socket: a backlog of stale cues
                    // applied late is worse than a cue that visibly did
                    // nothing.
                    if tx.try_send(received).is_err() {
                        tracing::warn!(%from, "osc inbound queue full, dropping message");
                    }
                }
            }
        });

        Ok(Self {
            messages: rx,
            local_addr,
        })
    }
}
