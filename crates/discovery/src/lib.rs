//! Finding WebLinked instances on the LAN.
//!
//! Two mechanisms, and both are needed.
//!
//! **A browse for `_weblinked._tcp`.** WebLinked advertises its control API
//! over mDNS from 0.8.0. This is the good path: it finds instances on any
//! port, it costs one multicast question instead of hundreds of HTTP
//! requests, and the TXT record carries two things the HTTP API cannot report
//! at all — the **OSC port** and the **OSC prefix**. Those matter more than
//! they look. rookery sends commands over OSC and gets no reply, so an
//! instance added with the wrong OSC port looks perfectly healthy (its state
//! polls fine over HTTP) and silently drops every cue. Before the
//! advertisement existed there was nothing to do but assume 7655 and show the
//! operator that it was an assumption.
//!
//! **An active subnet probe**, kept, because a browse does not find
//! everything:
//!
//! - an instance older than 0.8.0, which advertises nothing;
//! - one started with `--no-mdns`, or on a network where multicast is
//!   filtered — which is common enough on managed show networks that this is
//!   not a corner case;
//! - one bound to a routed subnet, since mDNS does not cross a router.
//!
//! The probe's signature is WebLinked's own control API on its default port:
//! a `GET /api/state` answers either `200` with a body containing
//! `compiled_backends` — a key no other service on a show network is going to
//! emit — or `401` when the instance was started with `--token`, which is
//! just as identifying and must not be skipped over.
//!
//! Two limits on the probe, both deliberate, both degrading to "add it by
//! hand":
//!
//! - **Only the default port is swept.** Sweeping a port range across a /24
//!   turns a quick scan into thousands of requests. An instance on a custom
//!   port either advertises, or is added manually.
//! - **Only directly-attached subnets, and nothing bigger than a /22.**
//!   Same rule flock uses. A tunnelled or routed instance is added manually.
//!
//! Results from both are merged by `merge()`, with the advertised record
//! winning: it is strictly better informed than a probe result, never worse.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::time::Duration;

use futures::stream::StreamExt;
use serde::Serialize;

/// WebLinked's default HTTP control port.
const WEBLINKED_HTTP_PORT: u16 = 7654;
/// Total addresses one scan will probe, across every interface.
///
/// Sized so a full /22 fits — that is the largest subnet the sweep accepts, and
/// the module docs promise it is swept. At 512 it did not fit, so every /22 was
/// half-swept with nothing logged. Two of them still fit here; a third is
/// skipped whole and said so, rather than being truncated mid-range.
///
/// The cost is time, not traffic: PROBE_CONCURRENCY at PROBE_TIMEOUT works out
/// around 40 addresses a second against hosts that do not answer, so a full /22
/// is roughly 25 s of scanning.
const MAX_HOSTS_TO_PROBE: usize = 2044;
const PROBE_CONCURRENCY: usize = 64;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// A key in `/api/state` that nothing else on a show network emits.
const STATE_SIGNATURE: &str = "compiled_backends";

/// The service type WebLinked advertises under.
const SERVICE_TYPE: &str = "_weblinked._tcp.local.";
/// How long to listen for advertisements. Responders answer a browse in
/// milliseconds; this is long enough to catch a sleepy one and short enough
/// that an operator does not think the button is broken.
const BROWSE_TIMEOUT: Duration = Duration::from_millis(2500);

