use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use magicklala_graph::{NodeCatalog, NodeSpec};
use magicklala_nodes::NodeRegistry;
use magicklala_runtime::AgentSupervisor;

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
}

pub fn build_router(state: AppState) -> Router {
    Router::new().route("/health", get(health)).route("/api/nodes", get(node_specs)).with_state(state)
}

pub async fn serve(config: ServerConfig, state: AppState) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    axum::serve(listener, build_router(state)).await
}

async fn health() -> &'static str {
    "ok"
}

async fn node_specs(State(state): State<AppState>) -> Json<Vec<NodeSpec>> {
    Json(state.registry.specs())
}
