//! Fan-out, state polling and northbound OSC dispatch.
//!
//! `Fleet` is the object that turns "do this to the stage group" into N
//! datagrams and a per-instance result, and it owns the background poller
//! that keeps the dashboard populated. It sits between `rookery-core` (which
//! knows what an instance is) and `rookery-web` (which draws it), so the
//! northbound OSC receiver can drive a whole fleet without any of the web
//! layer being involved.

mod dispatch;
mod fanout;
mod target;

pub use dispatch::{parse_northbound, NorthboundCue, DEFAULT_NORTHBOUND_PREFIX};
pub use fanout::{Fanout, FanoutEntry};
pub use target::{Scope, Target};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures::stream::StreamExt;

use rookery_core::client::InstanceClientProvider;
use rookery_core::{Command, Instance, InstanceId, InstanceState, Registry};

/// How many instances to poll at once.
///
/// Each poll is one small HTTP request, so this is about not opening eighty
/// sockets on a show network at the same instant rather than about local
/// cost.
const POLL_CONCURRENCY: usize = 16;

struct Cached {
    state: InstanceState,
    last_ok: Option<Instant>,
    /// `dropped_ticks` per source id at the previous successful poll, so the
    /// next one can tell "is dropping now" from "dropped once, an hour ago".
    /// See `SourceState::dropping` for why the cumulative count is useless on
    /// its own.
    last_dropped: HashMap<String, u64>,
}

impl Cached {
    fn empty() -> Self {
        Self {
            state: InstanceState::default(),
            last_ok: None,
            last_dropped: HashMap::new(),
        }
    }

    /// Marks each source as currently dropping or not, and records the new
    /// counts for the next comparison.
    ///
    /// A counter that goes *down* means the instance restarted, so the
    /// baseline is reset rather than treated as a negative delta.
    fn mark_dropping(&mut self, sources: &mut rookery_core::SourcesState) {
        let mut next = HashMap::new();
        for source in &mut sources.sources {
            let key = source.id.clone().unwrap_or_default();
            let now = source.pacing.dropped_ticks.unwrap_or(0);
            source.dropping = match self.last_dropped.get(&key) {
                Some(&before) => now > before,
                // Nothing to compare against on the first poll. "Unknown" is
                // not an option for a bool, and claiming a fresh instance is
                // degraded on the strength of its startup counter is exactly
                // the false amber this whole mechanism exists to avoid.
                None => false,
            };
            next.insert(key, now);
        }
        self.last_dropped = next;
    }
}

/// Whether the instance's preview output needs rewriting to reach `wanted`.
///
/// False in two cases that both look like "cannot tell", and in both the right
/// move is to leave it alone:
///
/// * **No preview output at all** — an instance started `--no-preview`. Setting
///   a factor on an output that does not exist fails, and doing it once per
///   poll forever is a fault that never surfaces because each attempt is only a
///   warning.
/// * **A preview output that does not report its factor** — an older WebLinked.
///   Writing blind would mean rewriting it on every poll for the life of the
///   process, because nothing would ever confirm it had taken.
///
/// This one was found by a test rather than by reading: the first version
/// asked "what is the factor" and treated `None` as "not what I wanted", which
/// wrote the factor on every single poll.
fn needs_preview_factor(sources: &rookery_core::SourcesState, wanted: u8) -> bool {
    let Some(source) = sources
        .sources
        .iter()
        .find(|s| s.id == sources.primary)
        .or_else(|| sources.sources.first())
    else {
        return false;
    };
    let Some(preview) = source.outputs.iter().find(|o| o.kind == "preview") else {
        return false;
    };
    match preview.factor {
        Some(current) => current != wanted,
        None => false,
    }
}

pub struct Fleet {
    registry: Arc<Registry>,
    provider: Arc<dyn InstanceClientProvider>,
    states: RwLock<HashMap<InstanceId, Cached>>,
}

