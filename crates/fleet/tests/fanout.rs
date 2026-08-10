//! Fan-out across a simulated fleet, over real sockets.

use std::sync::Arc;
use std::time::Duration;

use rookery_core::{Command, Instance, Registry};
use rookery_fleet::{Fleet, Scope, Target};
use rookery_instance_live::LiveClientProvider;
use rookery_instance_mock::MockInstance;

const SETTLE: Duration = Duration::from_secs(2);

fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rookery-fleet-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Three simulated instances: two tagged `stage`, one tagged `lobby`.
async fn fleet_of_three() -> (Arc<Fleet>, Vec<MockInstance>) {
    let registry = Registry::load_or_new(tempdir().join("registry.json")).unwrap();
    let mut mocks = Vec::new();
    for (name, tag) in [("gfx-1", "stage"), ("gfx-2", "stage"), ("gfx-3", "lobby")] {
        let mock = MockInstance::start().await.unwrap();
        let mut instance = Instance::new(name, "127.0.0.1").with_tags(&[tag]);
        instance.osc_port = mock.osc_port();
        instance.http_port = mock.http_port();
        registry.upsert(instance).unwrap();
        mocks.push(mock);
    }
    let provider = Arc::new(LiveClientProvider::new().await.unwrap());
    (Arc::new(Fleet::new(Arc::new(registry), provider)), mocks)
}

#[tokio::test]
async fn a_group_cue_reaches_its_members_and_nobody_else() {
    let (fleet, mocks) = fleet_of_three().await;

    let fanout = fleet
        .send(
            &Target::group("stage"),
            &Command::Url {
                url: "https://example.com/stage".to_string(),
            },
        )
        .await;

    assert_eq!(fanout.entries.len(), 2);
    assert!(fanout.fully_sent(), "{}", fanout.summary());

    assert!(mocks[0].wait_for_messages(1, SETTLE).await);
    assert!(mocks[1].wait_for_messages(1, SETTLE).await);
    assert_eq!(
        mocks[2].journal().messages.len(),
        0,
        "the lobby instance should not have been touched"
    );

    for mock in &mocks[..2] {
        assert_eq!(
            mock.state().sources[0].source.loaded_url.as_deref(),
            Some("https://example.com/stage")
        );
    }
}

#[tokio::test]
async fn an_all_cue_reaches_everything() {
    let (fleet, mocks) = fleet_of_three().await;
    let fanout = fleet
        .send(&Target::all(), &Command::Reload { ignore_cache: true })
        .await;

    assert_eq!(fanout.entries.len(), 3);
    assert!(fanout.fully_sent());
    for mock in &mocks {
        assert!(mock.wait_for_messages(1, SETTLE).await);
        assert_eq!(mock.journal().reloads, 1);
    }
}

/// A cue naming a tag nobody carries must not read as success. This is the
/// failure mode the whole project is built to avoid: an operator presses a
/// button, nothing happens, and nothing says so.
#[tokio::test]
async fn a_cue_matching_no_instances_is_not_a_success() {
    let (fleet, _mocks) = fleet_of_three().await;
    let fanout = fleet
        .send(
            &Target::group("nonexistent"),
            &Command::Reload {
                ignore_cache: false,
            },
        )
        .await;

    assert!(fanout.entries.is_empty());
    assert!(
        !fanout.fully_sent(),
        "an empty fan-out must never report full success"
    );
    assert!(fanout.summary().contains("matched no instances"));
}

/// One bad host among several must not stop the others. Partial success is
/// the normal case in a fleet.
#[tokio::test]
async fn one_unreachable_instance_does_not_block_the_rest() {
    let registry = Registry::load_or_new(tempdir().join("registry.json")).unwrap();
    let good = MockInstance::start().await.unwrap();

    let mut ok = Instance::new("gfx-good", "127.0.0.1").with_tags(&["stage"]);
    ok.osc_port = good.osc_port();
    ok.http_port = good.http_port();
    registry.upsert(ok).unwrap();

    // A hostname that cannot resolve — the shape of a typo'd entry.
    let bad = Instance::new("gfx-bad", "no-such-host.invalid").with_tags(&["stage"]);
    registry.upsert(bad).unwrap();

    let provider = Arc::new(LiveClientProvider::new().await.unwrap());
    let fleet = Fleet::new(Arc::new(registry), provider);

    let fanout = fleet
        .send(
            &Target::group("stage"),
            &Command::Reload {
                ignore_cache: false,
            },
        )
        .await;

    assert_eq!(fanout.entries.len(), 2);
    assert_eq!(fanout.sent_count(), 1);
    assert_eq!(fanout.failed_count(), 1);
    assert!(!fanout.fully_sent());

    let failed = fanout.entries.iter().find(|e| !e.sent).unwrap();
    assert_eq!(failed.instance_name, "gfx-bad");
    assert!(failed.error.is_some());

    assert!(good.wait_for_messages(1, SETTLE).await);
}

