//! End-to-end tests over real sockets: `LiveClient` -> UDP -> simulated
//! WebLinked -> HTTP -> `LiveClient`.
//!
//! Nothing here is mocked at the API boundary. The encoder, the datagram, the
//! address grammar and the JSON shape are all exercised — which is the only
//! reason these tests are worth having, since wire-format faults are the ones
//! this project is most exposed to.

use std::time::Duration;

use rookery_core::client::{InstanceClient, InstanceClientProvider};
use rookery_core::{Command, Instance};
use rookery_instance_live::LiveClientProvider;
use rookery_instance_mock::MockInstance;

const SETTLE: Duration = Duration::from_secs(2);

/// One named output of the primary source. Named rather than indexed because a
/// real instance carries a preview output alongside whatever else is
/// configured, and its position is an implementation detail.
fn named_output<'a>(
    state: &'a rookery_core::SourcesState,
    name: &str,
) -> &'a rookery_core::OutputInfo {
    state.sources[0]
        .outputs
        .iter()
        .find(|o| o.name == name)
        .unwrap_or_else(|| panic!("no output called {name:?}"))
}

async fn fixture() -> (MockInstance, std::sync::Arc<dyn InstanceClient>) {
    let mock = MockInstance::start().await.unwrap();
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    let provider = LiveClientProvider::new().await.unwrap();
    let client = provider.client_for(&instance);
    (mock, client)
}

#[tokio::test]
async fn a_url_command_crosses_the_wire_and_changes_the_page() {
    let (mock, client) = fixture().await;

    client
        .send(
            &Command::Url {
                url: "https://example.com/lower-third".to_string(),
            },
            None,
        )
        .await
        .unwrap();

    assert!(
        mock.wait_for_messages(1, SETTLE).await,
        "no message arrived"
    );
    let state = client.state().await.unwrap();
    assert_eq!(
        state.sources[0].source.loaded_url.as_deref(),
        Some("https://example.com/lower-third")
    );
    assert_eq!(mock.journal().rejected, 0);
}

/// The regression that matters most. WebLinked shipped a decoder that dropped
/// any OSC string whose length was 3 mod 4, so `/weblinked/url` worked for
/// most addresses and silently did nothing for a quarter of them. Sweep every
/// residue across the wire and require all four to land.
#[tokio::test]
async fn urls_of_every_length_residue_actually_arrive() {
    let (mock, client) = fixture().await;

    let mut sent = Vec::new();
    for extra in 0..8 {
        let url = format!("https://example.com/{}", "a".repeat(extra));
        client
            .send(&Command::Url { url: url.clone() }, None)
            .await
            .unwrap();
        sent.push(url);
    }

    assert!(
        mock.wait_for_messages(sent.len(), SETTLE).await,
        "only {} of {} messages arrived — this is the padding residue bug",
        mock.journal().messages.len(),
        sent.len()
    );
    assert_eq!(mock.journal().rejected, 0);

    let arrived: Vec<String> = mock
        .journal()
        .messages
        .iter()
        .map(|m| m.first_string())
        .collect();
    assert_eq!(arrived, sent);
}

#[tokio::test]
async fn a_source_selector_addresses_one_pipeline_and_leaves_the_others_alone() {
    let mut state = rookery_instance_mock::default_state();
    // A second pipeline, as `--config` would produce.
    let mut second = state.sources[0].clone();
    second.id = Some("lower-third".to_string());
    second.source.url = Some("https://example.com/original".to_string());
    state.sources.push(second);

    let mock = MockInstance::start_with(state, None).await.unwrap();
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    let provider = LiveClientProvider::new().await.unwrap();
    let client = provider.client_for(&instance);

    client
        .send(
            &Command::Url {
                url: "https://example.com/changed".to_string(),
            },
            Some("lower-third"),
        )
        .await
        .unwrap();
    assert!(mock.wait_for_messages(1, SETTLE).await);

    let state = client.state().await.unwrap();
    let primary = state
        .sources
        .iter()
        .find(|s| s.id.as_deref() == Some("main"));
    let named = state
        .sources
        .iter()
        .find(|s| s.id.as_deref() == Some("lower-third"));
    assert_eq!(
        named.unwrap().source.loaded_url.as_deref(),
        Some("https://example.com/changed")
    );
    assert_ne!(
        primary.unwrap().source.loaded_url.as_deref(),
        Some("https://example.com/changed"),
        "a source-addressed command leaked into the primary"
    );
}

