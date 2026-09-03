mod error;
mod handlers;
mod preview;
mod state;
mod static_assets;
mod ws;

pub use state::AppState;

use axum::routing::{get, post, put};
use axum::Router;

/// The full rookery router: static frontend, REST API, live-push websocket.
///
/// No authentication layer, deliberately and visibly — see the README's
/// Security section. rookery inherits the trust model of the thing it drives:
/// WebLinked's own OSC listener has no authentication either, so anything
/// that can reach the show network can already change what is on air. Putting
/// a login on rookery alone would look like protection it does not provide.
/// Bind it to an interface only a trusted network can reach.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(static_assets::index))
        .route("/app.js", get(static_assets::app_js))
        .route("/style.css", get(static_assets::style_css))
        .route("/health", get(|| async { "ok" }))
        .route("/api/state", get(handlers::get_state))
        .route("/ws", get(ws::ws_handler))
        .route("/api/instances", post(handlers::create_instance))
        .route(
            "/api/instances/{id}",
            put(handlers::update_instance).delete(handlers::delete_instance),
        )
        .route("/api/instances/{id}/send", post(handlers::send_to_instance))
        .route("/api/instances/{id}/preview", get(preview::get_preview))
        .route("/api/instances/{id}/input", post(preview::post_input))
        .route("/api/groups/{tag}/send", post(handlers::send_to_group))
        .route("/api/all/send", post(handlers::send_to_all))
        .route("/api/resolve", get(handlers::resolve_target))
        .route("/api/discovery/scan", get(handlers::scan_discovery))
        .with_state(state)
}
