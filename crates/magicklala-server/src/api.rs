use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use magicklala_connectors::{ConnectorConfig, ConnectorKind, InboundEvent, MAX_WEBHOOK_BODY_BYTES, WEBHOOK_SIGNATURE_HEADER, WebhookConnector};
use magicklala_embedder::{ChunkConfig, DocumentInput, DocumentKind, KnowledgePipeline};
use magicklala_graph::{
    EngineServices, EventEmitter, LlmGateway, NodeCatalog, NodeId, NodeInstance, NodeSpec, RunId, RunStatus, RunSummary, RunTrigger, SpellEngine, SpellError,
    SpellGraph, SpellValidator, TraceEvent, ValidationError, Value, WireId,
};
use magicklala_runtime::{
    AgentStatus, CORRELATION_ID_KEY, CopilotError, CopilotRequest, CopilotSuggestion, DEFAULT_RUN_TIMEOUT, DispatchError, GraphPatchOp, MeteredLlmGateway,
    ReleaseOrigin, RuntimeEvent, SharedEmbedder,
};
use magicklala_store::*;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{PathItem, RefOr, ResponseBuilder};
use utoipa::{IntoParams, Modify, OpenApi, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::memory::content_hash;
use crate::secrets::project_secret;

const DEFAULT_SEARCH_LIMIT: u32 = 5;
const DEFAULT_RUN_LIMIT: u32 = 50;
const CHAT_REPLY_GRACE: Duration = Duration::from_secs(5);
const BEARER_SCHEME: &str = "bearer_token";
const PUBLIC_PATHS: [&str; 2] = ["/health", "/api/v1/webhooks/{path}"];
const SPELL_RUN_TRIGGER_KINDS: [&str; 3] = ["event/on_start", "event/on_message", "core/on_message"];
const HIDDEN_WEBHOOK_HEADERS: [&str; 4] = ["authorization", "cookie", "proxy-authorization", "x-magicklala-signature"];

#[derive(Deserialize, ToSchema)]
pub struct CreateProject {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateSpell {
    pub project_id: Uuid,
    pub name: String,
    #[schema(value_type = Object)]
    pub graph: SpellGraph,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct RunSpell {
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub trigger_node: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateAgent {
    pub project_id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub settings: serde_json::Value,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct PatchAgent {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    #[schema(value_type = Option<Object>)]
    pub settings: Option<serde_json::Value>,
    pub default_spell_id: Option<Uuid>,
    pub pinned_release_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct PublishRelease {
    pub spell_id: Option<Uuid>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ChatRequest {
    pub content: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default = "default_sender")]
    pub sender: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateCredential {
    pub project_id: Uuid,
    pub name: String,
    pub provider: String,
    #[schema(write_only)]
    pub secret: String,
}

#[derive(Deserialize, ToSchema)]
pub struct AddKnowledge {
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub markdown: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct AddMemory {
    pub content: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub metadata: serde_json::Value,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateWebhook {
    pub agent_id: Uuid,
    pub path: String,
    #[serde(default)]
    pub secret_credential_id: Option<Uuid>,
}

#[derive(Deserialize, IntoParams)]
pub struct ProjectFilter {
    pub project_id: Option<Uuid>,
}

#[derive(Deserialize, IntoParams)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<u32>,
}

#[derive(Deserialize, IntoParams)]
pub struct LimitQuery {
    pub limit: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
}

#[derive(Serialize)]
pub struct AgentView {
    #[serde(flatten)]
    pub agent: Agent,
    pub status: AgentStatus,
}

async fn embed(state: &AppState, text: &str) -> ApiResult<Vec<f32>> {
    let mut vectors = state.embedder.embed(&[text.to_owned()]).await.map_err(|error| ApiError::internal(error.to_string()))?;
    vectors.pop().ok_or_else(|| ApiError::internal("embedder returned no vector"))
}

fn default_channel() -> String {
    "rest".to_owned()
}

fn default_sender() -> String {
    "user".to_owned()
}

fn created<T: Serialize>(value: T) -> impl IntoResponse {
    (StatusCode::CREATED, Json(value))
}

async fn agent_view(state: &AppState, agent: Agent) -> AgentView {
    let status = state.supervisor.status(agent.id.0).await.unwrap_or(AgentStatus::Stopped);
    AgentView { agent, status }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "MagickLaLa control plane", description = "REST and WebSocket API for MagickLaLa projects, spells, agents, credentials, knowledge and runs."),
    paths(
        health,
        health_v1,
        list_node_specs,
        create_project,
        list_projects,
        create_spell,
        list_spells,
        get_spell,
        save_spell_graph,
        validate_spell,
        run_spell,
        create_agent,
        list_agents,
        get_agent,
        patch_agent,
        delete_agent,
        publish_release,
        list_releases,
        start_agent,
        stop_agent,
        chat_with_agent,
        create_credential,
        list_credentials,
        delete_credential,
        add_knowledge,
        search_knowledge,
        add_memory,
        search_memories,
        list_runs,
        create_webhook,
        receive_webhook,
        copilot_suggest,
        crate::ws::websocket,
    ),
    components(schemas(
        CreateProject,
        CreateSpell,
        RunSpell,
        CreateAgent,
        PatchAgent,
        PublishRelease,
        ChatRequest,
        CreateCredential,
        AddKnowledge,
        AddMemory,
        CreateWebhook,
        ValidationReport
    )),
    modifiers(&SecurityDocs)
)]
pub struct ApiDoc;

struct SecurityDocs;

fn operations_mut(item: &mut PathItem) -> impl Iterator<Item = &mut utoipa::openapi::path::Operation> {
    [&mut item.get, &mut item.put, &mut item.post, &mut item.delete, &mut item.patch, &mut item.options, &mut item.head, &mut item.trace]
        .into_iter()
        .filter_map(Option::as_mut)
}

impl Modify for SecurityDocs {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(BEARER_SCHEME, SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()));
        openapi.security = Some(vec![SecurityRequirement::new(BEARER_SCHEME, Vec::<String>::new())]);
        for (path, item) in &mut openapi.paths.paths {
            let public = PUBLIC_PATHS.contains(&path.as_str());
            for operation in operations_mut(item) {
                if public {
                    operation.security = Some(Vec::new());
                } else {
                    operation
                        .responses
                        .responses
                        .entry("401".to_owned())
                        .or_insert_with(|| RefOr::T(ResponseBuilder::new().description("Missing or invalid bearer token").build()));
                }
            }
        }
    }
}

#[utoipa::path(get, path = "/health", tag = "system", responses((status = 200, description = "Server is up")))]
pub async fn health() -> &'static str {
    "ok"
}

#[utoipa::path(get, path = "/api/v1/health", tag = "system", responses((status = 200, description = "Server is up")))]
pub async fn health_v1() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

pub async fn openapi_document() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[utoipa::path(get, path = "/api/v1/node-specs", tag = "spells", responses((status = 200, description = "Every node kind the runtime can execute")))]
pub async fn list_node_specs(State(state): State<AppState>) -> Json<Vec<NodeSpec>> {
    Json(state.registry.specs())
}

#[utoipa::path(post, path = "/api/v1/projects", tag = "projects", request_body = CreateProject, responses((status = 201, description = "Project created"), (status = 409, description = "Name taken")))]
pub async fn create_project(State(state): State<AppState>, ApiJson(body): ApiJson<CreateProject>) -> ApiResult<impl IntoResponse> {
    Ok(created(state.store.create_project(NewProject { name: body.name, description: body.description }).await?))
}

#[utoipa::path(get, path = "/api/v1/projects", tag = "projects", responses((status = 200, description = "All projects")))]
pub async fn list_projects(State(state): State<AppState>) -> ApiResult<Json<Vec<Project>>> {
    Ok(Json(state.store.list_projects().await?))
}

#[utoipa::path(post, path = "/api/v1/spells", tag = "spells", request_body = CreateSpell, responses((status = 201, description = "Spell created"), (status = 404, description = "Unknown project")))]
pub async fn create_spell(State(state): State<AppState>, ApiJson(body): ApiJson<CreateSpell>) -> ApiResult<impl IntoResponse> {
    Ok(created(state.store.create_spell(NewSpell { project_id: ProjectId(body.project_id), name: body.name, graph: body.graph }).await?))
}

#[utoipa::path(get, path = "/api/v1/spells", tag = "spells", params(ProjectFilter), responses((status = 200, description = "Spells of a project")))]
pub async fn list_spells(State(state): State<AppState>, ApiQuery(filter): ApiQuery<ProjectFilter>) -> ApiResult<Json<Vec<SpellRecord>>> {
    let project = filter.project_id.ok_or_else(|| ApiError::bad_request("invalid_query", "project_id is required"))?;
    Ok(Json(state.store.list_spells(ProjectId(project)).await?))
}

#[utoipa::path(get, path = "/api/v1/spells/{id}", tag = "spells", params(("id" = Uuid, Path, description = "Spell id")), responses((status = 200, description = "Spell"), (status = 404, description = "Unknown spell")))]
pub async fn get_spell(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<SpellRecord>> {
    Ok(Json(state.store.get_spell(SpellRecordId(id)).await?))
}

#[utoipa::path(put, path = "/api/v1/spells/{id}/graph", tag = "spells", params(("id" = Uuid, Path, description = "Spell id")), responses((status = 200, description = "Graph saved")))]
pub async fn save_spell_graph(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(graph): ApiJson<SpellGraph>) -> ApiResult<Json<SpellRecord>> {
    Ok(Json(state.store.save_spell_graph(SpellRecordId(id), graph).await?))
}

fn validation_report(state: &AppState, graph: &SpellGraph) -> ValidationReport {
    match SpellValidator::new(state.registry.specs()).validate(graph) {
        Ok(()) => ValidationReport { valid: true, errors: Vec::new() },
        Err(errors) => ValidationReport { valid: false, errors: errors.iter().map(ToString::to_string).collect() },
    }
}

#[utoipa::path(post, path = "/api/v1/spells/{id}/validate", tag = "spells", params(("id" = Uuid, Path, description = "Spell id")), responses((status = 200, description = "Validation report", body = ValidationReport)))]
pub async fn validate_spell(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<ValidationReport>> {
    let spell = state.store.get_spell(SpellRecordId(id)).await?;
    Ok(Json(validation_report(&state, &spell.graph)))
}

struct BusTrace {
    state: AppState,
    agent: Option<Uuid>,
    collected: Mutex<Vec<TraceEvent>>,
}

impl BusTrace {
    fn events(&self) -> Vec<TraceEvent> {
        self.collected.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl EventEmitter for BusTrace {
    fn emit(&self, event: TraceEvent) {
        self.collected.lock().unwrap_or_else(PoisonError::into_inner).push(event.clone());
        self.state.supervisor.bus().publish(RuntimeEvent::Trace { agent_id: self.agent, event });
    }
}

fn node_spec(state: &AppState, node: &NodeInstance) -> Option<NodeSpec> {
    state.registry.get(&node.kind).map(|executor| executor.spec().clone())
}

fn is_trigger(state: &AppState, node: &NodeInstance) -> bool {
    node_spec(state, node).is_some_and(|spec| spec.is_trigger)
}

fn preferred_trigger<'graph>(state: &AppState, graph: &'graph SpellGraph) -> Option<&'graph NodeInstance> {
    let triggers: Vec<&NodeInstance> = graph.nodes.iter().filter(|node| is_trigger(state, node)).collect();
    SPELL_RUN_TRIGGER_KINDS.iter().find_map(|kind| triggers.iter().find(|node| node.kind == *kind).copied()).or_else(|| triggers.first().copied())
}

fn spell_trigger(state: &AppState, graph: &SpellGraph, requested: Option<&str>, payload: Value) -> ApiResult<RunTrigger> {
    let node = match requested {
        Some(id) => graph
            .node(&NodeId::from(id))
            .filter(|node| is_trigger(state, node))
            .ok_or_else(|| ApiError::bad_request("invalid_trigger", format!("{id} is not a trigger node of this spell")))?,
        None => preferred_trigger(state, graph).ok_or_else(|| ApiError::bad_request("invalid_spell", "the spell has no trigger node"))?,
    };
    let socket = node_spec(state, node).and_then(|spec| spec.outputs.first().map(|socket| socket.name.clone())).unwrap_or_else(|| "out".to_owned());
    Ok(RunTrigger { node: node.id.clone(), socket, payload })
}

fn spell_engine(state: &AppState, project: ProjectId) -> SpellEngine {
    let llm: Arc<dyn LlmGateway> = Arc::new(MeteredLlmGateway::new(state.llms.for_project(project), Arc::clone(&state.store)));
    let base = state.engine.services();
    let services = EngineServices { llm: Some(llm), memory: base.memory.clone(), knowledge: base.knowledge.clone() };
    let catalog: Arc<dyn NodeCatalog> = state.registry.clone();
    SpellEngine::new(catalog, services).with_step_limit(state.engine.step_limit()).with_private_http(state.runs.allow_private_http)
}

struct SpellRunLog<'run> {
    spell: &'run SpellRecord,
    trigger: &'run RunTrigger,
    started_at: DateTime<Utc>,
    events: &'run [TraceEvent],
}

impl SpellRunLog<'_> {
    fn run_id(&self) -> RunId {
        self.events
            .iter()
            .find_map(|event| match event {
                TraceEvent::RunStarted { run_id, .. } => Some(*run_id),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn finished_summary(&self) -> Option<RunSummary> {
        self.events.iter().rev().find_map(|event| match event {
            TraceEvent::RunFinished { summary, .. } => Some(summary.clone()),
            _ => None,
        })
    }

    fn nodes_finished(&self) -> u64 {
        u64::try_from(self.events.iter().filter(|event| matches!(event, TraceEvent::NodeFinished { .. })).count()).unwrap_or(u64::MAX)
    }

    fn failed_summary(&self, error: &SpellError) -> RunSummary {
        let mut summary = self.finished_summary().unwrap_or_else(|| RunSummary {
            run_id: self.run_id(),
            spell_id: self.spell.graph.id,
            started_at: self.started_at,
            finished_at: Utc::now(),
            status: RunStatus::Failed,
            nodes_executed: self.nodes_finished(),
            outputs: std::collections::BTreeMap::new(),
            activations: Vec::new(),
            activations_truncated: false,
            error: None,
        });
        if summary.status == RunStatus::Succeeded {
            summary.status = RunStatus::Failed;
        }
        summary.error = summary.error.or_else(|| Some(error.to_string()));
        summary
    }

    fn execution_run(&self, run_id: RunId, status: RunStatus, error: Option<String>, nodes_executed: u64) -> ExecutionRun {
        ExecutionRun {
            run_id: run_id.0,
            agent_id: None,
            spell_id: self.spell.id,
            release_id: None,
            status,
            trigger: self.trigger.node.0.clone(),
            started_at: self.started_at,
            finished_at: Some(Utc::now()),
            nodes_executed,
            error,
            trace: serde_json::to_value(self.events).unwrap_or_default(),
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/spells/{id}/run",
    tag = "spells",
    params(("id" = Uuid, Path, description = "Spell id")),
    request_body = RunSpell,
    responses(
        (status = 200, description = "Run succeeded: summary and trace"),
        (status = 400, description = "Spell is invalid, has no trigger or the body is malformed"),
        (status = 404, description = "Unknown spell"),
        (status = 422, description = "Run failed: error, run_id, summary and trace; the failed run is recorded"),
        (status = 504, description = "Run exceeded MAGICKLALA_SPELL_RUN_TIMEOUT_MS and was cancelled; the timed out run is recorded")
    )
)]
pub async fn run_spell(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, raw: Bytes) -> ApiResult<Response> {
    let body: RunSpell = if raw.is_empty() {
        RunSpell::default()
    } else {
        serde_json::from_slice(&raw).map_err(|error| ApiError::bad_request("invalid_body", error.to_string()))?
    };
    let spell = state.store.get_spell(SpellRecordId(id)).await?;
    let report = validation_report(&state, &spell.graph);
    if !report.valid {
        return Err(ApiError::bad_request("invalid_spell", report.errors.join("; ")));
    }
    let trigger = spell_trigger(&state, &spell.graph, body.trigger_node.as_deref(), Value::from(body.input))?;
    let trace = Arc::new(BusTrace { state: state.clone(), agent: None, collected: Mutex::new(Vec::new()) });
    let emitter: Arc<dyn EventEmitter> = Arc::clone(&trace) as Arc<dyn EventEmitter>;
    let engine = spell_engine(&state, spell.project_id);
    let cancellation = CancellationToken::new();
    let started_at = Utc::now();
    let outcome = tokio::time::timeout(state.runs.timeout, engine.run_with_cancellation(&spell.graph, trigger.clone(), emitter, cancellation.clone())).await;
    cancellation.cancel();
    let events = trace.events();
    let log = SpellRunLog { spell: &spell, trigger: &trigger, started_at, events: &events };
    match outcome {
        Ok(Ok(summary)) => {
            state.store.record_run(log.execution_run(summary.run_id, summary.status, summary.error.clone(), summary.nodes_executed)).await?;
            Ok(Json(serde_json::json!({ "summary": summary, "trace": events })).into_response())
        }
        Ok(Err(SpellError::Validation(errors))) => {
            Err(ApiError::bad_request("invalid_spell", errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")))
        }
        Ok(Err(error)) => {
            let summary = log.failed_summary(&error);
            let message = summary.error.clone().unwrap_or_else(|| error.to_string());
            state.store.record_run(log.execution_run(summary.run_id, summary.status, Some(message.clone()), summary.nodes_executed)).await?;
            let body = serde_json::json!({ "error": message, "code": "spell_failed", "run_id": summary.run_id, "summary": summary, "trace": events });
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response())
        }
        Err(_elapsed) => {
            let run_id = log.run_id();
            let message = format!("spell run exceeded {} ms and was cancelled", state.runs.timeout.as_millis());
            state.store.record_run(log.execution_run(run_id, RunStatus::TimedOut, Some(message.clone()), log.nodes_finished())).await?;
            let body = serde_json::json!({ "error": message, "code": "run_timeout", "run_id": run_id, "trace": events });
            Ok((StatusCode::GATEWAY_TIMEOUT, Json(body)).into_response())
        }
    }
}
#[utoipa::path(post, path = "/api/v1/agents", tag = "agents", request_body = CreateAgent, responses((status = 201, description = "Agent created")))]
pub async fn create_agent(State(state): State<AppState>, ApiJson(body): ApiJson<CreateAgent>) -> ApiResult<impl IntoResponse> {
    let agent = state
        .store
        .create_agent(NewAgent { project_id: ProjectId(body.project_id), name: body.name, description: body.description, settings: body.settings })
        .await?;
    state.supervisor.register(agent.id.0).await;
    Ok(created(agent_view(&state, agent).await))
}

#[utoipa::path(get, path = "/api/v1/agents", tag = "agents", params(ProjectFilter), responses((status = 200, description = "Agents with live status")))]
pub async fn list_agents(State(state): State<AppState>, ApiQuery(filter): ApiQuery<ProjectFilter>) -> ApiResult<Json<Vec<AgentView>>> {
    let projects = match filter.project_id {
        Some(project) => vec![ProjectId(project)],
        None => state.store.list_projects().await?.into_iter().map(|project| project.id).collect(),
    };
    let mut views = Vec::new();
    for project in projects {
        for agent in state.store.list_agents(project).await? {
            views.push(agent_view(&state, agent).await);
        }
    }
    Ok(Json(views))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 200, description = "Agent with live status"), (status = 404, description = "Unknown agent")))]
pub async fn get_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<AgentView>> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    Ok(Json(agent_view(&state, agent).await))
}

#[utoipa::path(
    patch,
    path = "/api/v1/agents/{id}",
    tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    request_body = PatchAgent,
    responses(
        (status = 200, description = "Agent updated; a running agent is redeployed onto a newly pinned release"),
        (status = 400, description = "Invalid field, or the release does not belong to the agent's spell"),
        (status = 404, description = "Unknown agent, spell or release")
    )
)]
pub async fn patch_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(body): ApiJson<PatchAgent>) -> ApiResult<Json<AgentView>> {
    let current = state.store.get_agent(AgentId(id)).await?;
    let requested_spell = body.default_spell_id.map(SpellRecordId);
    let pinned = match body.pinned_release_id {
        Some(release) => Some(owned_release(&state, &current, requested_spell, ReleaseId(release)).await?),
        None => None,
    };
    let update = UpdateAgent {
        name: body.name,
        description: body.description,
        enabled: body.enabled,
        settings: body.settings,
        default_spell_id: pinned.as_ref().map(|release| release.spell_id).or(requested_spell).map(Some),
        pinned_release_id: pinned.as_ref().map(|release| Some(release.id)),
    };
    let agent = state.store.update_agent(AgentId(id), update).await?;
    if !agent.enabled {
        state.supervisor.stop(agent.id.0).await.ok();
    } else if let Some(release) = &pinned
        && state.supervisor.status(agent.id.0).await.is_some()
    {
        deploy_release(&state, agent.id.0, release).await?;
    }
    Ok(Json(agent_view(&state, agent).await))
}

