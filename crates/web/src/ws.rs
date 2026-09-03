use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;

use crate::handlers::build_state_response;
use crate::state::AppState;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// How often an unchanged snapshot is resent anyway, so `age_ms` in the last
/// frame a client holds cannot drift arbitrarily far from the truth.
const KEEPALIVE: Duration = Duration::from_secs(5);

/// One-way server->client push, on the *UI's* cadence rather than the poller's
/// — the fleet poller decides how often instances are actually asked, and
/// pushing faster than it would only resend identical snapshots.
///
/// A frame goes out when the fleet has actually changed, plus one every
/// `KEEPALIVE` regardless. The dedupe deliberately ignores `age_ms`: it is
/// milliseconds-since-last-poll, recomputed in `Fleet::snapshot` on every read,
/// so any fleet that has ever polled successfully produces a different byte
/// string every single tick and the comparison never matched. That is why the
/// old claim that an idle fleet cost nothing on the wire was false — it was
/// sending the whole snapshot, thumbnails and all, twice a second forever.
///
/// Excluding it is safe because `age_ms` is derived, not state: nothing that
/// makes an instance interesting (its sources, its error, whether it is polled)
/// lives in it, so a change worth seeing always moves some other field. The
/// keepalive is there for readers of `/ws` that do show staleness — the shipped
/// control page does not.
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut last_key: Option<String> = None;
    let mut last_sent_at = tokio::time::Instant::now();
    loop {
        let mut response = build_state_response(&state);

        // The comparison key: this snapshot with the one live-clock field
        // flattened out. Taken by stashing and restoring the field rather than
        // cloning the whole response, which carries every instance's state.
        let ages: Vec<Option<u64>> = response
            .instances
            .iter_mut()
            .map(|view| view.state.age_ms.take())
            .collect();
        let key = match serde_json::to_string(&response) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("failed to serialize state snapshot: {e}");
                break;
            }
        };
        for (view, age) in response.instances.iter_mut().zip(ages) {
            view.state.age_ms = age;
        }

        let changed = last_key.as_deref() != Some(key.as_str());
        if changed || last_sent_at.elapsed() >= KEEPALIVE {
            let snapshot = match serde_json::to_string(&response) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("failed to serialize state snapshot: {e}");
                    break;
                }
            };
            if socket.send(Message::Text(snapshot.into())).await.is_err() {
                break;
            }
            last_key = Some(key);
            last_sent_at = tokio::time::Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
