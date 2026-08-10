//! A simulated WebLinked instance, on real sockets.
//!
//! This is not a stub that stands in for the transport — it is a second
//! implementation of the *far* end. It binds a real UDP port and decodes real
//! OSC datagrams, and serves real HTTP on a real TCP port. So a test using it
//! exercises `rookery-instance-live` in full: the encoder, the socket, the
//! address grammar, the JSON shape. Nothing is bypassed.
//!
//! That is worth the extra machinery for one reason: the bugs this project is
//! most exposed to are wire-format bugs, and an in-process mock that takes a
//! `Command` and returns a `SourceState` cannot catch a single one of them.
//! WebLinked's own OSC padding bug would have passed such a mock perfectly.
//!
//! It is still a simulation, and the honesty rule applies: **passing against
//! the mock is not evidence that anything works against WebLinked.** It shows
//! that rookery agrees with rookery's reading of WebLinked's protocol. Only
//! running against the real binary tests the reading itself — see
//! `docs/verification.md`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::UdpSocket;

use rookery_core::{InputEvent, OutputInfo, SourceState, SourcesState};
use rookery_osc::{decode_packet, Message};

/// Everything the simulated instance has been told to do, so a test can
/// assert on the commands themselves and not only on their effect.
#[derive(Debug, Default, Clone)]
pub struct Journal {
    pub messages: Vec<Message>,
    pub reloads: u32,
    pub scripts: Vec<String>,
    /// Datagrams that arrived and could not be decoded, or whose address did
    /// not match the prefix. A non-zero count here with an empty `messages`
    /// is the signature of a wire-format fault.
    pub rejected: u32,
    /// Every input event delivered to `/api/input`, in order.
    pub input: Vec<InputEvent>,
    /// Preview factors the client has asked for, in order.
    pub preview_factors: Vec<u8>,
}

struct Inner {
    state: SourcesState,
    journal: Journal,
    token: Option<String>,
    prefix: String,
    /// How the simulated preview behaves. `None` stands in for an instance
    /// started `--no-preview`, which is a 404 rather than an error.
    preview: Option<PreviewSim>,
}

/// A synthetic preview: a real BGRA buffer of the right size, with a sequence
/// that advances on every fetch so change-detection has something to detect.
struct PreviewSim {
    factor: u8,
    sequence: i64,
    /// Frames served before the first one is ready, standing in for the 503 a
    /// real instance gives between start-up and its first paint.
    warmup: u32,
}

impl PreviewSim {
    fn dimensions(&self) -> (u32, u32) {
        (1920 / self.factor as u32, 1080 / self.factor as u32)
    }
}

#[derive(Clone)]
pub struct MockInstance {
    inner: Arc<Mutex<Inner>>,
    osc_port: u16,
    http_port: u16,
}

