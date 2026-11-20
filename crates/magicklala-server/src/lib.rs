mod access;
mod api;
mod embedding;
mod error;
mod memory;
mod secrets;
mod subgraphs;
mod ws;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use axum::middleware;
use axum::routing::{delete, get, post, put};
use magicklala_connectors::{ConnectorRegistry, MAX_WEBHOOK_BODY_BYTES};
use magicklala_embedder::{Embedder, LocalHashEmbedder};
use magicklala_graph::{EngineServices, LlmGateway, NodeCatalog, SpellEngine, SpellValidator};
use magicklala_nodes::{LlmGatewayConfig, NodeRegistry};
use magicklala_runtime::{
    AgentSupervisor, CopilotEngine, EngineReleaseRunner, InMemoryEventBus, MeteredLlmGateway, ProjectLlmFactory, StoreKnowledgeBridge, StoreMemoryBridge,
    StoreScope,
};
use magicklala_store::{AesGcmVault, CryptoVault, ProjectId, Repository};
use tokio::sync::OwnedMutexGuard;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

pub use access::{
    ALLOWED_HOSTS_ENV, API_TOKEN_ENV, AccessPolicy, CORS_ORIGINS_ENV, LOCAL_STUDIO_ORIGINS, LOOPBACK_HOSTS, PUBLIC_PATHS, WS_TICKET_TTL, WsTickets,
    parse_hosts, parse_origins, tokens_match,
};
pub use api::ApiDoc;
pub use embedding::cosine;
pub use error::{ApiError, ApiResult};
pub use memory::{InMemoryRepository, content_hash, graph_hash};
pub use secrets::{LlmSource, ProjectLlms, VaultCredentialResolver, project_secret};
pub use subgraphs::{MAX_SUBGRAPH_LOOKUPS, ProjectSubgraphResolver, StoreSubgraphLoader};

pub const ALLOW_PRIVATE_HTTP_ENV: &str = "MAGICKLALA_ALLOW_PRIVATE_HTTP";
pub const SPELL_RUN_TIMEOUT_ENV: &str = "MAGICKLALA_SPELL_RUN_TIMEOUT_MS";
pub const DEFAULT_SPELL_RUN_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellRunSettings {
    pub timeout: Duration,
    pub allow_private_http: bool,
}

impl Default for SpellRunSettings {
    fn default() -> Self {
        Self { timeout: DEFAULT_SPELL_RUN_TIMEOUT, allow_private_http: false }
    }
}

impl SpellRunSettings {
    pub fn from_env() -> Self {
        let timeout = std::env::var(SPELL_RUN_TIMEOUT_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|millis| *millis > 0)
            .map_or(DEFAULT_SPELL_RUN_TIMEOUT, Duration::from_millis);
        let allow_private_http =
            std::env::var(ALLOW_PRIVATE_HTTP_ENV).is_ok_and(|flag| matches!(flag.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
        Self { timeout, allow_private_http }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub database_url: String,
    pub access: AccessPolicy,
    pub runs: SpellRunSettings,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 7777)),
            database_url: "sqlite://magicklala.db".to_owned(),
            access: AccessPolicy::default(),
            runs: SpellRunSettings::default(),
        }
    }
}

impl ServerConfig {
    pub fn from_env(bind: SocketAddr, database_url: String) -> Self {
        Self { bind, database_url, access: AccessPolicy::from_env().with_bind(bind), runs: SpellRunSettings::from_env() }
    }
}

#[derive(Debug, Default)]
pub struct AgentLocks {
    locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
}

impl AgentLocks {
    pub async fn lock(&self, agent_id: Uuid) -> OwnedMutexGuard<()> {
        let lock = Arc::clone(self.locks.lock().unwrap_or_else(PoisonError::into_inner).entry(agent_id).or_default());
        lock.lock_owned().await
    }

