use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;

use crate::handlers::build_state_response;
use crate::state::AppState;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// One-way server->client push. Serialises the current snapshot and sends it
/// only when it differs from the last one, so an idle fleet costs nothing on
/// the wire and a change shows up within a tick.
///
/// The cadence here is the *UI's*, not the poller's — the fleet poller
/// decides how often instances are actually asked. Pushing faster than the
/// poller would just resend identical snapshots.
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut last_sent: Option<String> = None;
    loop {
        let snapshot = match serde_json::to_string(&build_state_response(&state)) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("failed to serialize state snapshot: {e}");
                break;
            }
        };
        if last_sent.as_deref() != Some(snapshot.as_str()) {
            if socket.send(Message::Text(snapshot.clone().into())).await.is_err() {
                break;
            }
            last_sent = Some(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
