mod discord;
mod http;
mod plugin;
mod rejection;
mod signature;
mod slack;
mod state;
mod studio;
mod tick;
mod webhook;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

pub use discord::{DISCORD_DEFAULT_BASE_URL, DISCORD_MESSAGE_LIMIT, DiscordConnector, DiscordInbound};
pub use plugin::{ExternalPluginSpec, JsonRpcRequest, PluginCapability, PluginRegistry, PluginTransport};
pub use rejection::InboundRejection;
pub use signature::{sign_hmac_sha256, verify_hmac_sha256};
pub use slack::{SLACK_DEFAULT_BASE_URL, SLACK_MAX_CLOCK_SKEW_SECONDS, SlackConnector, SlackInbound};
pub use studio::{StudioChatConnector, StudioReply};
pub use tick::{MIN_TICK_INTERVAL_MS, TickConnector};
pub use webhook::{MAX_WEBHOOK_BODY_BYTES, WEBHOOK_SIGNATURE_HEADER, WebhookConnector};

#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("connector {0} is not registered")]
    Unknown(Uuid),
    #[error("connector misconfigured: {0}")]
    Config(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("connector is not running")]
    NotRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    Studio,
    Webhook,
    Tick,
    Rest,
    Discord,
    Slack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorConfig {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub kind: ConnectorKind,
    pub enabled: bool,
    pub credential_id: Option<Uuid>,
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ConnectorStatus {
    Stopped,
    Starting,
    Running,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundEvent {
    pub connector_id: Uuid,
    pub agent_id: Uuid,
    pub kind: ConnectorKind,
    pub channel: String,
    pub sender: String,
    pub content: String,
    pub payload: serde_json::Value,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboundAction {
    pub connector_id: Uuid,
    pub channel: String,
    pub content: String,
    pub payload: serde_json::Value,
}

#[async_trait]
pub trait Connector: Send + Sync {
    fn config(&self) -> &ConnectorConfig;
    fn status(&self) -> ConnectorStatus;
    async fn start(&self, inbound: mpsc::Sender<InboundEvent>) -> Result<(), ConnectorError>;
    async fn stop(&self) -> Result<(), ConnectorError>;
    async fn send(&self, action: OutboundAction) -> Result<(), ConnectorError>;
}

#[derive(Clone, Default)]
pub struct ConnectorRegistry {
    connectors: HashMap<Uuid, Arc<dyn Connector>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, connector: Arc<dyn Connector>) -> Option<Arc<dyn Connector>> {
        self.connectors.insert(connector.config().id, connector)
    }

    pub fn remove(&mut self, id: Uuid) -> Option<Arc<dyn Connector>> {
        self.connectors.remove(&id)
    }

    pub fn get(&self, id: Uuid) -> Option<Arc<dyn Connector>> {
        self.connectors.get(&id).cloned()
    }

    pub fn for_agent(&self, agent_id: Uuid) -> Vec<Arc<dyn Connector>> {
        self.connectors.values().filter(|connector| connector.config().agent_id == agent_id).cloned().collect()
    }

    pub async fn dispatch(&self, action: OutboundAction) -> Result<(), ConnectorError> {
        let connector = self.get(action.connector_id).ok_or(ConnectorError::Unknown(action.connector_id))?;
        connector.send(action).await
    }
}