impl Fleet {
    pub fn new(registry: Arc<Registry>, provider: Arc<dyn InstanceClientProvider>) -> Self {
        Self {
            registry,
            provider,
            states: RwLock::new(HashMap::new()),
        }
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Every instance with whatever rookery currently knows about it.
    pub fn snapshot(&self) -> Vec<(Instance, InstanceState)> {
        let states = self.states.read().expect("fleet lock poisoned");
        self.registry
            .list()
            .into_iter()
            .map(|instance| {
                let state = states
                    .get(&instance.id)
                    .map(|c| {
                        let mut state = c.state.clone();
                        state.age_ms = c.last_ok.map(|t| t.elapsed().as_millis() as u64);
                        state
                    })
                    .unwrap_or(InstanceState {
                        polled: instance.poll,
                        ..Default::default()
                    });
                (instance, state)
            })
            .collect()
    }

    /// The live client for one instance.
    ///
    /// Exposed because preview and input are per-instance and synchronous —
    /// they are not fan-out, and routing them through `send` would mean
    /// inventing a `Command` for something that is not an OSC verb.
    pub fn client_for(&self, instance: &Instance) -> Arc<dyn rookery_core::InstanceClient> {
        self.provider.client_for(instance)
    }

    /// Resolves a target to the instances it currently names.
    pub fn resolve(&self, target: &Target) -> Vec<Instance> {
        match &target.scope {
            Scope::Instance { id } => self.registry.get(id).into_iter().collect(),
            Scope::Group { tag } => self.registry.members_of(tag),
            Scope::All => self.registry.list(),
        }
    }

    /// A human description of a target, for a log line or the UI's activity feed.
    ///
    /// `Target::describe` cannot do this on its own: it lives in `core`, which has no
    /// registry, so a single-instance target can only name its id. A UUID is the
    /// right thing in an address and the wrong thing in front of an operator — it
    /// says nothing about which machine just changed, which is the one fact the line
    /// exists to convey. So the naming happens here, where the registry is.
    pub fn describe_target(&self, target: &Target) -> String {
        let scope = match &target.scope {
            Scope::Instance { id } => match self.registry.get(id) {
                Some(instance) => instance.name,
                // Removed between the request and the log line. Fall back rather
                // than lose the entry.
                None => target.scope.describe(),
            },
            other => other.describe(),
        };
        match &target.source {
            Some(source) => format!("{scope} (source {source})"),
            None => scope,
        }
    }

    /// Sends one command to everything the target names, concurrently.
    ///
    /// An empty result is **not** success. A cue that matched nothing is one
    /// of the most dangerous things a show-control system can do quietly, so
    /// this logs a warning and the callers turn it into a visible failure
    /// (`rookery-web` answers 404; the northbound dispatcher logs it loudly).
    pub async fn send(&self, target: &Target, command: &Command) -> Fanout {
        let instances = self.resolve(target);
        if instances.is_empty() {
            tracing::warn!(
                target = %self.describe_target(target),
                command = %command.summary(),
                "cue matched no instances — nothing was sent"
            );
        }

        let source = target.source.as_deref();
        let entries = futures::stream::iter(instances)
            .map(|instance| async move {
                let client = self.provider.client_for(&instance);
                let result = client.send(command, source).await;
                FanoutEntry {
                    instance_id: instance.id,
                    instance_name: instance.name.clone(),
                    sent: result.is_ok(),
                    error: result.err().map(|e| format!("{e:#}")),
                }
            })
            .buffer_unordered(POLL_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;

        let mut entries = entries;
        entries.sort_by(|a, b| a.instance_name.cmp(&b.instance_name));

        let fanout = Fanout {
            target: self.describe_target(target),
            command: command.summary(),
            entries,
        };
        tracing::info!(
            target = %fanout.target,
            command = %fanout.command,
            "{}",
            fanout.summary()
        );
        fanout
    }

    /// Polls every instance that has polling enabled, concurrently, and
    /// updates the cache.
    ///
    /// A failed poll keeps the previous snapshot and records the error beside
    /// it: "last seen 40 seconds ago, now unreachable" is more use mid-show
    /// than a blank row.
    pub async fn poll_once(&self) {
        let instances = self.registry.list();
        let results = futures::stream::iter(instances)
            .map(|instance| async move {
                if !instance.poll {
                    return (instance.id, None);
                }
                let client = self.provider.client_for(&instance);
                let state = client.state().await;
                // Reconcile the preview factor here rather than at the moment
                // it is set, so it survives the instance restarting — which
                // resets it to whatever its command line said. Only written
                // when it actually differs, so the steady state is no traffic
                // at all.
                if let (Some(wanted), Ok(sources)) = (instance.preview_factor, &state) {
                    if needs_preview_factor(sources, wanted) {
                        if let Err(e) = client.set_preview_factor(wanted).await {
                            tracing::warn!(
                                instance = %instance.name,
                                "could not set the preview factor: {e:#}"
                            );
                        }
                    }
                }
                (instance.id, Some(state))
            })
            .buffer_unordered(POLL_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;

        let mut states = self.states.write().expect("fleet lock poisoned");
        for (id, result) in results {
            let entry = states.entry(id).or_insert_with(Cached::empty);
            match result {
                None => {
                    entry.state = InstanceState {
                        polled: false,
                        ..Default::default()
                    };
                    entry.last_ok = None;
                    entry.last_dropped.clear();
                }
                Some(Ok(mut sources)) => {
                    entry.mark_dropping(&mut sources);
                    entry.state.sources = Some(sources);
                    entry.state.error = None;
                    entry.state.polled = true;
                    entry.last_ok = Some(Instant::now());
                }
                Some(Err(e)) => {
                    entry.state.error = Some(format!("{e:#}"));
                    entry.state.polled = true;
                }
            }
        }

        // An instance removed from the registry should not keep a cached
        // state around to be resurrected if its id is somehow reused.
        let live: std::collections::HashSet<_> =
            self.registry.list().into_iter().map(|i| i.id).collect();
        states.retain(|id, _| live.contains(id));
    }

    /// Spawns the background poller. Returns immediately.
    pub fn spawn_poller(self: &Arc<Self>, interval: Duration) {
        let fleet = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // If a poll overruns the interval, skip the missed ticks rather
            // than queueing them — a slow instance must not turn into a
            // backlog of requests aimed at every other one.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                fleet.poll_once().await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rookery_core::{PacingInfo, SourceState, SourcesState};

    fn sources_with(dropped: u64) -> SourcesState {
        SourcesState {
            primary: Some("main".to_string()),
            sources: vec![SourceState {
                id: Some("main".to_string()),
                running: Some(true),
                pacing: PacingInfo {
                    dropped_ticks: Some(dropped),
                    ..Default::default()
                },
                ..Default::default()
            }],
        }
    }

    /// The rule that came out of running against a real WebLinked: a static
    /// headless instance sits on a large, unchanging dropped-tick count, and
    /// that must read as healthy.
    #[test]
    fn a_steady_dropped_tick_count_is_not_dropping() {
        let mut cached = Cached::empty();

        // First poll: nothing to compare against, so not dropping.
        let mut first = sources_with(346);
        cached.mark_dropping(&mut first);
        assert!(!first.sources[0].dropping);
        assert_eq!(first.sources[0].health(), rookery_core::Health::Ok);

        // Second poll, same count: still healthy.
        let mut second = sources_with(346);
        cached.mark_dropping(&mut second);
        assert!(!second.sources[0].dropping);
        assert_eq!(second.sources[0].health(), rookery_core::Health::Ok);

        // Third poll, the count moved: now it is actually dropping.
        let mut third = sources_with(349);
        cached.mark_dropping(&mut third);
        assert!(third.sources[0].dropping);
        assert_eq!(third.sources[0].health(), rookery_core::Health::Degraded);

        // Fourth poll, steady again: recovered, not stuck amber.
        let mut fourth = sources_with(349);
        cached.mark_dropping(&mut fourth);
        assert!(!fourth.sources[0].dropping);
    }

    /// A restarted instance starts its counters again. A count that went
    /// backwards is a new process, not a negative delta.
    #[test]
    fn a_restarted_instance_does_not_read_as_dropping() {
        let mut cached = Cached::empty();
        let mut before = sources_with(500);
        cached.mark_dropping(&mut before);
        let mut after_restart = sources_with(2);
        cached.mark_dropping(&mut after_restart);
        assert!(!after_restart.sources[0].dropping);
    }

    /// Each pipeline is tracked separately: one source dropping must not
    /// paint its neighbour amber, and vice versa.
    #[test]
    fn sources_are_tracked_independently() {
        let mut cached = Cached::empty();
        let build = |a: u64, b: u64| SourcesState {
            primary: Some("main".to_string()),
            sources: vec![
                SourceState {
                    id: Some("main".to_string()),
                    running: Some(true),
                    pacing: PacingInfo {
                        dropped_ticks: Some(a),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                SourceState {
                    id: Some("lower-third".to_string()),
                    running: Some(true),
                    pacing: PacingInfo {
                        dropped_ticks: Some(b),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
        };

        let mut first = build(10, 10);
        cached.mark_dropping(&mut first);
        let mut second = build(10, 14);
        cached.mark_dropping(&mut second);

        assert!(!second.sources[0].dropping, "main was steady");
        assert!(second.sources[1].dropping, "lower-third moved");
    }
}
