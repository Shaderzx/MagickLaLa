use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use magicklala_connectors::{ConnectorKind, InboundEvent};
use magicklala_embedder::{ChunkConfig, DocumentInput, DocumentKind, KnowledgePipeline};
use magicklala_graph::{
    EngineServices, EventEmitter, NodeCatalog, NodeId, NodeSpec, RunTrigger, SpellEngine, SpellGraph, SpellValidator, TraceEvent, ValidationError, Value,
    WireId,
};
use magicklala_runtime::{AgentStatus, CopilotRequest, CopilotSuggestion, GraphPatchOp, RuntimeEvent};
use magicklala_store::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use utoipa::{IntoParams, OpenApi, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::embedding::HashingEmbedder;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::memory::content_hash;

const DEFAULT_SEARCH_LIMIT: u32 = 5;
const DEFAULT_RUN_LIMIT: u32 = 50;

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
    ))
)]
pub struct ApiDoc;

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

impl EventEmitter for BusTrace {
    fn emit(&self, event: TraceEvent) {
        if let Ok(mut collected) = self.collected.lock() {
            collected.push(event.clone());
        }
        self.state.supervisor.bus().publish(RuntimeEvent::Trace { agent_id: self.agent, event });
    }
}

#[utoipa::path(post, path = "/api/v1/spells/{id}/run", tag = "spells", params(("id" = Uuid, Path, description = "Spell id")), request_body = RunSpell, responses((status = 200, description = "Run summary"), (status = 400, description = "Spell is invalid"), (status = 501, description = "Engine not available")))]
pub async fn run_spell(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, raw: Bytes) -> ApiResult<Json<serde_json::Value>> {
    let body: RunSpell = if raw.is_empty() {
        RunSpell::default()
    } else {
        serde_json::from_slice(&raw).map_err(|error| ApiError::bad_request("invalid_body", error.to_string()))?
    };
    let spell = state.store.get_spell(SpellRecordId(id)).await?;
    let trigger_node =
        body.trigger_node.map(NodeId).or_else(|| spell.graph.nodes.first().map(|node| node.id.clone())).unwrap_or_else(|| NodeId::from("trigger"));
    let catalog: Arc<dyn NodeCatalog> = state.registry.clone();
    let engine = SpellEngine::new(catalog, EngineServices::default());
    let trace = Arc::new(BusTrace { state: state.clone(), agent: None, collected: Mutex::new(Vec::new()) });
    let summary =
        engine.run(&spell.graph, RunTrigger { node: trigger_node, socket: "out".to_owned(), payload: Value::from(body.input) }, trace.clone()).await?;
    let events = trace.collected.lock().map(|events| events.clone()).unwrap_or_default();
    Ok(Json(serde_json::json!({ "summary": summary, "trace": events })))
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

#[utoipa::path(patch, path = "/api/v1/agents/{id}", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), request_body = PatchAgent, responses((status = 200, description = "Agent updated")))]
pub async fn patch_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(body): ApiJson<PatchAgent>) -> ApiResult<Json<AgentView>> {
    let update = UpdateAgent {
        name: body.name,
        description: body.description,
        enabled: body.enabled,
        settings: body.settings,
        default_spell_id: body.default_spell_id.map(|spell| Some(SpellRecordId(spell))),
        pinned_release_id: None,
    };
    let agent = state.store.update_agent(AgentId(id), update).await?;
    if !agent.enabled {
        state.supervisor.stop(agent.id.0).await.ok();
    }
    Ok(Json(agent_view(&state, agent).await))
}