async fn owned_release(state: &AppState, agent: &Agent, requested_spell: Option<SpellRecordId>, release: ReleaseId) -> ApiResult<SpellRelease> {
    let spell_id = requested_spell
        .or(agent.default_spell_id)
        .ok_or_else(|| ApiError::bad_request("invalid_body", "pin a release only after the agent has a default spell"))?;
    let spell = state.store.get_spell(spell_id).await?;
    if spell.project_id != agent.project_id {
        return Err(ApiError::bad_request("invalid_body", "spell belongs to another project"));
    }
    state
        .store
        .list_releases(spell_id)
        .await?
        .into_iter()
        .find(|candidate| candidate.id == release)
        .ok_or_else(|| ApiError::bad_request("invalid_body", format!("release {} does not belong to this agent's spell", release.0)))
}

#[utoipa::path(delete, path = "/api/v1/agents/{id}", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 204, description = "Agent deleted and removed from the runtime"), (status = 404, description = "Unknown agent")))]
pub async fn delete_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<StatusCode> {
    state.store.delete_agent(AgentId(id)).await?;
    state.supervisor.forget(id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/releases",
    tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    request_body = PublishRelease,
    responses(
        (status = 201, description = "Release snapshot published, pinned and deployed"),
        (status = 400, description = "Spell is invalid or belongs to another project"),
        (status = 404, description = "Unknown agent or spell")
    )
)]
pub async fn publish_release(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<PublishRelease>,
) -> ApiResult<impl IntoResponse> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    let spell_id = body
        .spell_id
        .map(SpellRecordId)
        .or(agent.default_spell_id)
        .ok_or_else(|| ApiError::bad_request("invalid_body", "spell_id is required when the agent has no default spell"))?;
    let spell = state.store.get_spell(spell_id).await?;
    if spell.project_id != agent.project_id {
        return Err(ApiError::bad_request("invalid_body", "spell belongs to another project"));
    }
    let snapshot = spell.graph;
    let report = validation_report(&state, &snapshot);
    if !report.valid {
        return Err(ApiError::bad_request("invalid_spell", report.errors.join("; ")));
    }
    let release = state.store.create_release_with_graph(NewSpellRelease { spell_id, notes: body.notes }, Some(snapshot)).await?;
    let pin = UpdateAgent { default_spell_id: Some(Some(spell_id)), pinned_release_id: Some(Some(release.id)), ..UpdateAgent::default() };
    state.store.update_agent(agent.id, pin).await?;
    if state.supervisor.status(agent.id.0).await.is_some() {
        deploy_release(&state, agent.id.0, &release).await?;
    }
    Ok(created(release))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/releases", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 200, description = "Releases of the agent's spell"), (status = 404, description = "Unknown agent")))]
