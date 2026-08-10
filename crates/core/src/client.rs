//! The seam between "what rookery knows about an instance" and "how it
//! actually talks to one".
//!
//! Note the shape of the trait: **sending and observing are separate calls
//! with separate failure modes**, because they are separate protocols. That
//! asymmetry is the defining fact of this project and hiding it behind one
//! `apply_and_confirm` method would be a lie the network cannot support.

use async_trait::async_trait;
use std::sync::Arc;

use crate::command::Command;
use crate::instance::Instance;
use crate::preview::{InputEvent, PreviewFrame, PreviewUnavailable};
use crate::state::SourcesState;

#[async_trait]
pub trait InstanceClient: Send + Sync {
    /// Sends one command over OSC.
    ///
    /// `Ok` means the datagram left this machine. It is **not** confirmation
    /// that the instance received or acted on it — UDP does not acknowledge
    /// and WebLinked sends nothing back. Confirmation, where it exists at
    /// all, comes from the next `state()` poll showing the change.
    async fn send(&self, command: &Command, source: Option<&str>) -> anyhow::Result<()>;

    /// Sends several commands as one bundle, so they cannot be reordered
    /// relative to each other in flight.
    async fn send_all(&self, commands: &[Command], source: Option<&str>) -> anyhow::Result<()>;

    /// Polls the instance's HTTP API for every pipeline's state.
    ///
    /// Fails when the instance is unreachable, when its HTTP server is bound
    /// to loopback (WebLinked's default — it needs `--bind 0.0.0.0` to be
    /// pollable from another machine), or when a token is required and the
    /// stored one is wrong.
    async fn state(&self) -> anyhow::Result<SourcesState>;

    /// Fetches the latest preview frame for one source.
    ///
    /// `Ok(Err(reason))` is a working instance with no picture to give — it was
    /// started `--no-preview`, or has not painted yet. That is a normal state
    /// and not a transport failure, so it is a value rather than an error: the
    /// UI says which, instead of showing an empty box or a red row.
    async fn preview(
        &self,
        source: Option<&str>,
    ) -> anyhow::Result<Result<PreviewFrame, PreviewUnavailable>>;

    /// Delivers input to the page.
    ///
    /// Unlike `send`, this goes over **HTTP**, not OSC — WebLinked exposes no
    /// OSC verb for input, and a click needs to be ordered with respect to the
    /// clicks around it, which UDP will not promise.
    async fn send_input(&self, events: &[InputEvent], source: Option<&str>) -> anyhow::Result<()>;

    /// Asks the instance to run its preview output at `factor`.
    ///
    /// Idempotent and only called when an operator has opted in — see
    /// `Instance::preview_factor` for why this is not done automatically.
    async fn set_preview_factor(&self, factor: u8) -> anyhow::Result<()>;
}

/// Resolves an `Instance` record to the client that talks to it.
pub trait InstanceClientProvider: Send + Sync {
    fn client_for(&self, instance: &Instance) -> Arc<dyn InstanceClient>;
}
