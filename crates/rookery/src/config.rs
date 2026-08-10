use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Where rookery's own web UI listens.
    pub bind: String,
    pub registry_path: String,
    /// How often each instance's HTTP state is polled, in milliseconds.
    ///
    /// 1000 is a compromise: fast enough that a change made from a Companion
    /// button shows up while the operator is still looking at the screen,
    /// slow enough that a fleet of forty is forty requests a second rather
    /// than four hundred.
    pub poll_interval_ms: u64,
    /// The northbound OSC listener — where a desk or cue stack sends.
    ///
    /// `None` (the default) switches it off. **It has no authentication**;
    /// that is OSC, not a choice made here. Anything that can reach this port
    /// can retarget every instance in the fleet, so bind it deliberately.
    pub osc_bind: Option<String>,
    /// The address prefix the northbound listener answers on.
    pub osc_prefix: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8090".to_string(),
            registry_path: "data/registry.json".to_string(),
            poll_interval_ms: 1000,
            osc_bind: None,
            osc_prefix: rookery_fleet::DEFAULT_NORTHBOUND_PREFIX.to_string(),
        }
    }
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        if std::path::Path::new(path).exists() {
            let raw = std::fs::read_to_string(path)?;
            Ok(toml::from_str(&raw)?)
        } else {
            Ok(Self::default())
        }
    }
}