pub async fn list_releases(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<Vec<SpellRelease>>> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    match agent.default_spell_id {
        Some(spell) => Ok(Json(state.store.list_releases(spell).await?)),
        None => Ok(Json(Vec::new())),
    }
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/start", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 200, description = "Agent running"), (status = 400, description = "Pinned release is invalid"), (status = 404, description = "Unknown agent"), (status = 409, description = "Already running, disabled or unreleased")))]
pub async fn start_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<AgentView>> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    if !agent.enabled {
        return Err(ApiError::conflict("agent_disabled", "agent is disabled"));
    }
    let pinned = pinned_release(&state, &agent).await?.ok_or_else(|| ApiError::conflict("no_release", "publish a release before starting the agent"))?;
    let report = validation_report(&state, &pinned.graph);
    if !report.valid {
        return Err(ApiError::bad_request("invalid_spell", report.errors.join("; ")));
    }
    state.supervisor.register(id).await;
    if state.supervisor.release(id).await.is_none_or(|deployed| deployed.release_record_id() != Some(pinned.id)) {
        deploy_release(&state, id, &pinned).await?;
    }
    state.supervisor.start(id).await?;
    Ok(Json(agent_view(&state, agent).await))
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/stop", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 200, description = "Agent stopped"), (status = 404, description = "Unknown agent")))]
pub async fn stop_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<AgentView>> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    state.supervisor.register(id).await;
    state.supervisor.stop(id).await?;
    Ok(Json(agent_view(&state, agent).await))
}

