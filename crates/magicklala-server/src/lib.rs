mod api;
mod embedding;
mod error;
mod memory;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post, put};
use magicklala_nodes::NodeRegistry;
use magicklala_runtime::AgentSupervisor;
use magicklala_store::{AesGcmVault, CryptoVault, Repository};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub use api::ApiDoc;
pub use embedding::{HashingEmbedder, cosine};
pub use error::{ApiError, ApiResult};
pub use memory::{InMemoryRepository, content_hash, graph_hash};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub database_url: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { bind: SocketAddr::from(([127, 0, 0, 1], 7777)), database_url: "sqlite://magicklala.db".to_owned() }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<NodeRegistry>,
    pub supervisor: Arc<AgentSupervisor>,
    pub store: Arc<dyn Repository>,
    pub vault: Arc<dyn CryptoVault>,
}

impl AppState {
    pub fn new(registry: Arc<NodeRegistry>, supervisor: Arc<AgentSupervisor>, store: Arc<dyn Repository>, vault: Arc<dyn CryptoVault>) -> Self {
        Self { registry, supervisor, store, vault }
    }

    pub fn in_memory(registry: Arc<NodeRegistry>, supervisor: Arc<AgentSupervisor>, vault_key: [u8; 32]) -> Self {
        Self::new(registry, supervisor, Arc::new(InMemoryRepository::new()), Arc::new(AesGcmVault::new(&vault_key)))
    }

    pub fn ephemeral_vault_key() -> [u8; 32] {
        rand::random()
    }
}

pub fn build_router(state: AppState) -> Router {
    let v1 = Router::new()
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
        .route("/agents/{id}/releases", post(api::publish_release).get(api::list_releases))
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
        .route("/webhooks/{*path}", post(api::receive_webhook))
        .route("/copilot/suggest", post(api::copilot_suggest));
    Router::new()
        .route("/health", get(api::health))
        .route("/api/nodes", get(api::list_node_specs))
        .route("/ws", get(ws::websocket))
        .nest("/api/v1", v1)
        .fallback(api::route_not_found)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
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
