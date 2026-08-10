//! The real transport to a WebLinked instance.
//!
//! Two protocols, two directions, and the asymmetry between them is the
//! whole design:
//!
//! - **Out: OSC over UDP.** Fire-and-forget. No acknowledgement, no reply, no
//!   error short of the socket itself refusing the write. Fast enough for a
//!   cue, and unaffected by an instance being busy.
//! - **Back: HTTP polling.** WebLinked's `/api/sources` carries everything —
//!   which outputs are running, whether the page loaded, how the clock is
//!   doing. This is the *only* way rookery ever learns whether a command
//!   landed.
//!
//! An operator has to be able to tell those apart, so nothing in here ever
//! reports a send as a confirmation.
//!
//! ## Two things about reaching a real instance
//!
//! **WebLinked binds its HTTP server to `127.0.0.1` by default.** An instance
//! started without `--bind` is fully controllable from rookery over OSC (that
//! listener defaults to `0.0.0.0`) and completely unpollable. That is not a
//! bug in either program, and it is the single most likely reason a newly
//! added instance shows commands going out and no state coming back.
//!
//! **A token protects HTTP only.** `--token` gates the HTTP API; WebLinked's
//! OSC listener has no authentication of any kind. Anyone who can reach
//! UDP 7655 can change what is on air regardless of what rookery is
//! configured with.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use rookery_core::client::{InstanceClient, InstanceClientProvider};
use rookery_core::{Command, Instance, SourceState, SourcesState};
use rookery_osc::OscSender;

/// How long to wait on the state poll. Generous: WebLinked answers in
/// milliseconds when healthy, but a machine mid-format-change has every
/// output reopening, and timing out during exactly the moment something
/// interesting is happening is when a dashboard is least useful.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

pub struct LiveClient {
    instance: Instance,
    sender: OscSender,
    http: reqwest::Client,
}

impl LiveClient {
    pub fn new(instance: Instance, sender: OscSender, http: reqwest::Client) -> Self {
        Self {
            instance,
            sender,
            http,
        }
    }

    fn authorised(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.instance.credentials.token {
            // The bearer header rather than `?token=`, so the secret never
            // lands in a proxy log or a URL that gets pasted into a ticket.
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn get(&self, path: &str) -> anyhow::Result<reqwest::Response> {
        let url = format!("{}{path}", self.instance.base_url());
        let response = self
            .authorised(self.http.get(&url))
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                // reqwest's own message is about a URL; an operator needs to
                // know which of their machines it was.
                anyhow::anyhow!("{} ({}): {e}", self.instance.name, url)
            })?;
        Ok(response)
    }

    fn check_status(&self, response: &reqwest::Response) -> anyhow::Result<()> {
        match response.status().as_u16() {
            200 => Ok(()),
            401 => anyhow::bail!(
                "{}: 401 — this instance was started with --token and rookery's stored \
                 token is missing or wrong",
                self.instance.name
            ),
            other => anyhow::bail!("{}: HTTP {other}", self.instance.name),
        }
    }
}

#[async_trait]
impl InstanceClient for LiveClient {
    async fn send(&self, command: &Command, source: Option<&str>) -> anyhow::Result<()> {
        let target = self.instance.osc_target()?;
        let message = command.to_osc(&self.instance.osc_prefix, source);
        tracing::info!(
            instance = %self.instance.name,
            %target,
            address = %message.address,
            "osc send: {}",
            command.summary()
        );
        self.sender.send(target, &message).await
    }

    async fn send_all(&self, commands: &[Command], source: Option<&str>) -> anyhow::Result<()> {
        if commands.is_empty() {
            return Ok(());
        }
        let target = self.instance.osc_target()?;
        let messages: Vec<_> = commands
            .iter()
            .map(|c| c.to_osc(&self.instance.osc_prefix, source))
            .collect();
        tracing::info!(
            instance = %self.instance.name,
            %target,
            count = messages.len(),
            "osc send bundle"
        );
        self.sender.send_bundle(target, &messages).await
    }

    async fn state(&self) -> anyhow::Result<SourcesState> {
        let response = self.get("/api/sources").await?;

        // `/api/sources` arrived with multi-source support, after v0.3.0. An
        // older instance answers 404, and falling back to `/api/state` costs
        // one extra request on exactly those instances and keeps them visible
        // instead of permanently red.
        if response.status().as_u16() == 404 {
            let response = self.get("/api/state").await?;
            self.check_status(&response)?;
            let single: SourceState = response.json().await?;
            return Ok(SourcesState {
                primary: single.id.clone(),
                sources: vec![single],
            });
        }

        self.check_status(&response)?;
        Ok(response.json().await?)
    }
}

/// Builds `LiveClient`s over one shared UDP socket and one shared HTTP
/// connection pool.
pub struct LiveClientProvider {
    sender: OscSender,
    http: reqwest::Client,
}

impl LiveClientProvider {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self {
            sender: OscSender::new().await?,
            http: reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?,
        })
    }

    pub fn sender(&self) -> &OscSender {
        &self.sender
    }
}

impl InstanceClientProvider for LiveClientProvider {
    fn client_for(&self, instance: &Instance) -> Arc<dyn InstanceClient> {
        Arc::new(LiveClient::new(
            instance.clone(),
            self.sender.clone(),
            self.http.clone(),
        ))
    }
}