async fn pinned_release(state: &AppState, agent: &Agent) -> ApiResult<Option<SpellRelease>> {
    let (Some(release_id), Some(spell_id)) = (agent.pinned_release_id, agent.default_spell_id) else { return Ok(None) };
    Ok(state.store.list_releases(spell_id).await?.into_iter().find(|release| release.id == release_id))
}

async fn deploy_release(state: &AppState, agent_id: Uuid, release: &SpellRelease) -> ApiResult<()> {
    let origin = ReleaseOrigin { spell_id: release.spell_id, release_id: Some(release.id) };
    state.supervisor.deploy_release_from(agent_id, release.graph.clone(), origin).await.map(|_| ()).map_err(dispatch_error)
}

fn dispatch_error(error: DispatchError) -> ApiError {
    match error {
        DispatchError::InvalidRelease(detail) => ApiError::bad_request("invalid_spell", detail),
        DispatchError::NotRunning(_) => ApiError::conflict("agent_not_running", "start the agent before sending it events"),
        DispatchError::NoRelease(_) => ApiError::conflict("no_release", "publish a release before starting the agent"),
        DispatchError::Busy(_) => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "agent_busy", "the agent mailbox is full; retry shortly"),
        DispatchError::Runtime(runtime) => runtime.into(),
        other => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "dispatch_failed", other.to_string()),
    }
}

