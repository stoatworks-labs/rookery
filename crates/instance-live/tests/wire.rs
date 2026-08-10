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
    assert_eq!(state.sources[0].outputs[0].enabled, Some(false));
}

/// An output whose name has a space is entirely normal — "Programme Fill" —
/// and it goes into the OSC address, where a naive encoder could mangle it.
#[tokio::test]
async fn an_output_name_with_a_space_survives_the_address() {
    let mut state = rookery_instance_mock::default_state();
    state.sources[0].outputs[0].name = "Programme Fill".to_string();
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
    assert_eq!(state.sources[0].outputs[0].enabled, Some(false));
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
