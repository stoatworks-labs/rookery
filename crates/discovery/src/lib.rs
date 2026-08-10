//! Finding WebLinked instances on the LAN.
//!
//! **There is no service advertisement to browse.** WebLinked does not
//! register itself over mDNS — its NDI *outputs* do, but that is the video
//! leaving the machine, not the control surface, and a `_ndi._tcp` record
//! tells you nothing about which host runs a WebLinked or on which port its
//! API is listening. (A machine can also run WebLinked with no NDI output at
//! all: SDI-only, or screen-only.) So, like flock before it, this is an
//! active subnet probe rather than a browse.
//!
//! The signature is WebLinked's own control API on its default port: a `GET
//! /api/state` answers either `200` with a body containing
//! `compiled_backends` — a key no other service on a show network is going to
//! emit — or `401` when the instance was started with `--token`, which is
//! just as identifying and must not be skipped over.
//!
//! Two limits, both deliberate, both degrading to "add it by hand":
//!
//! - **Only the default port is swept.** Sweeping a port range across a /24
//!   turns a quick scan into thousands of requests. An instance on a custom
//!   port is added manually.
//! - **Only directly-attached subnets, and nothing bigger than a /22.**
//!   Same rule flock uses. A tunnelled or routed instance is added manually.
//!
//! An instance found here still has to have its OSC port confirmed by the
//! operator: nothing in the HTTP response says which UDP port the OSC
//! listener is on, so the default is assumed and shown as an assumption.

use std::net::Ipv4Addr;
use std::time::Duration;

use futures::stream::StreamExt;
use serde::Serialize;

/// WebLinked's default HTTP control port.
const WEBLINKED_HTTP_PORT: u16 = 7654;
const MAX_HOSTS_TO_PROBE: usize = 512;
const PROBE_CONCURRENCY: usize = 64;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// A key in `/api/state` that nothing else on a show network emits.
const STATE_SIGNATURE: &str = "compiled_backends";

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredInstance {
    pub host: String,
    pub http_port: u16,
    /// From the state response where it was readable. `None` when the
    /// instance answered `401`.
    pub version: Option<String>,
    /// True when the instance answered `401` — it is a WebLinked, and it
    /// wants a token before it will say anything else. Surfaced so the UI can
    /// ask for one at the moment of adding rather than after a red row shows
    /// up.
    pub needs_token: bool,
}

pub struct Discovery {
    client: reqwest::Client,
}

impl Discovery {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(PROBE_TIMEOUT).build()?,
        })
    }

    /// Sweeps every directly-attached subnet for WebLinked's control API.
    pub async fn scan(&self) -> anyhow::Result<Vec<DiscoveredInstance>> {
        let candidates = local_ipv4_candidates();
        if candidates.is_empty() {
            tracing::warn!("no sweepable local subnet found — add instances by hand");
            return Ok(vec![]);
        }
        tracing::info!(hosts = candidates.len(), "probing for WebLinked instances");

        let mut found = futures::stream::iter(candidates)
            .map(|ip| async move { self.probe_one(ip).await })
            .buffer_unordered(PROBE_CONCURRENCY)
            .filter_map(|r| async move { r })
            .collect::<Vec<_>>()
            .await;

        found.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(found)
    }

    async fn probe_one(&self, ip: Ipv4Addr) -> Option<DiscoveredInstance> {
        let url = format!("http://{ip}:{WEBLINKED_HTTP_PORT}/api/state");
        let response = self.client.get(&url).send().await.ok()?;

        if response.status().as_u16() == 401 {
            // Identifying on its own: a token-protected WebLinked.
            return Some(DiscoveredInstance {
                host: ip.to_string(),
                http_port: WEBLINKED_HTTP_PORT,
                version: None,
                needs_token: true,
            });
        }
        if !response.status().is_success() {
            return None;
        }

        let body = response.text().await.ok()?;
        if !body.contains(STATE_SIGNATURE) {
            return None;
        }
        let version = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("version")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            });

        Some(DiscoveredInstance {
            host: ip.to_string(),
            http_port: WEBLINKED_HTTP_PORT,
            version,
            needs_token: false,
        })
    }
}

/// Every non-loopback IPv4 host address on directly-attached local subnets,
/// skipping any subnet too large to sweep safely. Same rule as flock's
/// `subnet_probe`.
fn local_ipv4_candidates() -> Vec<Ipv4Addr> {
    let mut candidates = Vec::new();
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return candidates;
    };
    for iface in interfaces {
        if iface.is_loopback() {
            continue;
        }
        let if_addrs::IfAddr::V4(v4) = iface.addr else {
            continue;
        };
        let ip = u32::from(v4.ip);
        let mask = u32::from(v4.netmask);
        let network = ip & mask;
        let host_bits = 32 - mask.count_ones();
        if !(1..=10).contains(&host_bits) {
            // Bigger than a /22 is too large to sweep quickly; a mask with no
            // host bits is not a LAN segment.
            continue;
        }
        let host_count = 1u32 << host_bits;
        for i in 1..host_count.saturating_sub(1) {
            candidates.push(Ipv4Addr::from(network | i));
            if candidates.len() >= MAX_HOSTS_TO_PROBE {
                return candidates;
            }
        }
    }
    candidates
}
