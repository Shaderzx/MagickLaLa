use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

use crate::http::{http_client, reject_unsuccessful, require_secret, setting_str, transport_failure};
use crate::signature::verify_hmac_sha256;
use crate::state::ConnectorRuntimeState;
use crate::{Connector, ConnectorConfig, ConnectorError, ConnectorStatus, InboundEvent, InboundRejection, OutboundAction};

pub const SLACK_DEFAULT_BASE_URL: &str = "https://slack.com/api";
pub const SLACK_MAX_CLOCK_SKEW_SECONDS: i64 = 300;

#[derive(Debug, Clone, PartialEq)]
pub enum SlackInbound {
    UrlVerification { challenge: String },
    Event(InboundEvent),
    Ignored,
}

pub struct SlackConnector {
    config: ConnectorConfig,
    bot_token: Option<String>,
    signing_secret: Option<String>,
    base_url: String,
    state: ConnectorRuntimeState,
    client: reqwest::Client,
}

impl SlackConnector {
    pub fn new(config: ConnectorConfig, bot_token: Option<String>, signing_secret: Option<String>) -> Self {
        let base_url = setting_str(&config.settings, "base_url").unwrap_or_else(|| SLACK_DEFAULT_BASE_URL.to_owned()).trim_end_matches('/').to_owned();
        Self { config, bot_token, signing_secret, base_url, state: ConnectorRuntimeState::default(), client: http_client() }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn verify(&self, body: &[u8], timestamp: &str, signature: &str) -> Result<(), InboundRejection> {
        let secret = require_secret(&self.signing_secret, "slack signing secret").map_err(|error| InboundRejection::Unauthorized(error.to_string()))?;
        let sent_at: i64 = timestamp.trim().parse().map_err(|_| InboundRejection::Unauthorized("slack request timestamp is not a number".to_owned()))?;
        if (Utc::now().timestamp() - sent_at).abs() > SLACK_MAX_CLOCK_SKEW_SECONDS {
            return Err(InboundRejection::Unauthorized("slack request timestamp is outside the replay window".to_owned()));
        }
        let digest = signature.trim().strip_prefix("v0=").ok_or_else(|| InboundRejection::Unauthorized("slack signature must start with v0=".to_owned()))?;
        let mut basestring = format!("v0:{}:", timestamp.trim()).into_bytes();
        basestring.extend_from_slice(body);
        if verify_hmac_sha256(secret.as_bytes(), &basestring, digest) {
            Ok(())
        } else {
            Err(InboundRejection::Unauthorized("slack signature does not match the payload".to_owned()))
        }
    }

    pub fn parse(&self, body: &[u8], timestamp: &str, signature: &str) -> Result<SlackInbound, InboundRejection> {
        self.verify(body, timestamp, signature)?;
        let payload: serde_json::Value = serde_json::from_slice(body).map_err(|error| InboundRejection::Invalid(format!("slack body is not JSON: {error}")))?;
        match payload.get("type").and_then(serde_json::Value::as_str) {
            Some("url_verification") => {
                let challenge =
                    setting_str(&payload, "challenge").ok_or_else(|| InboundRejection::Invalid("slack url_verification has no challenge".to_owned()))?;
                Ok(SlackInbound::UrlVerification { challenge })
            }
            Some("event_callback") => Ok(self.adapt_event(&payload).map_or(SlackInbound::Ignored, SlackInbound::Event)),
            _ => Ok(SlackInbound::Ignored),
        }
    }

    pub async fn receive(&self, body: &[u8], timestamp: &str, signature: &str) -> Result<SlackInbound, InboundRejection> {
        let parsed = self.parse(body, timestamp, signature)?;
        if let SlackInbound::Event(event) = &parsed {
            self.state.emit(event.clone()).await.map_err(|_| InboundRejection::NotRunning)?;
        }
        Ok(parsed)
    }

    fn adapt_event(&self, payload: &serde_json::Value) -> Option<InboundEvent> {
        let event = payload.get("event")?;
        let kind = event.get("type").and_then(serde_json::Value::as_str)?;
        if !matches!(kind, "message" | "app_mention") || event.get("bot_id").is_some() || event.get("subtype").is_some() {
            return None;
        }
        Some(InboundEvent {
            connector_id: self.config.id,
            agent_id: self.config.agent_id,
            kind: self.config.kind,
            channel: setting_str(event, "channel")?,
            sender: setting_str(event, "user").unwrap_or_else(|| "unknown".to_owned()),
            content: setting_str(event, "text").unwrap_or_default(),
            payload: payload.clone(),
            received_at: Utc::now(),
        })
    }
}

#[async_trait]
impl Connector for SlackConnector {
    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn status(&self) -> ConnectorStatus {
        self.state.status()
    }

    async fn start(&self, inbound: mpsc::Sender<InboundEvent>) -> Result<(), ConnectorError> {
        require_secret(&self.bot_token, "slack bot token")?;
        self.state.begin(Some(inbound)).map(|_| ())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.state.end().await;
        Ok(())
    }

    async fn send(&self, action: OutboundAction) -> Result<(), ConnectorError> {
        let token = require_secret(&self.bot_token, "slack bot token")?;
        if action.channel.is_empty() {
            return Err(ConnectorError::Config("slack channel is required".to_owned()));
        }
        let mut body = serde_json::json!({ "channel": action.channel, "text": action.content });
        if let Some(thread) = action.payload.get("thread_ts").and_then(serde_json::Value::as_str) {
            body["thread_ts"] = serde_json::json!(thread);
        }
        let response = self
            .client
            .post(format!("{}/chat.postMessage", self.base_url))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| transport_failure("slack", error))?;
        let reply = reject_unsuccessful("slack", response).await?;
        if reply.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            Ok(())
        } else {
            let reason = reply.get("error").and_then(serde_json::Value::as_str).unwrap_or("unknown_error");
            Err(ConnectorError::Transport(format!("slack rejected chat.postMessage: {reason}")))
        }
    }
}
