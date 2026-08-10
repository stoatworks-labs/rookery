//! Who a command is aimed at.

use serde::{Deserialize, Serialize};

use rookery_core::InstanceId;

/// Which instances a command reaches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum Scope {
    /// Exactly one instance.
    Instance { id: InstanceId },
    /// Every instance carrying a tag. Membership is resolved at send time, so
    /// retagging takes effect on the next cue with no reconciliation step.
    Group { tag: String },
    /// Every instance in the registry.
    All,
}

impl Scope {
    pub fn describe(&self) -> String {
        match self {
            Scope::Instance { id } => format!("instance {id}"),
            Scope::Group { tag } => format!("group {tag:?}"),
            Scope::All => "all instances".to_string(),
        }
    }
}

/// A scope plus which pipeline inside each instance to address.
///
/// `source` is `None` for the primary — the only pipeline a plain
/// command-line WebLinked has, and what keeps a single-source fleet
/// addressable without anyone knowing source ids exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    #[serde(flatten)]
    pub scope: Scope,
    #[serde(default)]
    pub source: Option<String>,
}

impl Target {
    pub fn instance(id: InstanceId) -> Self {
        Self {
            scope: Scope::Instance { id },
            source: None,
        }
    }

    pub fn group(tag: impl Into<String>) -> Self {
        Self {
            scope: Scope::Group { tag: tag.into() },
            source: None,
        }
    }

    pub fn all() -> Self {
        Self {
            scope: Scope::All,
            source: None,
        }
    }

    pub fn with_source(mut self, source: Option<String>) -> Self {
        self.source = source;
        self
    }

    pub fn describe(&self) -> String {
        match &self.source {
            Some(s) => format!("{} (source {s})", self.scope.describe()),
            None => self.scope.describe(),
        }
    }
}
