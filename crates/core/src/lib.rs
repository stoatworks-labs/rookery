//! rookery's transport-agnostic core: what an instance is, how groups are
//! derived, what can be commanded, and what state comes back.
//!
//! Knows nothing about UDP, HTTP or axum — see `rookery-instance-live` for
//! the real transport and `rookery-instance-mock` for the simulated one.

pub mod client;
pub mod command;
mod crypto;
pub mod instance;
pub mod preview;
pub mod registry;
pub mod state;

pub use client::{InstanceClient, InstanceClientProvider};
pub use command::Command;
pub use instance::{
    Instance, InstanceCredentials, InstanceId, DEFAULT_HTTP_PORT, DEFAULT_OSC_PORT,
    DEFAULT_OSC_PREFIX,
};
pub use preview::{
    InputBatch, InputEvent, KeyAction, PreviewFrame, PreviewUnavailable, FOCUS_FACTOR, WALL_FACTOR,
};
pub use registry::Registry;
pub use state::{
    AudioInfo, Health, InstanceState, OutputInfo, PacingInfo, SourceInfo, SourceState, SourcesState,
};