#[tokio::test]
async fn a_bundle_keeps_url_and_reload_in_order() {
    let (mock, client) = fixture().await;

    client
        .send_all(
            &[
                Command::Url {
                    url: "https://example.com/next".to_string(),
                },
                Command::Reload { ignore_cache: true },
            ],
            None,
        )
        .await
        .unwrap();

    assert!(mock.wait_for_messages(2, SETTLE).await);
    let journal = mock.journal();
    assert_eq!(journal.messages[0].address, "/weblinked/url");
    assert_eq!(journal.messages[1].address, "/weblinked/reload");
    assert_eq!(journal.reloads, 1);
}

#[tokio::test]
async fn an_output_toggle_names_the_output_in_the_address() {
    let (mock, client) = fixture().await;

    client
        .send(
            &Command::Output {
                name: "Graphic".to_string(),
                enabled: false,
            },
            None,
        )
        .await
        .unwrap();
    assert!(mock.wait_for_messages(1, SETTLE).await);

    assert_eq!(
        mock.journal().messages[0].address,
        "/weblinked/output/Graphic"
    );
    let state = client.state().await.unwrap();
    // By name, not by index: a real instance has a preview output too, and its
    // position among the others is not something to depend on.
    assert_eq!(named_output(&state, "Graphic").enabled, Some(false));
}

/// An output whose name has a space is entirely normal — "Programme Fill" —
/// and it goes into the OSC address, where a naive encoder could mangle it.
#[tokio::test]
async fn an_output_name_with_a_space_survives_the_address() {
    let mut state = rookery_instance_mock::default_state();
    let graphic = state.sources[0]
        .outputs
        .iter_mut()
        .find(|o| o.name == "Graphic")
        .unwrap();
    graphic.name = "Programme Fill".to_string();
    let mock = MockInstance::start_with(state, None).await.unwrap();

    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    let client = LiveClientProvider::new()
        .await
        .unwrap()
        .client_for(&instance);

    client
        .send(
            &Command::Output {
                name: "Programme Fill".to_string(),
                enabled: false,
            },
            None,
        )
        .await
        .unwrap();
    assert!(mock.wait_for_messages(1, SETTLE).await);

    assert_eq!(mock.journal().rejected, 0);
    let state = client.state().await.unwrap();
    assert_eq!(named_output(&state, "Programme Fill").enabled, Some(false));
}

#[tokio::test]
async fn a_token_is_required_when_the_instance_wants_one() {
    let mock = MockInstance::start_with_token("s3cret").await.unwrap();
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    let provider = LiveClientProvider::new().await.unwrap();

    // Without it, the poll fails with a message that names the actual cause
    // rather than a bare 401.
    let err = provider
        .client_for(&instance)
        .state()
        .await
        .expect_err("polling without a token should fail");
    assert!(
        err.to_string().contains("--token"),
        "unhelpful error: {err}"
    );

    instance.credentials.token = Some("s3cret".to_string());
    assert!(provider.client_for(&instance).state().await.is_ok());
}

/// The most likely misconfiguration in the field: WebLinked binds HTTP to
/// loopback unless told otherwise, so commands go out fine and state never
/// comes back. rookery must fail the poll and keep sending.
#[tokio::test]
async fn commands_still_send_when_the_http_side_is_unreachable() {
    let mock = MockInstance::start().await.unwrap();
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    // A port nothing is listening on.
    instance.http_port = 1;
    let client = LiveClientProvider::new()
        .await
        .unwrap()
        .client_for(&instance);

    client
        .send(
            &Command::Reload {
                ignore_cache: false,
            },
            None,
        )
        .await
        .expect("OSC send must not depend on the HTTP side");
    assert!(mock.wait_for_messages(1, SETTLE).await);
    assert_eq!(mock.journal().reloads, 1);

    let err = client.state().await.expect_err("poll should fail");
    assert!(
        err.to_string().contains("gfx-1"),
        "error should name the instance: {err}"
    );
}