#[tokio::test]
async fn polling_populates_the_snapshot_and_survives_one_bad_instance() {
    let (fleet, _mocks) = fleet_of_three().await;

    // A fourth instance whose HTTP side is dead.
    let dead = Instance::new("gfx-dead", "127.0.0.1");
    let mut dead = dead;
    dead.http_port = 1;
    fleet.registry().upsert(dead).unwrap();

    fleet.poll_once().await;

    let snapshot = fleet.snapshot();
    assert_eq!(snapshot.len(), 4);
    for (instance, state) in &snapshot {
        if instance.name == "gfx-dead" {
            assert!(state.error.is_some(), "a dead instance should record why");
            assert!(state.sources.is_none());
        } else {
            assert!(
                state.sources.is_some(),
                "{} should have polled cleanly",
                instance.name
            );
            assert_eq!(state.error, None);
            assert_eq!(state.health(), rookery_core::Health::Ok);
        }
    }
}

/// A failed poll must not wipe what was last known. "Last seen 40s ago, now
/// unreachable" beats a blank row when something is on air.
#[tokio::test]
async fn a_failed_poll_keeps_the_previous_snapshot_beside_the_error() {
    let registry = Registry::load_or_new(tempdir().join("registry.json")).unwrap();
    let mock = MockInstance::start().await.unwrap();
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    let id = instance.id;
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    registry.upsert(instance.clone()).unwrap();

    let registry = Arc::new(registry);
    let provider = Arc::new(LiveClientProvider::new().await.unwrap());
    let fleet = Fleet::new(registry.clone(), provider);

    fleet.poll_once().await;
    assert!(fleet.snapshot()[0].1.sources.is_some());

    // Repoint it at a dead port and poll again.
    let mut broken = registry.get(&id).unwrap();
    broken.http_port = 1;
    registry.upsert(broken).unwrap();
    fleet.poll_once().await;

    let (_, state) = &fleet.snapshot()[0];
    assert!(state.error.is_some(), "the failure must be recorded");
    assert!(
        state.sources.is_some(),
        "the last good snapshot must be kept, not wiped"
    );
}

/// A fleet whose instances each run two pipelines, as `--config` produces.
async fn fleet_with_two_sources() -> (Arc<Fleet>, Vec<MockInstance>) {
    let registry = Registry::load_or_new(tempdir().join("registry.json")).unwrap();
    let mut mocks = Vec::new();
    for name in ["gfx-1", "gfx-2"] {
        let mut state = rookery_instance_mock::default_state();
        let mut second = state.sources[0].clone();
        second.id = Some("lower-third".to_string());
        second.source.url = Some("https://example.com/original".to_string());
        state.sources.push(second);

        let mock = MockInstance::start_with(state, None).await.unwrap();
        let mut instance = Instance::new(name, "127.0.0.1").with_tags(&["stage"]);
        instance.osc_port = mock.osc_port();
        instance.http_port = mock.http_port();
        registry.upsert(instance).unwrap();
        mocks.push(mock);
    }
    let provider = Arc::new(LiveClientProvider::new().await.unwrap());
    (Arc::new(Fleet::new(Arc::new(registry), provider)), mocks)
}

#[tokio::test]
async fn a_targets_source_selector_survives_fan_out() {
    let (fleet, mocks) = fleet_with_two_sources().await;

    let fanout = fleet
        .send(
            &Target {
                scope: Scope::Group {
                    tag: "stage".to_string(),
                },
                source: Some("lower-third".to_string()),
            },
            &Command::Url {
                url: "https://example.com/changed".to_string(),
            },
        )
        .await;
    assert_eq!(fanout.entries.len(), 2);
    assert!(fanout.fully_sent());

    for mock in &mocks {
        assert!(mock.wait_for_messages(1, SETTLE).await);
        assert_eq!(
            mock.journal().messages[0].address,
            "/weblinked/source/lower-third/url"
        );
        assert_eq!(mock.journal().rejected, 0);

        let state = mock.state();
        let named = state
            .sources
            .iter()
            .find(|s| s.id.as_deref() == Some("lower-third"))
            .unwrap();
        let primary = state
            .sources
            .iter()
            .find(|s| s.id.as_deref() == Some("main"))
            .unwrap();
        assert_eq!(
            named.source.loaded_url.as_deref(),
            Some("https://example.com/changed")
        );
        assert_ne!(
            primary.source.loaded_url.as_deref(),
            Some("https://example.com/changed"),
            "the group cue leaked into the primary pipeline"
        );
    }
}