impl MockInstance {
    /// Starts a simulated instance with one source called `main` carrying an
    /// NDI output called `Graphic`, on two ephemeral ports.
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with(default_state(), None).await
    }

    /// Starts one that requires `token` on its HTTP API — the shape of a
    /// WebLinked launched with `--token`.
    pub async fn start_with_token(token: &str) -> anyhow::Result<Self> {
        Self::start_with(default_state(), Some(token.to_string())).await
    }

    pub async fn start_with(state: SourcesState, token: Option<String>) -> anyhow::Result<Self> {
        let inner = Arc::new(Mutex::new(Inner {
            state,
            journal: Journal::default(),
            token,
            prefix: "/weblinked".to_string(),
            preview: Some(PreviewSim {
                factor: 1,
                sequence: 0,
                warmup: 0,
            }),
        }));

        // Loopback only. A test fixture that answers on every interface is a
        // way to accidentally drive a real machine on the same network.
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let osc_port = socket.local_addr()?.port();
        {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 65536];
                loop {
                    let Ok((len, _from)) = socket.recv_from(&mut buffer).await else {
                        break;
                    };
                    let mut messages = Vec::new();
                    decode_packet(&buffer[..len], &mut |m| messages.push(m));
                    let mut guard = inner.lock().expect("mock lock poisoned");
                    if messages.is_empty() {
                        guard.journal.rejected += 1;
                        continue;
                    }
                    for message in messages {
                        guard.apply(message);
                    }
                }
            });
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let http_port = listener.local_addr()?.port();
        {
            let inner = inner.clone();
            let app = Router::new()
                .route("/api/sources", get(get_sources))
                .route("/api/state", get(get_state))
                .route("/api/preview", get(get_preview))
                .route("/api/input", post(post_input))
                .route("/api/output/update", post(post_output_update))
                .with_state(inner);
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
        }

        Ok(Self {
            inner,
            osc_port,
            http_port,
        })
    }

    pub fn osc_port(&self) -> u16 {
        self.osc_port
    }

    pub fn http_port(&self) -> u16 {
        self.http_port
    }

    pub fn osc_addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.osc_port))
    }

    /// Turns the simulated preview off, standing in for `--no-preview`.
    pub fn without_preview(self) -> Self {
        self.inner.lock().expect("mock lock poisoned").preview = None;
        self
    }

    /// Makes the next `n` preview fetches answer 503, standing in for an
    /// instance that is up but has not painted yet.
    pub fn with_preview_warmup(self, n: u32) -> Self {
        if let Some(preview) = self
            .inner
            .lock()
            .expect("mock lock poisoned")
            .preview
            .as_mut()
        {
            preview.warmup = n;
        }
        self
    }

    pub fn journal(&self) -> Journal {
        self.inner
            .lock()
            .expect("mock lock poisoned")
            .journal
            .clone()
    }

    pub fn state(&self) -> SourcesState {
        self.inner.lock().expect("mock lock poisoned").state.clone()
    }

    /// Blocks until the simulated instance has recorded `n` messages, or the
    /// deadline passes.
    ///
    /// UDP delivery is asynchronous even over loopback, so a test that sends
    /// and immediately asserts is testing the scheduler. Returns whether the
    /// count was reached, so a caller can assert on it rather than hang.
    pub async fn wait_for_messages(&self, n: usize, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.journal().messages.len() >= n {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

impl Inner {
    /// Mirrors WebLinked's `ControlApi::handleOsc`: strip the prefix, peel off
    /// an optional `source/<id>/`, dispatch the verb. Kept structurally
    /// parallel to the original on purpose — if that dispatch changes, the
    /// diff should be obvious here too.
    fn apply(&mut self, message: Message) {
        let Some(rest) = message.address.strip_prefix(&self.prefix) else {
            self.journal.rejected += 1;
            return;
        };
        let action = rest.trim_start_matches('/');

        let (source_id, verb) = match action.strip_prefix("source/") {
            Some(tail) => match tail.split_once('/') {
                Some((id, verb)) if !id.is_empty() => (Some(id.to_string()), verb.to_string()),
                _ => {
                    self.journal.rejected += 1;
                    return;
                }
            },
            None => (self.state.primary.clone(), action.to_string()),
        };

        let Some(source) = self.state.sources.iter_mut().find(|s| s.id == source_id) else {
            self.journal.rejected += 1;
            return;
        };

        if let Some(name) = verb.strip_prefix("output/") {
            let enabled = message.first_bool(true);
            if let Some(output) = source.outputs.iter_mut().find(|o| o.name == name) {
                output.enabled = Some(enabled);
                output.running = Some(enabled);
            }
        } else {
            match verb.as_str() {
                "url" => {
                    let url = message.first_string();
                    if !url.is_empty() {
                        source.source.url = Some(url.clone());
                        source.source.loaded_url = Some(url);
                    }
                }
                "reload" => self.journal.reloads += 1,
                "script" => {
                    let script = message.first_string();
                    if !script.is_empty() {
                        self.journal.scripts.push(script);
                    }
                }
                "mute" => source.source.audio_muted = Some(message.first_bool(true)),
                "format" => source.format = Some(message.first_string()),
                _ => {
                    self.journal.rejected += 1;
                    return;
                }
            }
        }

        self.journal.messages.push(message);
    }

    fn authorised(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = &self.token else {
            return true;
        };
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|given| given == expected)
            .unwrap_or(false)
    }
}

type Shared = Arc<Mutex<Inner>>;

async fn get_sources(
    State(inner): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<SourcesState>, StatusCode> {
    let guard = inner.lock().expect("mock lock poisoned");
    if !guard.authorised(&headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(guard.state.clone()))
}

async fn get_state(
    State(inner): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<SourceState>, StatusCode> {
    let guard = inner.lock().expect("mock lock poisoned");
    if !guard.authorised(&headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let primary = guard.state.primary.clone();
    guard
        .state
        .sources
        .iter()
        .find(|s| s.id == primary)
        .or_else(|| guard.state.sources.first())
        .cloned()
        .map(Json)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

#[derive(serde::Deserialize)]
struct SourceQuery {
    #[serde(default)]
    source: Option<String>,
}

/// The simulated `/api/preview`.
///
/// Serves a real buffer of exactly `width * height * 4` bytes, because the
/// thing most worth testing is that the client agrees with the headers about
/// how big the picture is — a short read is the failure that renders as half a
/// graphic rather than as an error.
async fn get_preview(
    State(inner): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<SourceQuery>,
) -> axum::response::Response {
    let mut guard = inner.lock().expect("mock lock poisoned");
    if !guard.authorised(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // An unknown pipeline is a 404, exactly as naming a missing source is.
    if let Some(id) = &query.source {
        if !guard
            .state
            .sources
            .iter()
            .any(|s| s.id.as_deref() == Some(id.as_str()))
        {
            return StatusCode::NOT_FOUND.into_response();
        }
    }

    let Some(preview) = guard.preview.as_mut() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if preview.warmup > 0 {
        preview.warmup -= 1;
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    let (width, height) = preview.dimensions();
    preview.sequence += 1;
    let sequence = preview.sequence;

    // A recognisable gradient rather than noise: a test that cares about the
    // pixels can assert on them, and a human looking at the wall can see at a
    // glance that it is the mock and not a real graphic.
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            bgra.push((x * 255 / width.max(1)) as u8);
            bgra.push((y * 255 / height.max(1)) as u8);
            bgra.push((sequence % 256) as u8);
            bgra.push(255);
        }
    }

    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::HeaderName::from_static("x-frame-width"),
                width.to_string(),
            ),
            (
                header::HeaderName::from_static("x-frame-height"),
                height.to_string(),
            ),
            (
                header::HeaderName::from_static("x-frame-sequence"),
                sequence.to_string(),
            ),
        ],
        bgra,
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct InputBody {
    events: Vec<InputEvent>,
}

async fn post_input(
    State(inner): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<InputBody>,
) -> StatusCode {
    let mut guard = inner.lock().expect("mock lock poisoned");
    if !guard.authorised(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    guard.journal.input.extend(body.events);
    StatusCode::OK
}

#[derive(serde::Deserialize)]
struct OutputUpdate {
    name: String,
    output: OutputSpec,
}

#[derive(serde::Deserialize)]
struct OutputSpec {
    #[serde(default)]
    options: serde_json::Value,
}

async fn post_output_update(
    State(inner): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<OutputUpdate>,
) -> StatusCode {
    let mut guard = inner.lock().expect("mock lock poisoned");
    if !guard.authorised(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    if body.name != "preview" {
        return StatusCode::NOT_FOUND;
    }
    let Some(factor) = body.output.options.get("factor").and_then(|v| v.as_u64()) else {
        return StatusCode::BAD_REQUEST;
    };
    // WebLinked clamps rather than refusing; match that so a test cannot pass
    // here and behave differently against the real thing.
    let factor = factor.clamp(1, 16) as u8;
    guard.journal.preview_factors.push(factor);
    if let Some(preview) = guard.preview.as_mut() {
        preview.factor = factor;
    }
    // The reported state has to move as well: rookery decides whether to write
    // again by reading this back, so a mock that accepted the change without
    // reflecting it would hide exactly the loop this models.
    for source in &mut guard.state.sources {
        for output in &mut source.outputs {
            if output.kind == "preview" {
                output.factor = Some(factor);
            }
        }
    }
    StatusCode::OK
}

/// One source, one NDI output, healthy — the shape of a plain command-line
/// WebLinked launch.
pub fn default_state() -> SourcesState {
    SourcesState {
        primary: Some("main".to_string()),
        sources: vec![SourceState {
            id: Some("main".to_string()),
            version: Some("0.7.1".to_string()),
            running: Some(true),
            format: Some("1920x1080p50".to_string()),
            outputs: vec![
                // Every WebLinked has one of these unless it was started
                // --no-preview, and rookery reads its factor to decide whether
                // the preview needs reconfiguring — so a mock without one is a
                // mock that cannot catch a reconciliation loop.
                OutputInfo {
                    kind: "preview".to_string(),
                    name: "preview".to_string(),
                    running: Some(true),
                    enabled: Some(true),
                    frames: Some(0),
                    factor: Some(1),
                    ..Default::default()
                },
                OutputInfo {
                    kind: "ndi".to_string(),
                    name: "Graphic".to_string(),
                    running: Some(true),
                    enabled: Some(true),
                    frames: Some(0),
                    receivers: Some(0),
                    ..Default::default()
                },
            ],
            compiled_backends: vec!["preview".to_string(), "ndi".to_string()],
            ..Default::default()
        }],
    }
}

/// A fleet of `n` simulated instances, named `gfx-1`..`gfx-n`.
pub async fn demo_fleet(n: usize) -> anyhow::Result<Vec<MockInstance>> {
    let mut fleet = Vec::with_capacity(n);
    for i in 1..=n {
        let mut state = default_state();
        state.sources[0].source.url = Some(format!("https://example.com/graphic-{i}"));
        fleet.push(MockInstance::start_with(state, None).await?);
    }
    Ok(fleet)
}
