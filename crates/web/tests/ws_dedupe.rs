//! The `/ws` push is supposed to send only when the fleet has changed.
//!
//! It did not. `Fleet::snapshot` recomputes `age_ms` on every read, so any
//! fleet that has ever polled successfully serialised to a different byte
//! string every tick and the `last_sent != snapshot` comparison never matched:
//! every client got the whole snapshot twice a second, forever, which is the
//! opposite of the documented "an idle fleet costs nothing on the wire".
//!
//! This drives the real router over a real socket and counts frames.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rookery_core::{Instance, Registry};
use rookery_discovery::Discovery;
use rookery_fleet::Fleet;
use rookery_instance_live::LiveClientProvider;
use rookery_instance_mock::MockInstance;
use rookery_web::{app, AppState};

fn tempdir() -> std::path::PathBuf {
    // Counter as well as clock: SystemTime on macOS ticks in microseconds, so
    // parallel tests sharing one directory share one registry.json.
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rookery-ws-test-{}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An idle fleet must not be pushed the whole snapshot twice a second.
///
/// The window is deliberately shorter than the 5 s keepalive, so the only
/// frames that can arrive are the initial one and any spurious repeats.
#[tokio::test]
async fn an_idle_fleet_is_not_pushed_every_tick() {
    // A real instance that really polls: age_ms is only populated once a poll
    // has succeeded, and it is age_ms that used to defeat the dedupe. An empty
    // fleet would pass this test even with the bug present.
    let registry = Registry::load_or_new(tempdir().join("registry.json")).unwrap();
    let mock = MockInstance::start().await.unwrap();
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    registry.upsert(instance).unwrap();

    let provider = Arc::new(LiveClientProvider::new().await.unwrap());
    let fleet = Arc::new(Fleet::new(Arc::new(registry), provider));
    fleet.poll_once().await;
    assert!(
        fleet
            .snapshot()
            .iter()
            .any(|(_, s)| s.age_ms.is_some() && s.error.is_none()),
        "the fixture must have polled cleanly, or age_ms never ticks and this \
         test proves nothing"
    );

    let state = AppState {
        fleet,
        discovery: Arc::new(Discovery::new().unwrap()),
        northbound: None,
        northbound_prefix: "/rookery".to_string(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app(state)).await.unwrap();
    });

    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("websocket connect");

    // Four ticks of the 500 ms loop. Pre-fix this was ~4 frames; it must now be
    // the single initial snapshot.
    let mut frames = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2200);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))) => frames += 1,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("websocket error: {e}"),
            Ok(None) => break,
            Err(_) => break, // deadline
        }
    }
    drop(socket);

    assert_eq!(
        frames, 1,
        "an idle fleet should get one snapshot, not one per 500 ms tick"
    );
}
