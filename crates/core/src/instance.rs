use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use uuid::Uuid;

/// WebLinked's own defaults, repeated here so an operator adding an instance
/// by hostname alone gets something that works.
pub const DEFAULT_HTTP_PORT: u16 = 7654;
pub const DEFAULT_OSC_PORT: u16 = 7655;
pub const DEFAULT_OSC_PREFIX: &str = "/weblinked";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstanceId(pub Uuid);

impl InstanceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for InstanceId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstanceCredentials {
    /// The instance's `--token`, if it was started with one. Only the HTTP
    /// side uses it; WebLinked's OSC listener has no authentication at all.
    /// Never echoed back to the frontend in full — the API layer redacts it.
    pub token: Option<String>,
}

/// One WebLinked process rookery manages.
///
/// Pure metadata plus how to reach it; the live control surface lives behind
/// `InstanceClient` so swapping the mock for the real transport never touches
/// this struct.
///
/// **Two ports, two directions, and they are not interchangeable.** Commands
/// go out over UDP to `osc_port`; state comes back by polling `http_port`.
/// An instance with a reachable OSC port and an unreachable HTTP port is
/// fully controllable and completely unobservable, which is a real
/// configuration (WebLinked binds HTTP to loopback unless told otherwise) and
/// is why `poll` exists as its own flag rather than being inferred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: InstanceId,
    pub name: String,
    /// Hostname or IP. No scheme, no port.
    pub host: String,
    #[serde(default = "default_osc_port")]
    pub osc_port: u16,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    /// The instance's OSC address prefix. Fixed at `/weblinked` in every
    /// build of WebLinked to date, but it is a compile-time constant over
    /// there rather than a protocol guarantee, so it is configurable here.
    #[serde(default = "default_osc_prefix")]
    pub osc_prefix: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub credentials: InstanceCredentials,
    /// Whether to poll this instance's HTTP API for state. Turning it off
    /// leaves the instance controllable but dark in the UI — the honest
    /// option for a machine whose control port is not reachable, and better
    /// than a permanently red status light.
    #[serde(default = "default_true")]
    pub poll: bool,
    /// Found by the subnet probe rather than typed in. Informational only.
    #[serde(default)]
    pub discovered: bool,
}

fn default_osc_port() -> u16 {
    DEFAULT_OSC_PORT
}
fn default_http_port() -> u16 {
    DEFAULT_HTTP_PORT
}
fn default_osc_prefix() -> String {
    DEFAULT_OSC_PREFIX.to_string()
}
fn default_true() -> bool {
    true
}

impl Instance {
    pub fn new(name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: InstanceId::new(),
            name: name.into(),
            host: host.into(),
            osc_port: DEFAULT_OSC_PORT,
            http_port: DEFAULT_HTTP_PORT,
            osc_prefix: DEFAULT_OSC_PREFIX.to_string(),
            tags: Vec::new(),
            credentials: InstanceCredentials::default(),
            poll: true,
            discovered: false,
        }
    }

    pub fn with_tags(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|t| t.to_string()).collect();
        self
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.http_port)
    }

    /// Resolves the OSC destination.
    ///
    /// Done eagerly per send rather than cached: a show machine's address can
    /// change between DHCP leases, and a fleet tool that keeps sending to a
    /// stale address looks exactly like one whose commands are being ignored.
    pub fn osc_target(&self) -> anyhow::Result<SocketAddr> {
        (self.host.as_str(), self.osc_port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve {}:{}", self.host, self.osc_port))
    }

    pub fn redacted(mut self) -> Self {
        if self.credentials.token.is_some() {
            self.credentials.token = Some("********".to_string());
        }
        self
    }

    /// Rejects an instance that could not be reached or addressed. Called
    /// before anything is written to the registry, so a bad entry never gets
    /// persisted and then fails silently at show time.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "name cannot be empty");
        anyhow::ensure!(!self.host.trim().is_empty(), "host cannot be empty");
        anyhow::ensure!(
            !self.host.contains("://"),
            "host is a bare hostname or IP, not a URL — drop the scheme from {:?}",
            self.host
        );
        anyhow::ensure!(self.osc_port != 0, "osc_port cannot be 0");
        anyhow::ensure!(self.http_port != 0, "http_port cannot be 0");
        anyhow::ensure!(
            self.osc_prefix.starts_with('/'),
            "osc_prefix must start with '/', got {:?}",
            self.osc_prefix
        );
        anyhow::ensure!(
            self.tags.iter().all(|t| !t.trim().is_empty()),
            "a tag cannot be empty — an empty tag would create an unnameable group"
        );
        Ok(())
    }
}
