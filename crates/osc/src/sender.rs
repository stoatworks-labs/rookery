//! Southbound UDP sender.
//!
//! One socket, shared by every instance rookery talks to. A socket per
//! destination would buy nothing — UDP is connectionless, and binding N
//! ephemeral ports just to send to N hosts makes the traffic harder to read
//! in a capture, not easier.
//!
//! **A send that returns `Ok` means the datagram left this machine.** It does
//! not mean the instance received it, parsed it, or acted on it: OSC over UDP
//! has no acknowledgement and WebLinked sends nothing back. That is the whole
//! reason rookery also polls each instance's HTTP state — see
//! `rookery-instance-live`. Nothing in the UI may present a successful send
//! as a confirmed change.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::UdpSocket;

use crate::Message;

#[derive(Clone)]
pub struct OscSender {
    socket: Arc<UdpSocket>,
}

impl OscSender {
    /// Binds an ephemeral local port on all interfaces.
    pub async fn new() -> anyhow::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        // Lets a single sender reach a broadcast address if an operator
        // configures one as an instance host. Not the recommended way to
        // drive a fleet (there is no per-instance state that way), but
        // refusing it outright would be a surprise.
        socket.set_broadcast(true).ok();
        Ok(Self {
            socket: Arc::new(socket),
        })
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.socket.local_addr().ok()
    }

    /// Sends one message. `Ok` means "left this machine" — see the module docs.
    pub async fn send(&self, target: SocketAddr, message: &Message) -> anyhow::Result<()> {
        let bytes = message.encode();
        self.send_bytes(target, &bytes).await
    }

    /// Sends several messages as a single bundle, so a receiver applies them
    /// together and the network cannot reorder them relative to each other.
    ///
    /// Worth using whenever one operator action means more than one verb —
    /// setting a URL and then reloading, say — because two separate datagrams
    /// can arrive in either order and the reload would then race the
    /// navigation it was meant to follow.
    pub async fn send_bundle(
        &self,
        target: SocketAddr,
        messages: &[Message],
    ) -> anyhow::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let bytes = crate::encode_bundle(messages);
        self.send_bytes(target, &bytes).await
    }

    async fn send_bytes(&self, target: SocketAddr, bytes: &[u8]) -> anyhow::Result<()> {
        let sent = self.socket.send_to(bytes, target).await?;
        anyhow::ensure!(
            sent == bytes.len(),
            "short UDP write to {target}: {sent} of {} bytes",
            bytes.len()
        );
        tracing::debug!(%target, bytes = bytes.len(), "osc sent");
        Ok(())
    }
}