    pub fn forget(&self, agent_id: Uuid) {
        self.locks.lock().unwrap_or_else(PoisonError::into_inner).remove(&agent_id);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootReport {
    pub started: Vec<Uuid>,
    pub failed: Vec<(Uuid, String)>,
}

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<NodeRegistry>,
    pub supervisor: Arc<AgentSupervisor>,
    pub store: Arc<dyn Repository>,
    pub vault: Arc<dyn CryptoVault>,
    pub embedder: Arc<dyn Embedder>,
    pub engine: Arc<SpellEngine>,
    pub copilot: Arc<CopilotEngine>,
    pub llms: Arc<ProjectLlms>,
    pub access: Arc<AccessPolicy>,
    pub runs: SpellRunSettings,
    pub connectors: ConnectorRegistry,
    pub tickets: Arc<WsTickets>,
    pub agent_locks: Arc<AgentLocks>,
}

pub struct Wiring {
    pub registry: Arc<NodeRegistry>,
    pub store: Arc<dyn Repository>,
    pub vault: Arc<dyn CryptoVault>,
    pub llm: LlmSource,
    pub embedder: Arc<dyn Embedder>,
    pub access: AccessPolicy,
    pub runs: SpellRunSettings,
    pub connectors: Option<ConnectorRegistry>,
}

impl Wiring {
    pub fn new(registry: Arc<NodeRegistry>, store: Arc<dyn Repository>, vault: Arc<dyn CryptoVault>) -> Self {
        Self {
            registry,
            store,
            vault,
            llm: LlmSource::Http(LlmGatewayConfig::from_env()),
            embedder: Arc::new(LocalHashEmbedder::default()),
            access: AccessPolicy::default(),
            runs: SpellRunSettings::default(),
            connectors: None,
        }
    }

    pub fn with_llm(mut self, llm: Arc<dyn LlmGateway>) -> Self {
        self.llm = LlmSource::Fixed(llm);
        self
    }

    pub fn with_llm_config(mut self, config: LlmGatewayConfig) -> Self {
        self.llm = LlmSource::Http(config);
        self
    }

    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = embedder;
        self
    }

    pub fn with_access(mut self, access: AccessPolicy) -> Self {
        self.access = access;
        self
    }

    pub fn with_spell_runs(mut self, runs: SpellRunSettings) -> Self {
        self.runs = runs;
        self
    }

    pub fn with_connectors(mut self, connectors: ConnectorRegistry) -> Self {
        self.connectors = Some(connectors);
        self
    }
}

impl AppState {
    pub fn wire(wiring: Wiring) -> Self {
        let Wiring { registry, store, vault, llm, embedder, access, runs, connectors } = wiring;
        let llms = Arc::new(ProjectLlms::new(llm, Arc::clone(&store), Arc::clone(&vault)));
        let shared_llm = llms.shared();
        let metered: Arc<dyn LlmGateway> = Arc::new(MeteredLlmGateway::new(Arc::clone(&shared_llm), Arc::clone(&store)));
        let services = EngineServices {
            llm: Some(Arc::clone(&metered)),
            memory: Some(Arc::new(StoreMemoryBridge::new(Arc::clone(&store), Arc::clone(&embedder)))),
            knowledge: Some(Arc::new(StoreKnowledgeBridge::new(Arc::clone(&store), Arc::clone(&embedder)))),
        };
        let catalog: Arc<dyn NodeCatalog> = registry.clone();
        let engine = Arc::new(SpellEngine::new(Arc::clone(&catalog), services.clone()).with_private_http(runs.allow_private_http));
        let project_llms = Arc::clone(&llms);
        let project_llm: ProjectLlmFactory = Arc::new(move |project: ProjectId| project_llms.for_project(project));
        let scope = StoreScope { repository: Arc::clone(&store), llm: Some(shared_llm), embedder: Some(Arc::clone(&embedder)) };
        let runner = EngineReleaseRunner::scoped(catalog, services, scope)
            .with_project_llm(project_llm)
            .with_subgraph_loader(Arc::new(StoreSubgraphLoader::new(Arc::clone(&store))))
            .with_private_http(runs.allow_private_http);
        let supervisor = AgentSupervisor::with_runner(Arc::new(InMemoryEventBus::default()), Arc::new(runner))
            .with_repository(Arc::clone(&store))
            .with_run_timeout(runs.timeout);
        let supervisor = match &connectors {
            Some(registry) => supervisor.with_connectors(registry.clone()),
            None => supervisor,
        };
        let copilot = Arc::new(CopilotEngine::new(SpellValidator::new(registry.specs())).with_model(metered, "openai", copilot_model()));
        Self {
            registry,
            supervisor: Arc::new(supervisor),
            store,
            vault,
            embedder,
            engine,
            copilot,
            llms,
            access: Arc::new(access),
            runs,
            connectors: connectors.unwrap_or_default(),
            tickets: Arc::new(WsTickets::new()),
            agent_locks: Arc::new(AgentLocks::default()),
        }
    }

