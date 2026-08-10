use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use rookery_core::{Command, Health, Instance, InstanceState};
use rookery_fleet::{Fanout, Scope, Target};

use crate::error::{parse_instance_id, ApiError};
use crate::state::AppState;

#[derive(Serialize)]
pub struct InstanceView {
    #[serde(flatten)]
    pub instance: Instance,
    pub state: InstanceState,
    pub health: Health,
}

#[derive(Serialize)]
pub struct GroupView {
    pub tag: String,
    pub members: usize,
    /// The worst health among the members — what the group chip is coloured
    /// by, so a fault inside a collapsed group is still visible.
    pub health: Health,
}

#[derive(Serialize)]
pub struct StateResponse {
    pub instances: Vec<InstanceView>,
    pub groups: Vec<GroupView>,
    pub northbound: Option<String>,
    pub northbound_prefix: String,
}

pub fn build_state_response(state: &AppState) -> StateResponse {
    let snapshot = state.fleet.snapshot();

    let mut by_tag: BTreeMap<String, Vec<Health>> = BTreeMap::new();
    let instances: Vec<InstanceView> = snapshot
        .into_iter()
        .map(|(instance, instance_state)| {
            let health = instance_state.health();
            for tag in &instance.tags {
                by_tag.entry(tag.clone()).or_default().push(health);
            }
            InstanceView {
                instance: instance.redacted(),
                state: instance_state,
                health,
            }
        })
        .collect();

    let groups = by_tag
        .into_iter()
        .map(|(tag, healths)| GroupView {
            members: healths.len(),
            health: worst(&healths),
            tag,
        })
        .collect();

    StateResponse {
        instances,
        groups,
        northbound: state.northbound.clone(),
        northbound_prefix: state.northbound_prefix.clone(),
    }
}

fn worst(healths: &[Health]) -> Health {
    healths
        .iter()
        .copied()
        .max_by_key(|h| match h {
            Health::Ok => 0,
            Health::Unknown => 1,
            Health::Degraded => 2,
            Health::Stopped => 3,
            Health::Fault => 4,
        })
        .unwrap_or(Health::Unknown)
}

pub async fn get_state(State(state): State<AppState>) -> Json<StateResponse> {
    Json(build_state_response(&state))
}

// ---------------------------------------------------------------- instances

#[derive(Deserialize)]
pub struct InstanceBody {
    pub name: String,
    pub host: String,
    #[serde(default)]
    pub osc_port: Option<u16>,
    #[serde(default)]
    pub http_port: Option<u16>,
    #[serde(default)]
    pub osc_prefix: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub poll: Option<bool>,
}

impl InstanceBody {
    fn apply_to(self, mut instance: Instance) -> Instance {
        instance.name = self.name;
        instance.host = self.host;
        if let Some(p) = self.osc_port {
            instance.osc_port = p;
        }
        if let Some(p) = self.http_port {
            instance.http_port = p;
        }
        if let Some(p) = self.osc_prefix {
            instance.osc_prefix = p;
        }
        instance.tags = self
            .tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if let Some(poll) = self.poll {
            instance.poll = poll;
        }
        match self.token.as_deref() {
            // The frontend echoes back the redaction for an unchanged token.
            // Treating that as the literal new token would lock rookery out
            // of the instance on the next save, which is a nasty way to lose
            // observability mid-show.
            Some("********") => {}
            Some("") => instance.credentials.token = None,
            Some(t) => instance.credentials.token = Some(t.to_string()),
            None => {}
        }
        instance
    }
}

pub async fn create_instance(
    State(state): State<AppState>,
    Json(body): Json<InstanceBody>,
) -> Result<Json<Instance>, ApiError> {
    let instance = body.apply_to(Instance::new("", ""));
    instance
        .validate()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state.fleet.registry().upsert(instance.clone())?;
    Ok(Json(instance.redacted()))
}

pub async fn update_instance(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<InstanceBody>,
) -> Result<Json<Instance>, ApiError> {
    let id = parse_instance_id(&id)?;
    let existing = state
        .fleet
        .registry()
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("no instance {id}")))?;
    let updated = body.apply_to(existing);
    updated
        .validate()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state.fleet.registry().upsert(updated.clone())?;
    Ok(Json(updated.redacted()))
}

pub async fn delete_instance(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_instance_id(&id)?;
    let removed = state.fleet.registry().remove(&id)?;
    if removed.is_none() {
        return Err(ApiError::NotFound(format!("no instance {id}")));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ----------------------------------------------------------------- commands

#[derive(Deserialize)]
pub struct SendBody {
    #[serde(flatten)]
    pub command: Command,
    #[serde(default)]
    pub source: Option<String>,
}

async fn send(
    state: &AppState,
    target: Target,
    command: Command,
) -> Result<Json<Fanout>, ApiError> {
    // An empty fan-out is a 404, not a 200 with nothing in it. A cue that
    // silently matched nothing is the exact failure this project exists to
    // avoid, and a green toast saying "sent to 0 instances" is how it would
    // look if this returned success.
    if state.fleet.resolve(&target).is_empty() {
        return Err(ApiError::NotFound(format!(
            "{} matched no instances — nothing was sent",
            state.fleet.describe_target(&target)
        )));
    }
    Ok(Json(state.fleet.send(&target, &command).await))
}

pub async fn send_to_instance(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<Fanout>, ApiError> {
    let id = parse_instance_id(&id)?;
    let target = Target::instance(id).with_source(body.source);
    send(&state, target, body.command).await
}

pub async fn send_to_group(
    State(state): State<AppState>,
    Path(tag): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<Fanout>, ApiError> {
    let target = Target::group(tag).with_source(body.source);
    send(&state, target, body.command).await
}

pub async fn send_to_all(
    State(state): State<AppState>,
    Json(body): Json<SendBody>,
) -> Result<Json<Fanout>, ApiError> {
    let target = Target::all().with_source(body.source);
    send(&state, target, body.command).await
}

/// What a target currently resolves to, without sending anything.
///
/// The UI calls this before a disruptive group command so the confirmation
/// prompt can name the machines rather than a count. "Change the URL on
/// gfx-1, gfx-2 and gfx-5?" is a question someone can answer; "change 3
/// instances?" is not.
#[derive(Deserialize)]
pub struct ResolveQuery {
    pub scope: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

pub async fn resolve_target(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ResolveQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scope = match query.scope.as_str() {
        "all" => Scope::All,
        "group" => Scope::Group {
            tag: query
                .tag
                .ok_or_else(|| ApiError::BadRequest("group scope needs a tag".into()))?,
        },
        "instance" => Scope::Instance {
            id: parse_instance_id(
                &query
                    .id
                    .ok_or_else(|| ApiError::BadRequest("instance scope needs an id".into()))?,
            )?,
        },
        other => return Err(ApiError::BadRequest(format!("unknown scope {other:?}"))),
    };
    let target = Target {
        scope,
        source: None,
    };
    let names: Vec<String> = state
        .fleet
        .resolve(&target)
        .into_iter()
        .map(|i| i.name)
        .collect();
    Ok(Json(serde_json::json!({
        "target": state.fleet.describe_target(&target),
        "names": names,
    })))
}

// ---------------------------------------------------------------- discovery

pub async fn scan_discovery(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let found = state.discovery.scan().await?;
    let known: Vec<String> = state
        .fleet
        .registry()
        .list()
        .into_iter()
        .map(|i| i.host)
        .collect();
    let fresh: Vec<_> = found
        .into_iter()
        .filter(|d| !known.contains(&d.host))
        .collect();
    Ok(Json(serde_json::json!({ "found": fresh })))
}
