//! Building the router, which is the one thing nothing else here does.
//!
//! `Router::route` validates each path and `panic_on_err!`s the result, so a
//! malformed route is a panic at construction rather than a compile error or a
//! 404. `main` calls `rookery_web::app(state)` unconditionally and before it
//! binds the listener, so any such panic means the binary cannot start at all —
//! not "that endpoint is broken", but "rookery does not run".
//!
//! v0.2.1 shipped exactly that: the axum 0.8 upgrade converted four routes from
//! `:id` to `{id}` and missed the fifth, on the one `.route(` call that spanned
//! several lines. Every published v0.2.1 artefact panics on launch. Nothing
//! caught it because this crate had no tests, and neither clippy nor a type
//! check can see a runtime panic.
//!
//! Constructing the router is therefore the assertion. It needs no server, no
//! socket and no fixture beyond an empty registry.

use std::sync::Arc;

use rookery_core::Registry;
use rookery_discovery::Discovery;
use rookery_fleet::Fleet;
use rookery_instance_live::LiveClientProvider;
use rookery_web::{app, AppState};

fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rookery-web-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn state() -> AppState {
    let registry = Registry::load_or_new(tempdir().join("registry.json")).unwrap();
    let provider = Arc::new(LiveClientProvider::new().await.unwrap());
    AppState {
        fleet: Arc::new(Fleet::new(Arc::new(registry), provider)),
        discovery: Arc::new(Discovery::new().unwrap()),
        northbound: None,
        northbound_prefix: "/rookery".to_string(),
    }
}

#[tokio::test]
async fn the_router_builds() {
    // Panics on any malformed path. That is the whole test.
    let _ = app(state().await);
}

/// The specific shape that shipped broken: axum 0.8 rejects a `:name` segment
/// outright, so this pins the syntax rather than trusting the line above to
/// keep covering it as routes are added.
#[test]
fn no_route_path_uses_axum_07_capture_syntax() {
    let source = include_str!("../src/lib.rs");
    let offenders: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('"') && line.contains("/:"))
        .collect();
    assert!(
        offenders.is_empty(),
        "axum 0.8 capture groups are {{name}}, not :name — found {offenders:?}"
    );
}