/// How an instance was found. Surfaced because it changes what is known about
/// it: an advertised instance comes with its OSC port, a swept one does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FoundVia {
    /// It advertised itself. Everything below is what it said about itself.
    Mdns,
    /// Found by probing the subnet. OSC details are unknown, not defaulted.
    Sweep,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredInstance {
    pub host: String,
    pub http_port: u16,
    /// From the state response or the TXT record where it was readable. `None`
    /// when the instance answered `401`.
    pub version: Option<String>,
    /// True when the instance answered `401`, or advertised `token=1` — it is
    /// a WebLinked, and it wants a token before it will say anything else.
    /// Surfaced so the UI can ask for one at the moment of adding rather than
    /// after a red row shows up.
    pub needs_token: bool,
    pub found_via: FoundVia,
    /// The operator-facing name the instance advertises. `None` from a sweep.
    pub name: Option<String>,
    /// **The reason the advertisement is worth having.** `None` means "not
    /// known", which the UI must show as an assumption — never silently as
    /// 7655, because a wrong OSC port produces an instance that polls
    /// perfectly and ignores every command.
    pub osc_port: Option<u16>,
    /// Likewise: a non-default prefix makes every address rookery sends wrong.
    pub osc_prefix: Option<String>,
    /// Stable per instance, so a returning one can be recognised across a
    /// restart or an address change.
    pub id: Option<String>,
    /// Every address the advertisement carried. Not serialised — the UI shows
    /// one address — but kept so a swept duplicate can be suppressed against
    /// all of them, not only the one picked for display.
    #[serde(skip)]
    pub addresses: Vec<String>,
}