#[tokio::test]
async fn a_script_command_carries_the_whole_script() {
    let (mock, client) = fixture().await;
    let script = "lowerThird.show('Anna Kowalski','Head of Sound')";

    client
        .send(
            &Command::Script {
                script: script.to_string(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(mock.wait_for_messages(1, SETTLE).await);

    assert_eq!(mock.journal().scripts, vec![script.to_string()]);
}

// ------------------------------------------------------------------ preview

#[tokio::test]
async fn a_preview_frame_arrives_whole_and_the_sequence_advances() {
    let (_mock, client) = fixture().await;

    let first = client.preview(None).await.unwrap().expect("a frame");
    assert_eq!((first.width, first.height), (1920, 1080));
    assert!(
        first.is_complete(),
        "claimed {}x{} = {} bytes, got {}",
        first.width,
        first.height,
        first.expected_len(),
        first.bgra.len()
    );

    let second = client.preview(None).await.unwrap().expect("a frame");
    assert!(
        second.sequence > first.sequence,
        "the sequence must advance or nothing can tell a live feed from a frozen one"
    );
}

/// `--no-preview` is a legitimate way to run an SDI-only machine. It must read
/// as "no picture available", not as a broken instance.
#[tokio::test]
async fn an_instance_without_a_preview_says_so_rather_than_failing() {
    let mock = MockInstance::start().await.unwrap().without_preview();
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    let client = LiveClientProvider::new()
        .await
        .unwrap()
        .client_for(&instance);

    assert_eq!(
        client.preview(None).await.unwrap().unwrap_err(),
        rookery_core::PreviewUnavailable::NotConfigured
    );
}

/// The second after a format change, every output is reopening and there is no
/// frame yet. Also not a fault.
#[tokio::test]
async fn a_warming_up_instance_reports_no_frame_yet_then_recovers() {
    let mock = MockInstance::start().await.unwrap().with_preview_warmup(2);
    let mut instance = Instance::new("gfx-1", "127.0.0.1");
    instance.osc_port = mock.osc_port();
    instance.http_port = mock.http_port();
    let client = LiveClientProvider::new()
        .await
        .unwrap()
        .client_for(&instance);

    for _ in 0..2 {
        assert_eq!(
            client.preview(None).await.unwrap().unwrap_err(),
            rookery_core::PreviewUnavailable::NoFrameYet
        );
    }
    assert!(client.preview(None).await.unwrap().is_ok());
}

#[tokio::test]
async fn setting_the_factor_shrinks_the_frame() {
    let (mock, client) = fixture().await;
    assert_eq!(client.preview(None).await.unwrap().unwrap().width, 1920);

    client.set_preview_factor(8).await.unwrap();
    assert_eq!(mock.journal().preview_factors, vec![8]);

    let frame = client.preview(None).await.unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (240, 135));
    assert!(frame.is_complete());
    // The point of the exercise: 8.29 MB down to 129.6 KB.
    assert_eq!(frame.bgra.len(), 240 * 135 * 4);
}

/// A pipeline that does not exist must 404 rather than silently handing back
/// the primary's picture — showing one graphic while labelling it another is
/// the worst thing a preview can do.
#[tokio::test]
async fn an_unknown_source_has_no_preview() {
    let (_mock, client) = fixture().await;
    assert!(
        client.preview(Some("no-such-pipeline")).await.is_err()
            || matches!(
                client.preview(Some("no-such-pipeline")).await,
                Ok(Err(rookery_core::PreviewUnavailable::NotConfigured))
            )
    );
}

// -------------------------------------------------------------------- input

#[tokio::test]
async fn input_events_reach_the_instance_in_order() {
    use rookery_core::{InputEvent, KeyAction};

    let (mock, client) = fixture().await;
    let events = vec![
        InputEvent::Focus { focused: true },
        InputEvent::Move { nx: 0.5, ny: 0.5 },
        InputEvent::Down {
            nx: 0.5,
            ny: 0.5,
            button: 0,
            clicks: 1,
        },
        InputEvent::Up {
            nx: 0.5,
            ny: 0.5,
            button: 0,
        },
        // The three-event shape a character needs, with `character` as the
        // character code rather than the key code — 104 is `h`.
        InputEvent::Key {
            action: KeyAction::Down,
            key_code: 72,
            character: 104,
            modifiers: 0,
        },
        InputEvent::Key {
            action: KeyAction::Char,
            key_code: 72,
            character: 104,
            modifiers: 0,
        },
        InputEvent::Key {
            action: KeyAction::Up,
            key_code: 72,
            character: 104,
            modifiers: 0,
        },
    ];

    client.send_input(&events, None).await.unwrap();

    let arrived = mock.journal().input;
    assert_eq!(arrived.len(), events.len());
    assert!(matches!(arrived[0], InputEvent::Focus { focused: true }));
    assert!(matches!(
        arrived[4],
        InputEvent::Key {
            action: KeyAction::Down,
            key_code: 72,
            character: 104,
            ..
        }
    ));
}

#[tokio::test]
async fn an_empty_input_batch_is_not_sent_at_all() {
    let (mock, client) = fixture().await;
    client.send_input(&[], None).await.unwrap();
    assert!(mock.journal().input.is_empty());
}