#[utoipa::path(delete, path = "/api/v1/agents/{id}", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 204, description = "Agent deleted")))]
pub async fn delete_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<StatusCode> {
    state.store.delete_agent(AgentId(id)).await?;
    state.supervisor.stop(id).await.ok();
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/releases", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), request_body = PublishRelease, responses((status = 201, description = "Release published and pinned"), (status = 400, description = "Spell is invalid")))]
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
    let report = validation_report(&state, &spell.graph);
    if !report.valid {
        return Err(ApiError::bad_request("invalid_spell", report.errors.join("; ")));
    }
    let release = state.store.create_release(NewSpellRelease { spell_id, notes: body.notes }).await?;
    let pin = UpdateAgent { default_spell_id: Some(Some(spell_id)), pinned_release_id: Some(Some(release.id)), ..UpdateAgent::default() };
    state.store.update_agent(agent.id, pin).await?;
    Ok(created(release))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/releases", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 200, description = "Releases of the agent's spell")))]
pub async fn list_releases(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<Vec<SpellRelease>>> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    match agent.default_spell_id {
        Some(spell) => Ok(Json(state.store.list_releases(spell).await?)),
        None => Ok(Json(Vec::new())),
    }
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/start", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 200, description = "Agent running"), (status = 409, description = "Already running, disabled or unreleased")))]
pub async fn start_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<AgentView>> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    if !agent.enabled {
        return Err(ApiError::conflict("agent_disabled", "agent is disabled"));
    }
    if agent.pinned_release_id.is_none() {
        return Err(ApiError::conflict("no_release", "publish a release before starting the agent"));
    }
    state.supervisor.register(id).await;
    state.supervisor.start(id).await?;
    Ok(Json(agent_view(&state, agent).await))
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/stop", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), responses((status = 200, description = "Agent stopped")))]
pub async fn stop_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>) -> ApiResult<Json<AgentView>> {
    let agent = state.store.get_agent(AgentId(id)).await?;
    state.supervisor.register(id).await;
    state.supervisor.stop(id).await?;
    Ok(Json(agent_view(&state, agent).await))
}

pub async fn deliver_chat(state: &AppState, agent_id: Uuid, body: ChatRequest) -> ApiResult<ChatMessage> {
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
    state.supervisor.bus().publish(RuntimeEvent::Inbound {
        event: InboundEvent {
            connector_id: Uuid::nil(),
            agent_id: agent.id.0,
            kind: ConnectorKind::Rest,
            channel: body.channel,
            sender: body.sender,
            content: body.content,
            payload: serde_json::to_value(&message).unwrap_or_default(),
            received_at: Utc::now(),
        },
    });
    Ok(message)
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/chat", tag = "agents", params(("id" = Uuid, Path, description = "Agent id")), request_body = ChatRequest, responses((status = 202, description = "Message accepted"), (status = 409, description = "Agent not running")))]
pub async fn chat_with_agent(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(body): ApiJson<ChatRequest>) -> ApiResult<impl IntoResponse> {
    let message = deliver_chat(&state, id, body).await?;
    Ok((StatusCode::ACCEPTED, Json(message)))
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
    let pipeline = KnowledgePipeline::new(HashingEmbedder, ChunkConfig::default()).map_err(|error| ApiError::internal(error.to_string()))?;
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
    let embedding = HashingEmbedder::embed_one(&query.q);
    Ok(Json(state.store.search_knowledge(AgentId(id), &query.q, &embedding, query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT)).await?))
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/memories", tag = "knowledge", params(("id" = Uuid, Path, description = "Agent id")), request_body = AddMemory, responses((status = 201, description = "Memory stored")))]
pub async fn add_memory(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiJson(body): ApiJson<AddMemory>) -> ApiResult<impl IntoResponse> {
    if body.content.trim().is_empty() {
        return Err(ApiError::bad_request("invalid_body", "content must not be empty"));
    }
    let embedding = HashingEmbedder::embed_one(&body.content);
    Ok(created(state.store.add_memory(NewMemory { agent_id: AgentId(id), content: body.content, embedding: Some(embedding), metadata: body.metadata }).await?))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/memories/search", tag = "knowledge", params(("id" = Uuid, Path, description = "Agent id"), SearchQuery), responses((status = 200, description = "Ranked memories")))]
pub async fn search_memories(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> ApiResult<Json<Vec<SearchHit<MemoryRecord>>>> {
    let embedding = HashingEmbedder::embed_one(&query.q);
    Ok(Json(state.store.search_memories(AgentId(id), &embedding, query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT)).await?))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/runs", tag = "runs", params(("id" = Uuid, Path, description = "Agent id"), LimitQuery), responses((status = 200, description = "Most recent runs first")))]
pub async fn list_runs(State(state): State<AppState>, ApiPath(id): ApiPath<Uuid>, ApiQuery(query): ApiQuery<LimitQuery>) -> ApiResult<Json<Vec<ExecutionRun>>> {
    Ok(Json(state.store.list_runs(AgentId(id), query.limit.unwrap_or(DEFAULT_RUN_LIMIT)).await?))
}

#[utoipa::path(post, path = "/api/v1/webhooks", tag = "runs", request_body = CreateWebhook, responses((status = 201, description = "Webhook registered"), (status = 409, description = "Path taken")))]
pub async fn create_webhook(State(state): State<AppState>, ApiJson(body): ApiJson<CreateWebhook>) -> ApiResult<impl IntoResponse> {
    let input = NewWebhook { agent_id: AgentId(body.agent_id), path: body.path, secret_credential_id: body.secret_credential_id.map(CredentialId) };
    Ok(created(state.store.create_webhook(input).await?))
}

#[utoipa::path(post, path = "/api/v1/webhooks/{path}", tag = "runs", params(("path" = String, Path, description = "Registered webhook path")), responses((status = 202, description = "Event accepted"), (status = 404, description = "Unknown webhook")))]
pub async fn receive_webhook(State(state): State<AppState>, ApiPath(path): ApiPath<String>, body: Bytes) -> ApiResult<impl IntoResponse> {
    let webhook = state.store.find_webhook(&path).await?;
    if !webhook.enabled {
        return Err(ApiError::conflict("webhook_disabled", "webhook is disabled"));
    }
    let payload = serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&body).into_owned()));
    let content = payload.as_str().map(ToOwned::to_owned).unwrap_or_else(|| payload.to_string());
    state.supervisor.bus().publish(RuntimeEvent::Inbound {
        event: InboundEvent {
            connector_id: webhook.id.0,
            agent_id: webhook.agent_id.0,
            kind: ConnectorKind::Webhook,
            channel: webhook.path.clone(),
            sender: "webhook".to_owned(),
            content,
            payload,
            received_at: Utc::now(),
        },
    });
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
    let ops = repair_ops(&errors);
    let summary = if errors.is_empty() {
        format!("{} is valid; no repairs needed for \"{}\"", request.spell.name, request.prompt)
    } else {
        format!("{} has {} problem(s); proposing {} repair op(s)", request.spell.name, errors.len(), ops.len())
    };
    Ok(Json(CopilotSuggestion { summary, ops }))
}

pub async fn route_not_found() -> ApiError {
    ApiError::not_found("no such route")
}
