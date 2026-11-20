use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{broadcast, mpsc};

use crate::state::ConnectorRuntimeState;
use crate::{Connector, ConnectorConfig, ConnectorError, ConnectorStatus, InboundEvent, OutboundAction};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StudioReply {
    Chunk { channel: String, delta: String },
    Message { action: OutboundAction },
}

pub struct StudioChatConnector {
    config: ConnectorConfig,
    state: ConnectorRuntimeState,
    replies: broadcast::Sender<StudioReply>,
}

impl StudioChatConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        let (replies, _) = broadcast::channel(1_024);
        Self { config, state: ConnectorRuntimeState::default(), replies }
    }

    pub fn subscribe_replies(&self) -> broadcast::Receiver<StudioReply> {
        self.replies.subscribe()
    }

    pub async fn submit(&self, channel: &str, sender: &str, content: &str) -> Result<InboundEvent, ConnectorError> {
        if content.trim().is_empty() {
            return Err(ConnectorError::Config("chat message is empty".to_owned()));
        }
        let event = InboundEvent {
            connector_id: self.config.id,
            agent_id: self.config.agent_id,
            kind: self.config.kind,
            channel: channel.to_owned(),
            sender: sender.to_owned(),
            content: content.to_owned(),
            payload: serde_json::json!({ "source": "studio" }),
            received_at: Utc::now(),
        };
        self.state.emit(event.clone()).await?;
        Ok(event)
    }

    pub fn stream_chunk(&self, channel: &str, delta: &str) -> Result<(), ConnectorError> {
        if !self.state.is_running() {
            return Err(ConnectorError::NotRunning);
        }
        let _listeners = self.replies.send(StudioReply::Chunk { channel: channel.to_owned(), delta: delta.to_owned() }).unwrap_or_default();
        Ok(())
    }
}

#[async_trait]
impl Connector for StudioChatConnector {
    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn status(&self) -> ConnectorStatus {
        self.state.status()
    }

    async fn start(&self, inbound: mpsc::Sender<InboundEvent>) -> Result<(), ConnectorError> {
        self.state.begin(Some(inbound)).map(|_| ())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.state.end().await;
        Ok(())
    }

    async fn send(&self, action: OutboundAction) -> Result<(), ConnectorError> {
        if !self.state.is_running() {
            return Err(ConnectorError::NotRunning);
        }
        let _listeners = self.replies.send(StudioReply::Message { action }).unwrap_or_default();
        Ok(())
    }
}