    pub async fn boot_enabled_agents(&self) -> BootReport {
        let mut report = BootReport::default();
        let Ok(projects) = self.store.list_projects().await else { return report };
        for project in projects {
            let Ok(agents) = self.store.list_agents(project.id).await else { continue };
            for agent in agents.into_iter().filter(|agent| agent.enabled && agent.pinned_release_id.is_some()) {
                let id = agent.id.0;
                match api::boot_agent(self, agent).await {
                    Ok(()) => report.started.push(id),
                    Err(error) => report.failed.push((id, error.message)),
                }
            }
        }
        report
    }

    pub fn in_memory(registry: Arc<NodeRegistry>, vault_key: [u8; 32]) -> Self {
        Self::wire(Wiring::new(registry, Arc::new(InMemoryRepository::new()), Arc::new(AesGcmVault::new(&vault_key))))
    }

    pub fn ephemeral_vault_key() -> [u8; 32] {
        rand::random()
    }
}

fn copilot_model() -> String {
    std::env::var("MAGICKLALA_COPILOT_MODEL").ok().filter(|model| !model.is_empty()).unwrap_or_else(|| "gpt-4o-mini".to_owned())
}

fn guarded_v1() -> Router<AppState> {
    Router::new()
        .route("/health", get(api::health_v1))
        .route("/openapi.json", get(api::openapi_document))
        .route("/node-specs", get(api::list_node_specs))
        .route("/projects", post(api::create_project).get(api::list_projects))
        .route("/spells", post(api::create_spell).get(api::list_spells))
        .route("/spells/{id}", get(api::get_spell))
        .route("/spells/{id}/graph", put(api::save_spell_graph))
        .route("/spells/{id}/validate", post(api::validate_spell))
        .route("/spells/{id}/run", post(api::run_spell))
        .route("/agents", post(api::create_agent).get(api::list_agents))
        .route("/agents/{id}", get(api::get_agent).patch(api::patch_agent).delete(api::delete_agent))
        .route("/agents/{id}/releases", post(api::publish_release).get(api::list_releases).layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES)))
        .route("/agents/{id}/start", post(api::start_agent))
        .route("/agents/{id}/stop", post(api::stop_agent))
        .route("/agents/{id}/chat", post(api::chat_with_agent))
        .route("/agents/{id}/knowledge", post(api::add_knowledge))
        .route("/agents/{id}/knowledge/search", get(api::search_knowledge))
        .route("/agents/{id}/memories", post(api::add_memory))
        .route("/agents/{id}/memories/search", get(api::search_memories))
        .route("/agents/{id}/runs", get(api::list_runs))
        .route("/credentials", post(api::create_credential).get(api::list_credentials))
        .route("/credentials/{id}", axum::routing::delete(api::delete_credential))
        .route("/webhooks", post(api::create_webhook))
        .route("/ws-ticket", post(api::create_ws_ticket))
        .route("/copilot/suggest", post(api::copilot_suggest))
}

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/nodes", get(api::list_node_specs))
        .route("/ws", get(ws::websocket))
        .nest("/api/v1", guarded_v1())
        .route_layer(middleware::from_fn_with_state(state.clone(), access::require_token));
    let [health_path, webhook_path] = PUBLIC_PATHS;
    let guarded_delete = delete(api::delete_webhook).route_layer(middleware::from_fn_with_state(state.clone(), access::require_token));
    let webhooks = Router::new()
        .route(&access::axum_route(webhook_path), post(api::receive_webhook).merge(guarded_delete))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES));
    let cors = access::cors_layer(&state.access);
    let trace = TraceLayer::new_for_http()
        .make_span_with(|request: &Request<_>| tracing::info_span!("request", method = %request.method(), path = %request.uri().path()));
    Router::new()
        .route(health_path, get(api::health))
        .merge(webhooks)
        .merge(protected)
        .fallback(api::route_not_found)
        .layer(trace)
        .layer(cors)
        .layer(middleware::from_fn_with_state(state.clone(), access::require_known_host))
        .with_state(state)
}

pub async fn serve(config: ServerConfig, state: AppState) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    serve_on(listener, state).await
}

pub async fn serve_on(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    axum::serve(listener, build_router(state)).with_graceful_shutdown(shutdown_signal()).await
}

async fn shutdown_signal() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}
