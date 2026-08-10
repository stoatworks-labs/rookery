//! The result of sending one command to many instances.
//!
//! Partial success is the normal case in a fleet, not an edge case: one
//! machine asleep, one with a typo'd hostname, six fine. So a fan-out never
//! collapses to a single ok/err — the caller always gets the per-instance
//! breakdown, and the UI always shows it.

use serde::{Deserialize, Serialize};

use rookery_core::InstanceId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanoutEntry {
    pub instance_id: InstanceId,
    pub instance_name: String,
    /// True when the datagram left this machine. **Not** confirmation that
    /// the instance acted on it — see `rookery-instance-live`.
    pub sent: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fanout {
    /// What the operator asked for, echoed back so a log line or a toast can
    /// be read without the request beside it.
    pub target: String,
    pub command: String,
    pub entries: Vec<FanoutEntry>,
}

impl Fanout {
    pub fn sent_count(&self) -> usize {
        self.entries.iter().filter(|e| e.sent).count()
    }

    pub fn failed_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.sent).count()
    }

    /// True when every instance in the target took the send.
    ///
    /// Note "took the send", not "applied the command". Nothing on this side
    /// of a UDP socket can tell you the latter.
    pub fn fully_sent(&self) -> bool {
        !self.entries.is_empty() && self.failed_count() == 0
    }

    pub fn summary(&self) -> String {
        if self.entries.is_empty() {
            return format!("{} matched no instances", self.target);
        }
        if self.fully_sent() {
            format!("sent to {} instance(s)", self.sent_count())
        } else {
            format!(
                "sent to {} of {} — {} failed",
                self.sent_count(),
                self.entries.len(),
                self.failed_count()
            )
        }
    }
}