#[derive(Serialize)]
pub struct ChatExchange {
    pub message: ChatMessage,
    pub reply: ChatMessage,
}

enum Completion {
    Finished { status: RunStatus, reply: Option<String>, error: Option<String> },
    ReplyLost,
    BusClosed,
}

async fn recorded_completion(state: &AppState, agent_id: Uuid, correlation: &str) -> Option<Completion> {
    let runs = state.store.list_runs(AgentId(agent_id), DEFAULT_RUN_LIMIT).await.ok()?;
    let run = runs.into_iter().find(|run| run.trace.get(CORRELATION_ID_KEY).and_then(serde_json::Value::as_str) == Some(correlation))?;
    Some(match run.status {
        RunStatus::Succeeded => Completion::ReplyLost,
        status => Completion::Finished { status, reply: None, error: run.error },
    })
}

async fn await_completion(state: &AppState, events: &mut Receiver<RuntimeEvent>, agent_id: Uuid, correlation: &str) -> Completion {
    loop {
        match events.recv().await {
            Ok(RuntimeEvent::RunCompleted { agent_id: source, correlation_id: Some(id), status, reply, error, .. })
                if source == agent_id && id == correlation =>
            {
                return Completion::Finished { status, reply, error };
            }
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => {
                if let Some(completion) = recorded_completion(state, agent_id, correlation).await {
                    return completion;
                }
            }
            Err(RecvError::Closed) => return Completion::BusClosed,
        }
    }
}