/// Naming a pipeline that does not exist on a given instance is a real
/// hazard of fanning out by group: the machines need not be configured
/// identically. WebLinked logs `no source called '<id>'` and changes nothing,
/// and the send still reports as sent — because it *was* sent. rookery cannot
/// tell the difference from the OSC side, which is exactly why the dashboard
/// polls state rather than trusting the fan-out result.
#[tokio::test]
async fn an_unknown_source_id_is_dropped_by_the_instance_not_by_the_send() {
    let (fleet, mocks) = fleet_of_three().await;

    let fanout = fleet
        .send(
            &Target::group("stage").with_source(Some("no-such-pipeline".to_string())),
            &Command::Reload {
                ignore_cache: false,
            },
        )
        .await;

    // The send succeeded — the datagram left the machine.
    assert!(fanout.fully_sent());

    // …and the instance dropped it on the floor.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(mocks[0].journal().messages.len(), 0);
    assert_eq!(mocks[0].journal().reloads, 0);
    assert!(mocks[0].journal().rejected > 0);
}

/// A UUID is the right thing in an OSC address and the wrong thing in an
/// operator-facing log line: it says nothing about which machine just changed.
#[tokio::test]
async fn a_fanout_names_the_instance_rather_than_its_uuid() {
    let (fleet, _mocks) = fleet_of_three().await;
    let id = fleet
        .registry()
        .list()
        .into_iter()
        .find(|i| i.name == "gfx-2")
        .unwrap()
        .id;

    let fanout = fleet
        .send(
            &Target::instance(id),
            &Command::Reload {
                ignore_cache: false,
            },
        )
        .await;

    assert_eq!(fanout.target, "gfx-2");
    assert!(
        !fanout.target.contains(&id.to_string()),
        "the uuid leaked into the operator-facing description: {}",
        fanout.target
    );

    // A pipeline selector still shows, because it changes what was addressed.
    let fanout = fleet
        .send(
            &Target::instance(id).with_source(Some("lower-third".to_string())),
            &Command::Reload {
                ignore_cache: false,
            },
        )
        .await;
    assert_eq!(fanout.target, "gfx-2 (source lower-third)");

    // Groups and all were already named; check they did not regress.
    let fanout = fleet
        .send(
            &Target::group("stage"),
            &Command::Reload {
                ignore_cache: false,
            },
        )
        .await;
    assert_eq!(fanout.target, "group \"stage\"");
}

/// An instance removed between the send and the log line must not lose the
/// entry — falling back to the id beats dropping it.
#[tokio::test]
async fn a_removed_instance_falls_back_to_its_id() {
    let (fleet, _mocks) = fleet_of_three().await;
    let id = rookery_core::InstanceId::new();
    assert_eq!(
        fleet.describe_target(&Target::instance(id)),
        format!("instance {id}")
    );
}

/// The preview factor is reconciled by the poller rather than at the moment it
/// is set, so it survives the instance restarting — which resets it to whatever
/// its command line said.
#[tokio::test]
async fn the_poller_reconciles_the_preview_factor_and_then_leaves_it_alone() {
    let (fleet, mocks) = fleet_of_three().await;

    // Nobody has asked for a factor, so nothing should be written.
    fleet.poll_once().await;
    assert!(
        mocks.iter().all(|m| m.journal().preview_factors.is_empty()),
        "an instance with no preview_factor must not be reconfigured"
    );

    let mut gfx1 = fleet
        .registry()
        .list()
        .into_iter()
        .find(|i| i.name == "gfx-1")
        .unwrap();
    gfx1.preview_factor = Some(8);
    fleet.registry().upsert(gfx1).unwrap();

    fleet.poll_once().await;
    assert_eq!(mocks[0].journal().preview_factors, vec![8]);
    assert!(mocks[1].journal().preview_factors.is_empty());

    // Already at 8 now, so a second pass writes nothing: the steady state is
    // no traffic, not one request per poll for ever.
    fleet.poll_once().await;
    fleet.poll_once().await;
    assert_eq!(
        mocks[0].journal().preview_factors,
        vec![8],
        "the factor was rewritten on a poll where it already matched"
    );
}
