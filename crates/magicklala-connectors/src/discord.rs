use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

use crate::http::{http_client, reject_unsuccessful, require_secret, setting_str, transport_failure};
use crate::state::ConnectorRuntimeState;
use crate::{Connector, ConnectorConfig, ConnectorError, ConnectorStatus, InboundEvent, OutboundAction};

pub const DISCORD_DEFAULT_BASE_URL: &str = "https://discord.com/api/v10";
pub const DISCORD_MESSAGE_LIMIT: usize = 2_000;

#[derive(Debug, Clone, PartialEq)]
pub enum DiscordInbound {
    Ping,
    Event(InboundEvent),
    Ignored,
}

pub struct DiscordConnector {
    config: ConnectorConfig,
    bot_token: Option<String>,
    base_url: String,
    state: ConnectorRuntimeState,
    client: reqwest::Client,
}

impl DiscordConnector {
    pub fn new(config: ConnectorConfig, bot_token: Option<String>) -> Self {
        let base_url = setting_str(&config.settings, "base_url").unwrap_or_else(|| DISCORD_DEFAULT_BASE_URL.to_owned()).trim_end_matches('/').to_owned();
        Self { config, bot_token, base_url, state: ConnectorRuntimeState::default(), client: http_client() }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn adapt(&self, payload: serde_json::Value) -> DiscordInbound {
        match payload.get("type").and_then(serde_json::Value::as_u64) {
            Some(1) => return DiscordInbound::Ping,
            Some(2) => return self.adapt_interaction(&payload).map_or(DiscordInbound::Ignored, DiscordInbound::Event),
            _ => {}
        }
        let message = payload.get("d").filter(|_| payload.get("t").and_then(serde_json::Value::as_str) == Some("MESSAGE_CREATE")).unwrap_or(&payload);
        if message.pointer("/author/bot").and_then(serde_json::Value::as_bool) == Some(true) {
            return DiscordInbound::Ignored;
        }
        let (Some(channel), Some(content)) = (setting_str(message, "channel_id"), setting_str(message, "content")) else {
            return DiscordInbound::Ignored;
        };
        let sender = message.pointer("/author/id").and_then(serde_json::Value::as_str).unwrap_or("unknown").to_owned();
        DiscordInbound::Event(self.event(channel, sender, content, payload.clone()))
    }

    pub async fn receive(&self, payload: serde_json::Value) -> Result<DiscordInbound, ConnectorError> {
        let adapted = self.adapt(payload);
        if let DiscordInbound::Event(event) = &adapted {
            self.state.emit(event.clone()).await?;
        }
        Ok(adapted)
    }

    fn adapt_interaction(&self, payload: &serde_json::Value) -> Option<InboundEvent> {
        let channel = setting_str(payload, "channel_id")?;
        let sender =
            payload.pointer("/member/user/id").or_else(|| payload.pointer("/user/id")).and_then(serde_json::Value::as_str).unwrap_or("unknown").to_owned();
        let name = payload.pointer("/data/name").and_then(serde_json::Value::as_str).unwrap_or_default();
        let arguments: Vec<String> = payload
            .pointer("/data/options")
            .and_then(serde_json::Value::as_array)
            .map(|options| {
                options.iter().filter_map(|option| option.get("value")).map(|value| value.as_str().map_or_else(|| value.to_string(), str::to_owned)).collect()
            })
            .unwrap_or_default();
        let content = std::iter::once(format!("/{name}")).chain(arguments).collect::<Vec<_>>().join(" ");
        Some(self.event(channel, sender, content, payload.clone()))
    }

    fn event(&self, channel: String, sender: String, content: String, payload: serde_json::Value) -> InboundEvent {
        InboundEvent {
            connector_id: self.config.id,
            agent_id: self.config.agent_id,
            kind: self.config.kind,
            channel,
            sender,
            content,
            payload,
            received_at: Utc::now(),
        }
    }
}

#[async_trait]
impl Connector for DiscordConnector {
    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn status(&self) -> ConnectorStatus {
        self.state.status()
    }

    async fn start(&self, inbound: mpsc::Sender<InboundEvent>) -> Result<(), ConnectorError> {
        require_secret(&self.bot_token, "discord bot token")?;
        self.state.begin(Some(inbound)).map(|_| ())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.state.end().await;
        Ok(())
    }

    async fn send(&self, action: OutboundAction) -> Result<(), ConnectorError> {
        let token = require_secret(&self.bot_token, "discord bot token")?;
        if action.channel.is_empty() || !action.channel.chars().all(|character| character.is_ascii_digit()) {
            return Err(ConnectorError::Config("discord channel must be a numeric snowflake id".to_owned()));
        }
        if action.content.chars().count() > DISCORD_MESSAGE_LIMIT {
            return Err(ConnectorError::Config(format!("discord messages are limited to {DISCORD_MESSAGE_LIMIT} characters")));
        }
        let url = format!("{}/channels/{}/messages", self.base_url, action.channel);
        let response = self
            .client
            .post(url)
            .header(reqwest::header::AUTHORIZATION, format!("Bot {token}"))
            .json(&serde_json::json!({ "content": action.content }))
            .send()
            .await
            .map_err(|error| transport_failure("discord", error))?;
        reject_unsuccessful("discord", response).await.map(|_| ())
    }
}