fn chat_reply(completion: Completion) -> ApiResult<String> {
    match completion {
        Completion::Finished { status: RunStatus::Succeeded, reply: Some(reply), .. } => Ok(reply),
        Completion::Finished { status: RunStatus::Succeeded, reply: None, .. } => {
            Err(ApiError::new(StatusCode::BAD_GATEWAY, "no_reply", "agent produced no reply"))
        }
        Completion::Finished { status: RunStatus::TimedOut, error, .. } => {
            Err(ApiError::new(StatusCode::GATEWAY_TIMEOUT, "run_timeout", error.unwrap_or_else(|| "the agent run timed out".to_owned())))
        }
        Completion::Finished { status, error, .. } => {
            Err(ApiError::new(StatusCode::BAD_GATEWAY, "run_failed", error.unwrap_or_else(|| format!("run ended {status:?}"))))
        }
        Completion::ReplyLost => {
            Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "reply_lost", "the run finished but its reply was dropped because the event stream lagged"))
        }
        Completion::BusClosed => Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "event_bus_closed", "the runtime event bus closed")),
    }
}

pub async fn deliver_chat(state: &AppState, agent_id: Uuid, body: ChatRequest) -> ApiResult<ChatExchange> {
    let agent = state.store.get_agent(AgentId(agent_id)).await?;
    if body.content.trim().is_empty() {
        return Err(ApiError::bad_request("invalid_body", "content must not be empty"));
    }
    if state.supervisor.status(agent.id.0).await != Some(AgentStatus::Running) {
        return Err(ApiError::conflict("agent_not_running", "start the agent before chatting with it"));
    }
    let message = state
        .store
        .append_chat(NewChatMessage {
            agent_id: agent.id,
            channel: body.channel.clone(),
            role: ChatRole::User,
            sender: body.sender.clone(),
            content: body.content.clone(),
            metadata: serde_json::Value::Null,
        })
        .await?;
    let correlation = Uuid::new_v4().to_string();
    let mut payload = serde_json::to_value(&message).unwrap_or_default();
    if let Some(fields) = payload.as_object_mut() {
        fields.insert(CORRELATION_ID_KEY.to_owned(), serde_json::Value::String(correlation.clone()));
    }
    let mut events = state.supervisor.bus().subscribe();
    let channel = body.channel.clone();
    state
        .supervisor
        .dispatch_inbound(InboundEvent {
            connector_id: Uuid::nil(),
            agent_id: agent.id.0,
            kind: ConnectorKind::Rest,
            channel: body.channel,
            sender: body.sender,
            content: body.content,
            payload,
            received_at: Utc::now(),
        })
        .await
        .map_err(dispatch_error)?;
    let completion = tokio::time::timeout(DEFAULT_RUN_TIMEOUT + CHAT_REPLY_GRACE, await_completion(state, &mut events, agent.id.0, &correlation))
        .await
        .map_err(|_| ApiError::new(StatusCode::GATEWAY_TIMEOUT, "run_timeout", "the agent did not reply in time"))?;
    let content = chat_reply(completion)?;
    let reply = state
        .store
        .append_chat(NewChatMessage {
            agent_id: agent.id,
            channel,
            role: ChatRole::Assistant,
            sender: agent.name.clone(),
            content,
            metadata: serde_json::json!({ "in_reply_to": message.id, "correlation_id": correlation }),
        })
        .await?;
    Ok(ChatExchange { message, reply })
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/chat",
    tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    request_body = ChatRequest,
    responses(
        (status = 200, description = "The agent's correlated reply to this message"),
        (status = 400, description = "Empty content or malformed body"),
        (status = 404, description = "Unknown agent"),
        (status = 409, description = "Agent not running"),
        (status = 502, description = "The run failed or produced no reply"),
        (status = 503, description = "Agent mailbox is full or the reply was lost"),
        (status = 504, description = "The run timed out")
    )
)]
pub async fn chat_with_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(body): ApiJson<ChatRequest>) -> ApiResult<impl IntoResponse> {
    Ok(Json(deliver_chat(&state, id, body).await?))
}
#[utoipa::path(post, path = "/api/v1/credentials", tag = "credentials", request_body = CreateCredential, responses((status = 201, description = "Credential sealed; metadata only")))]
pub async fn create_credential(State(state): State<AppState>, ApiJson(body): ApiJson<CreateCredential>) -> ApiResult<impl IntoResponse> {
    let input = NewCredential { project_id: ProjectId(body.project_id), name: body.name, provider: body.provider, secret: body.secret };
    Ok(created(state.store.put_credential(input, state.vault.as_ref()).await?))
}

#[utoipa::path(get, path = "/api/v1/credentials", tag = "credentials", params(ProjectFilter), responses((status = 200, description = "Credential metadata, never secrets")))]
pub async fn list_credentials(State(state): State<AppState>, ApiQuery(filter): ApiQuery<ProjectFilter>) -> ApiResult<Json<Vec<CredentialMeta>>> {
    let project = filter.project_id.ok_or_else(|| ApiError::bad_request("invalid_query", "project_id is required"))?;
    Ok(Json(state.store.list_credentials(ProjectId(project)).await?))
}