impl DiscoveredInstance {
    /// What makes two results the same instance.
    ///
    /// The advertised `id` when there is one, because a multi-homed machine
    /// answers a browse **once per address** — a Mac with Ethernet, a
    /// ZeroTier interface and a couple of virtual-machine bridges resolved to
    /// five entries for one WebLinked, all with the same id. Falling back to
    /// host:port for a swept result keeps two instances on one host distinct,
    /// which is a supported arrangement.
    fn key(&self) -> InstanceKey {
        match &self.id {
            Some(id) => InstanceKey::Id(id.clone()),
            None => InstanceKey::Address(self.host.clone(), self.http_port),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum InstanceKey {
    Id(String),
    Address(String, u16),
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

    /// Browses for advertisements and sweeps the local subnets, merging both.
    ///
    /// Neither half failing is fatal: a machine with no mDNS still gets the
    /// sweep, and a machine whose subnet is too large to sweep still gets
    /// whatever advertised itself.
    pub async fn scan(&self) -> anyhow::Result<Vec<DiscoveredInstance>> {
        let (advertised, swept) = tokio::join!(self.browse(), self.sweep());

        let advertised = advertised.unwrap_or_else(|error| {
            tracing::warn!(%error, "mDNS browse failed; falling back to the subnet sweep");
            Vec::new()
        });
        let swept = swept.unwrap_or_else(|error| {
            tracing::warn!(%error, "subnet sweep failed; using advertisements only");
            Vec::new()
        });

        Ok(merge(advertised, swept))
    }

    /// Listens for `_weblinked._tcp` advertisements.
    async fn browse(&self) -> anyhow::Result<Vec<DiscoveredInstance>> {
        let daemon = mdns_sd::ServiceDaemon::new()?;
        let receiver = daemon.browse(SERVICE_TYPE)?;

        let mut found = Vec::new();
        let deadline = tokio::time::Instant::now() + BROWSE_TIMEOUT;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let event = match tokio::time::timeout(remaining, receiver.recv_async()).await {
                Ok(Ok(event)) => event,
                // Channel closed, or the window expired — either way we are done.
                Ok(Err(_)) | Err(_) => break,
            };

            if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                found.push(from_service_info(&info));
            }
        }

        let _ = daemon.stop_browse(SERVICE_TYPE);
        let _ = daemon.shutdown();

        tracing::info!(count = found.len(), "advertised WebLinked instances");
        Ok(found)
    }

    /// Sweeps every directly-attached subnet for WebLinked's control API.
    async fn sweep(&self) -> anyhow::Result<Vec<DiscoveredInstance>> {
        let candidates = local_ipv4_candidates();
        if candidates.is_empty() {
            tracing::warn!("no sweepable local subnet found — relying on advertisements");
            return Ok(vec![]);
        }
        tracing::info!(hosts = candidates.len(), "probing for WebLinked instances");

        let found = futures::stream::iter(candidates)
            .map(|ip| async move { self.probe_one(ip).await })
            .buffer_unordered(PROBE_CONCURRENCY)
            .filter_map(|r| async move { r })
            .collect::<Vec<_>>()
            .await;

        Ok(found)
    }

    async fn probe_one(&self, ip: Ipv4Addr) -> Option<DiscoveredInstance> {
        let url = format!("http://{ip}:{WEBLINKED_HTTP_PORT}/api/state");
        let response = self.client.get(&url).send().await.ok()?;

        if response.status().as_u16() == 401 {
            // Identifying on its own: a token-protected WebLinked.
            return Some(swept_instance(ip.to_string(), None, true));
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

        Some(swept_instance(ip.to_string(), version, false))
    }
}

/// Combines what advertised itself with what the sweep turned up.
///
/// Two passes, not one, because the halves are keyed differently.
///
/// Advertisements collapse on their `id`: one instance answers a browse **once
/// per interface**, so a machine with Ethernet, a VPN and two virtual-machine
/// bridges resolves to four entries for one WebLinked. A swept result has no
/// id to collapse on, so it is suppressed by address instead — against *every*
/// address the advertisement carried, not only the one picked for display, or
/// the instance reappears under its VPN address as a second row.
fn merge(
    advertised: Vec<DiscoveredInstance>,
    swept: Vec<DiscoveredInstance>,
) -> Vec<DiscoveredInstance> {
    let mut merged: HashMap<InstanceKey, DiscoveredInstance> = HashMap::new();
    let mut advertised_addresses: HashSet<(String, u16)> = HashSet::new();

    for instance in advertised {
        for address in &instance.addresses {
            advertised_addresses.insert((address.clone(), instance.http_port));
        }
        advertised_addresses.insert((instance.host.clone(), instance.http_port));

        match merged.entry(instance.key()) {
            std::collections::hash_map::Entry::Occupied(mut existing) => {
                // Not `or_insert`. Each interface's answer carries only that
                // interface's address, so keeping the first arrival picks an
                // address at random — on a machine with bridges and tunnels
                // that came out as a 127/8 address, which is a row an operator
                // can add and never reach. Pool every address seen for this id,
                // then choose.
                let entry = existing.get_mut();
                entry.addresses.extend(instance.addresses);
                entry.addresses.sort();
                entry.addresses.dedup();
                if let Some(best) = preferred_address(&entry.addresses) {
                    entry.host = best;
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(instance);
            }
        }
    }
    for instance in swept {
        if advertised_addresses.contains(&(instance.host.clone(), instance.http_port)) {
            continue;
        }
        merged.entry(instance.key()).or_insert(instance);
    }

    let mut found: Vec<_> = merged.into_values().collect();
    found.sort_by(|a, b| a.host.cmp(&b.host).then(a.http_port.cmp(&b.http_port)));
    found
}

fn swept_instance(host: String, version: Option<String>, needs_token: bool) -> DiscoveredInstance {
    DiscoveredInstance {
        host,
        http_port: WEBLINKED_HTTP_PORT,
        version,
        needs_token,
        found_via: FoundVia::Sweep,
        // All `None` rather than defaulted. A sweep genuinely does not know
        // these, and writing 7655 in here would turn "we are guessing" into
        // "we were told", which is the distinction the whole OSC-has-no-acks
        // design rests on.
        name: None,
        osc_port: None,
        osc_prefix: None,
        id: None,
        addresses: Vec::new(),
    }
}

/// Reads a resolved advertisement into our shape.
///
/// The address is preferred over the advertised host name: a `.local` name
/// needs the resolver to work on every machine that later talks to it, and a
/// literal address is what an operator can check by hand.
// mdns-sd 0.21 resolves to a ResolvedService of public fields rather than a
// ServiceInfo of getters. Same data: `host` for `get_hostname()`, `port` for
// `get_port()`, `txt_properties` for `get_properties()`, and `addresses` as a
// HashSet<ScopedIp> whose Display is the plain address, so the strings this
// builds are unchanged.
fn from_service_info(info: &mdns_sd::ResolvedService) -> DiscoveredInstance {
    let properties = &info.txt_properties;
    let text = |key: &str| properties.get_property_val_str(key).map(|v| v.to_string());

    let addresses: Vec<String> = info
        .addresses
        .iter()
        .map(|address| address.to_string())
        .collect();

    let host = preferred_address(&addresses)
        .unwrap_or_else(|| info.host.trim_end_matches('.').to_string());

    DiscoveredInstance {
        host,
        http_port: info.port,
        version: text("ver"),
        // The instance says so itself, so the UI can ask for the token while
        // the operator is still looking at the add dialog.
        needs_token: text("token").as_deref() == Some("1"),
        found_via: FoundVia::Mdns,
        name: text("name"),
        // Only when OSC is actually enabled. An instance with `osc=0` has no
        // listener at all, and offering a port for it would be worse than
        // offering nothing.
        osc_port: match text("osc").as_deref() {
            Some("0") => None,
            _ => text("oscport").and_then(|p| p.parse().ok()),
        },
        osc_prefix: text("oscprefix"),
        id: text("id"),
        addresses,
    }
}

/// Picks the address most likely to be reachable from another machine.
///
/// A responder answers on every interface it has, so a browse yields the LAN
/// address, the VPN address, one per virtual-machine bridge, link-local
/// addresses and loopback — with no ordering. Picking arbitrarily produced a
/// `127/8` address on the development Mac, which adds cleanly to the registry
/// and is unreachable from anywhere else. Ties break on the string so the
/// choice is stable between scans rather than flapping between two equally
/// good addresses.
fn preferred_address(addresses: &[String]) -> Option<String> {
    addresses
        .iter()
        .filter_map(|text| text.parse::<std::net::IpAddr>().ok().map(|ip| (ip, text)))
        .min_by_key(|(ip, text)| (address_rank(ip), (*text).clone()))
        .map(|(_, text)| text.clone())
}

fn address_rank(ip: &std::net::IpAddr) -> u8 {
    use std::net::IpAddr;
    match ip {
        // Routable IPv4 first, then private IPv4 — a show network is almost
        // always the latter, but a public address is no less reachable.
        IpAddr::V4(v4) if v4.is_loopback() => 6,
        IpAddr::V4(v4) if v4.is_link_local() => 5,
        IpAddr::V4(v4) if v4.is_private() => 1,
        IpAddr::V4(_) => 0,
        IpAddr::V6(v6) if v6.is_loopback() => 8,
        // fe80::/10. `is_unicast_link_local` is still unstable, so this is the
        // prefix test it would do.
        IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80 => 7,
        IpAddr::V6(_) => 4,
    }
}

/// Every non-loopback IPv4 host address on directly-attached local subnets,
/// skipping any subnet too large to sweep safely. Same rule as flock's
/// `subnet_probe`.
fn local_ipv4_candidates() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let subnets: Vec<(u32, u32)> = interfaces
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .filter_map(|iface| match iface.addr {
            if_addrs::IfAddr::V4(v4) => Some((u32::from(v4.ip), u32::from(v4.netmask))),
            _ => None,
        })
        .collect();
    candidates_for(&subnets)
}

/// The address expansion, split out from interface enumeration so it can be
/// tested against a subnet this machine does not have.
fn candidates_for(subnets: &[(u32, u32)]) -> Vec<Ipv4Addr> {
    let mut candidates = Vec::new();
    for &(ip, mask) in subnets {
        let network = ip & mask;
        let host_bits = 32 - mask.count_ones();
        if !(1..=10).contains(&host_bits) {
            // Bigger than a /22 is too large to sweep quickly; a mask with no
            // host bits is not a LAN segment.
            continue;
        }
        let host_count = 1u32 << host_bits;
        let usable = host_count.saturating_sub(2) as usize;

        // Whole subnet or none of it. Stopping partway through — which is what
        // a running total did — sweeps the bottom half of a /22 and silently
        // never probes the top, so an instance at 10.0.3.x on a 10.0.0.0/22
        // show network is reported as "Nothing found" by a scan that never
        // looked at it. The host_bits check above already bounds one subnet to
        // a /22 (1022 usable), so this only ever skips a LATER interface.
        if candidates.len() + usable > MAX_HOSTS_TO_PROBE {
            tracing::warn!(
                subnet = %Ipv4Addr::from(network),
                host_bits,
                usable,
                probed_so_far = candidates.len(),
                budget = MAX_HOSTS_TO_PROBE,
                "subnet not swept: it does not fit in the remaining probe budget. \
                 Instances on it must be added by hand."
            );
            continue;
        }

        for i in 1..host_count.saturating_sub(1) {
            candidates.push(Ipv4Addr::from(network | i));
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swept_instance_never_claims_to_know_the_osc_port() {
        // The whole reason found_via exists. A sweep cannot see the OSC port,
        // and defaulting it here would present a guess as a fact — the failure
        // is silent, because OSC is fire-and-forget and a wrong port produces
        // an instance that polls green and ignores every cue.
        let instance = swept_instance("192.168.1.40".into(), None, false);
        assert_eq!(instance.found_via, FoundVia::Sweep);
        assert!(instance.osc_port.is_none());
        assert!(instance.osc_prefix.is_none());
    }

    fn advertised(host: &str, id: &str, addresses: &[&str]) -> DiscoveredInstance {
        DiscoveredInstance {
            host: host.into(),
            http_port: 7654,
            version: Some("0.8.0".into()),
            needs_token: false,
            found_via: FoundVia::Mdns,
            name: Some("Stage Left".into()),
            osc_port: Some(9001),
            osc_prefix: Some("/stage".into()),
            id: Some(id.into()),
            addresses: addresses.iter().map(|a| (*a).to_string()).collect(),
        }
    }

    #[test]
    fn merging_prefers_the_advertised_record() {
        // Both halves find the same instance. The advertised one carries
        // strictly more, so it has to win — and the swept duplicate must not
        // survive as a second row.
        let found = merge(
            vec![advertised("192.168.1.40", "deadbeef", &["192.168.1.40"])],
            vec![swept_instance(
                "192.168.1.40".into(),
                Some("0.8.0".into()),
                false,
            )],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].found_via, FoundVia::Mdns);
        assert_eq!(found[0].osc_port, Some(9001));
    }

    #[test]
    fn one_instance_on_a_multi_homed_machine_is_one_entry() {
        // The bug this caught for real: a browse resolves once per interface,
        // so a Mac with a VPN and two VM bridges produced five rows for one
        // WebLinked. The advertised id is what collapses them.
        let found = merge(
            vec![
                advertised("192.168.1.40", "fc0e50e0", &["192.168.1.40"]),
                advertised("10.147.17.93", "fc0e50e0", &["10.147.17.93"]),
                advertised("::1", "fc0e50e0", &["::1"]),
            ],
            vec![],
        );
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn a_swept_duplicate_is_suppressed_by_any_advertised_address() {
        // The sweep finds the instance at its LAN address while the browse
        // chose to display its VPN address. Matching only the displayed one
        // would leave the same machine listed twice.
        let found = merge(
            vec![advertised(
                "10.147.17.93",
                "fc0e50e0",
                &["10.147.17.93", "192.168.1.40"],
            )],
            vec![swept_instance("192.168.1.40".into(), None, false)],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].found_via, FoundVia::Mdns);
    }

    #[test]
    fn the_chosen_address_is_one_another_machine_could_reach() {
        // Found live: the development Mac advertises across bridges, tunnels
        // and loopback, and picking arbitrarily produced `127.158.240.116` —
        // an entry that adds cleanly and is unreachable from anywhere else.
        let addresses: Vec<String> = [
            "127.158.240.116",
            "::1",
            "fe80::842f:57ff:fee5:de64",
            "169.254.1.1",
            "192.168.1.90",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        assert_eq!(
            preferred_address(&addresses).as_deref(),
            Some("192.168.1.90")
        );

        // Loopback is still better than nothing when it is genuinely all there
        // is — a single-machine setup has to remain discoverable.
        assert_eq!(
            preferred_address(&["127.0.0.1".to_string()]).as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(preferred_address(&[]), None);
    }

    #[test]
    fn collapsing_by_id_keeps_the_best_address_not_the_first() {
        // Each interface answers with only its own address, so the first to
        // arrive is arbitrary. Pooling them and re-choosing is what makes the
        // result deterministic *and* reachable.
        let found = merge(
            vec![
                advertised("127.158.240.116", "fc0e50e0", &["127.158.240.116"]),
                advertised("192.168.1.90", "fc0e50e0", &["192.168.1.90"]),
            ],
            vec![],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, "192.168.1.90");
    }

    #[test]
    fn two_instances_on_one_host_are_two_entries() {
        // Supported deliberately in WebLinked — the Chromium profile directory
        // is keyed on the control port — so the merge key must include it.
        let a = swept_instance("192.168.1.40".into(), None, false);
        let mut b = swept_instance("192.168.1.40".into(), None, false);
        b.http_port = 7664;
        assert_ne!(a.key(), b.key());
    }

    /// 10.0.0.0/22 with a WebLinked at 10.0.3.x — the case the module docs
    /// promise is swept, and the one a 512-address running total silently cut
    /// in half.
    #[test]
    fn a_slash_22_is_swept_all_the_way_to_the_top() {
        let subnets = [(u32::from(Ipv4Addr::new(10, 0, 1, 5)), 0xffff_fc00)];
        let candidates = candidates_for(&subnets);

        assert_eq!(candidates.len(), 1022, "a /22 has 1022 usable addresses");
        assert!(candidates.contains(&Ipv4Addr::new(10, 0, 0, 1)));
        assert!(
            candidates.contains(&Ipv4Addr::new(10, 0, 3, 200)),
            "the top quarter must be probed, not dropped"
        );
        assert!(!candidates.contains(&Ipv4Addr::new(10, 0, 0, 0)), "network");
        assert!(
            !candidates.contains(&Ipv4Addr::new(10, 0, 3, 255)),
            "broadcast"
        );
    }

    #[test]
    fn several_interfaces_are_all_swept() {
        let subnets = [
            (u32::from(Ipv4Addr::new(192, 168, 1, 10)), 0xffff_ff00),
            (u32::from(Ipv4Addr::new(192, 168, 2, 10)), 0xffff_ff00),
            (u32::from(Ipv4Addr::new(192, 168, 3, 10)), 0xffff_ff00),
        ];
        let candidates = candidates_for(&subnets);

        assert_eq!(candidates.len(), 254 * 3);
        // The third interface used to be past the budget and never probed.
        assert!(candidates.contains(&Ipv4Addr::new(192, 168, 3, 77)));
    }

    #[test]
    fn a_subnet_that_does_not_fit_is_skipped_whole_not_truncated() {
        // Three /22s: two fit the budget, the third cannot. It must be dropped
        // entirely rather than half-swept, so "not found" never means "found
        // nothing in the part we looked at".
        let subnets = [
            (u32::from(Ipv4Addr::new(10, 0, 0, 5)), 0xffff_fc00),
            (u32::from(Ipv4Addr::new(10, 1, 0, 5)), 0xffff_fc00),
            (u32::from(Ipv4Addr::new(10, 2, 0, 5)), 0xffff_fc00),
        ];
        let candidates = candidates_for(&subnets);

        assert_eq!(candidates.len(), 1022 * 2);
        assert!(candidates.iter().all(|ip| ip.octets()[1] != 2));
    }

    #[test]
    fn a_subnet_bigger_than_a_slash_22_is_not_swept_at_all() {
        let subnets = [(u32::from(Ipv4Addr::new(10, 0, 0, 5)), 0xffff_f800)]; // /21
        assert!(candidates_for(&subnets).is_empty());
    }
}