#[utoipa::path(delete, path = "/api/v1/credentials/{id}", tag = "credentials", params(("id" = Uuid, Path, description = "Credential id")), responses((status = 204, description = "Credential deleted")))]
pub async fn delete_credential(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<StatusCode> {
    state.store.delete_credential(CredentialId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/knowledge", tag = "knowledge", params(("id" = Uuid, Path, description = "Agent id")), request_body = AddKnowledge, responses((status = 201, description = "Document chunked and embedded")))]
pub async fn add_knowledge(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(body): ApiJson<AddKnowledge>) -> ApiResult<impl IntoResponse> {
    let pipeline =
        KnowledgePipeline::new(SharedEmbedder(Arc::clone(&state.embedder)), ChunkConfig::default()).map_err(|error| ApiError::internal(error.to_string()))?;
    let kind = if body.markdown { DocumentKind::Markdown } else { DocumentKind::Text };
    let document = DocumentInput { title: body.title.clone(), kind, source_uri: body.source_uri.clone(), bytes: body.content.clone().into_bytes() };
    let chunks = pipeline.ingest(&document).await.map_err(|error| ApiError::bad_request("invalid_document", error.to_string()))?;
    let chunks = chunks
        .into_iter()
        .map(|chunk| KnowledgeChunk { doc_id: KnowledgeDocId::default(), index: chunk.index, text: chunk.text, embedding: Some(chunk.embedding) })
        .collect();
    let mime = if body.markdown { "text/markdown" } else { "text/plain" };
    let doc = NewKnowledgeDoc {
        agent_id: AgentId(id),
        title: body.title,
        source_uri: body.source_uri,
        mime: mime.to_owned(),
        content_hash: content_hash(body.content.as_bytes()),
    };
    Ok(created(state.store.add_knowledge_doc(doc, chunks).await?))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/knowledge/search", tag = "knowledge", params(("id" = Uuid, Path, description = "Agent id"), SearchQuery), responses((status = 200, description = "Ranked knowledge chunks")))]
pub async fn search_knowledge(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> ApiResult<Json<Vec<SearchHit<KnowledgeChunk>>>> {
    let embedding = embed(&state, &query.q).await?;
    Ok(Json(state.store.search_knowledge(AgentId(id), &query.q, &embedding, query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT)).await?))
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/memories", tag = "knowledge", params(("id" = Uuid, Path, description = "Agent id")), request_body = AddMemory, responses((status = 201, description = "Memory stored")))]
pub async fn add_memory(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(body): ApiJson<AddMemory>) -> ApiResult<impl IntoResponse> {
    if body.content.trim().is_empty() {
        return Err(ApiError::bad_request("invalid_body", "content must not be empty"));
    }
    let embedding = embed(&state, &body.content).await?;
    Ok(created(state.store.add_memory(NewMemory { agent_id: AgentId(id), content: body.content, embedding: Some(embedding), metadata: body.metadata }).await?))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/memories/search", tag = "knowledge", params(("id" = Uuid, Path, description = "Agent id"), SearchQuery), responses((status = 200, description = "Ranked memories")))]
pub async fn search_memories(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> ApiResult<Json<Vec<SearchHit<MemoryRecord>>>> {
    let embedding = embed(&state, &query.q).await?;
    Ok(Json(state.store.search_memories(AgentId(id), &embedding, query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT)).await?))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/runs", tag = "runs", params(("id" = Uuid, Path, description = "Agent id"), LimitQuery), responses((status = 200, description = "Most recent runs first")))]
pub async fn list_runs(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiQuery(query): ApiQuery<LimitQuery>) -> ApiResult<Json<Vec<ExecutionRun>>> {
    Ok(Json(state.store.list_runs(AgentId(id), query.limit.unwrap_or(DEFAULT_RUN_LIMIT)).await?))
}

#[utoipa::path(post, path = "/api/v1/webhooks", tag = "runs", request_body = CreateWebhook, responses((status = 201, description = "Webhook registered under a normalized path"), (status = 400, description = "Invalid path or a secret credential from another project"), (status = 404, description = "Unknown agent"), (status = 409, description = "Path taken")))]
pub async fn create_webhook(State(state): State<AppState>, ApiJson(body): ApiJson<CreateWebhook>) -> ApiResult<impl IntoResponse> {
    let agent = state.store.get_agent(AgentId(body.agent_id)).await?;
    if let Some(credential) = body.secret_credential_id
        && !state.store.list_credentials(agent.project_id).await?.iter().any(|meta| meta.id.0 == credential)
    {
        return Err(ApiError::bad_request("invalid_body", "the webhook secret credential must belong to the agent's project"));
    }
    let input = NewWebhook { agent_id: agent.id, path: body.path, secret_credential_id: body.secret_credential_id.map(CredentialId) };
    Ok(created(state.store.create_webhook(input).await?))
}

fn body_rejection(rejection: &BytesRejection) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", format!("webhook body exceeds {MAX_WEBHOOK_BODY_BYTES} bytes"))
    } else {
        ApiError::bad_request("invalid_body", rejection.body_text())
    }
}

async fn verify_webhook_signature(state: &AppState, webhook: &WebhookRecord, project: ProjectId, headers: &HeaderMap, raw: &[u8]) -> ApiResult<()> {
    let Some(credential) = webhook.secret_credential_id else { return Ok(()) };
    let rejected = |detail: String| ApiError::new(StatusCode::UNAUTHORIZED, "invalid_signature", detail);
    let secret = project_secret(state.store.as_ref(), state.vault.as_ref(), project, credential).await.map_err(rejected)?;
    let config = ConnectorConfig {
        id: webhook.id.0,
        agent_id: webhook.agent_id.0,
        kind: ConnectorKind::Webhook,
        enabled: webhook.enabled,
        credential_id: Some(credential.0),
        settings: serde_json::json!({ "path": webhook.path }),
    };
    let signature = headers.get(WEBHOOK_SIGNATURE_HEADER).and_then(|value| value.to_str().ok());
    WebhookConnector::new(config, Some(secret)).verify(raw, signature).map_err(|rejection| rejected(rejection.to_string()))
}

fn webhook_body(raw: &[u8]) -> ApiResult<serde_json::Value> {
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_slice(raw).map_err(|error| ApiError::bad_request("invalid_body", format!("webhook body is not JSON: {error}")))
}

fn forwarded_headers(headers: &HeaderMap) -> serde_json::Value {
    headers
        .iter()
        .filter(|(name, _)| !HIDDEN_WEBHOOK_HEADERS.contains(&name.as_str()))
        .filter_map(|(name, value)| value.to_str().ok().map(|text| (name.as_str().to_owned(), serde_json::Value::String(text.to_owned()))))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

fn webhook_content(body: &serde_json::Value) -> String {
    ["content", "text", "message"]
        .iter()
        .find_map(|key| body.get(key).and_then(serde_json::Value::as_str))
        .or_else(|| body.as_str())
        .map_or_else(|| body.to_string(), ToOwned::to_owned)
}

#[utoipa::path(
    post,
    path = "/api/v1/webhooks/{path}",
    tag = "runs",
    params(
        ("path" = String, Path, description = "Registered webhook path"),
        ("X-MagickLaLa-Signature" = Option<String>, Header, description = "sha256=<hex HMAC-SHA256 of the raw body>, required when the webhook has a secret credential")
    ),
    responses(
        (status = 202, description = "Signature verified and event dispatched to the agent"),
        (status = 400, description = "Invalid path or malformed JSON body"),
        (status = 401, description = "Missing or invalid X-MagickLaLa-Signature, or the secret is unavailable"),
        (status = 404, description = "Unknown webhook"),
        (status = 409, description = "Webhook disabled or agent not running"),
        (status = 413, description = "Body larger than 1 MiB"),
        (status = 503, description = "Agent mailbox is full")
    )
)]
pub async fn receive_webhook(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<String>,
    ApiQuery(query): ApiQuery<HashMap<String, String>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<impl IntoResponse> {
    let raw = body.map_err(|rejection| body_rejection(&rejection))?;
    let webhook = state.store.find_webhook(&normalize_webhook_path(&path)?).await?;
    if !webhook.enabled {
        return Err(ApiError::conflict("webhook_disabled", "webhook is disabled"));
    }
    let agent = state.store.get_agent(webhook.agent_id).await?;
    verify_webhook_signature(&state, &webhook, agent.project_id, &headers, &raw).await?;
    let body = webhook_body(&raw)?;
    let content = webhook_content(&body);
    let payload = serde_json::json!({ "method": "POST", "path": webhook.path, "headers": forwarded_headers(&headers), "query": query, "body": body });
    let event = InboundEvent {
        connector_id: webhook.id.0,
        agent_id: webhook.agent_id.0,
        kind: ConnectorKind::Webhook,
        channel: webhook.path.clone(),
        sender: "webhook".to_owned(),
        content,
        payload,
        received_at: Utc::now(),
    };
    state.supervisor.register(webhook.agent_id.0).await;
    state.supervisor.dispatch_inbound(event).await.map_err(dispatch_error)?;
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "accepted": true, "webhook_id": webhook.id, "agent_id": webhook.agent_id }))))
}
fn repair_ops(errors: &[ValidationError]) -> Vec<GraphPatchOp> {
    let mut wires: Vec<WireId> = errors
        .iter()
        .filter_map(|error| match error {
            ValidationError::MissingNode { wire, .. } | ValidationError::MissingSocket { wire, .. } | ValidationError::TypeMismatch { wire, .. } => {
                Some(wire.clone())
            }
            _ => None,
        })
        .collect();
    wires.dedup();
    let unknown = errors.iter().filter_map(|error| match error {
        ValidationError::UnknownNodeKind { node, .. } => Some(GraphPatchOp::RemoveNode { node_id: node.clone() }),
        _ => None,
    });
    wires.into_iter().map(|wire_id| GraphPatchOp::RemoveWire { wire_id }).chain(unknown).collect()
}

#[utoipa::path(post, path = "/api/v1/copilot/suggest", tag = "copilot", responses((status = 200, description = "Suggested graph patch")))]
pub async fn copilot_suggest(State(state): State<AppState>, ApiJson(request): ApiJson<CopilotRequest>) -> ApiResult<Json<CopilotSuggestion>> {
    let errors = SpellValidator::new(state.registry.specs()).validate(&request.spell).err().unwrap_or_default();
    if !errors.is_empty() {
        let ops = repair_ops(&errors);
        let summary = format!("{} has {} problem(s); proposing {} repair op(s)", request.spell.name, errors.len(), ops.len());
        return Ok(Json(CopilotSuggestion { summary, ops }));
    }
    match state.copilot.suggest(&request).await {
        Ok((suggestion, _)) => Ok(Json(suggestion)),
        Err(CopilotError::Rejected(rejection)) => Err(ApiError::bad_request("patch_rejected", rejection.errors.join("; "))),
        Err(error) => Err(ApiError::new(StatusCode::BAD_GATEWAY, "copilot_failed", error.to_string())),
    }
}

pub async fn route_not_found() -> ApiError {
    ApiError::not_found("no such route")
}
