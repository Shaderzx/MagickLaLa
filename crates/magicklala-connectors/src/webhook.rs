use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

use crate::http::{http_client, reject_unsuccessful, setting_str, transport_failure};
use crate::signature::verify_hmac_sha256;
use crate::state::ConnectorRuntimeState;
use crate::{Connector, ConnectorConfig, ConnectorError, ConnectorStatus, InboundEvent, InboundRejection, OutboundAction};

pub const WEBHOOK_SIGNATURE_HEADER: &str = "X-MagickLaLa-Signature";
pub const WEBHOOK_TIMESTAMP_HEADER: &str = "X-MagickLaLa-Timestamp";
pub const WEBHOOK_MAX_SKEW_SECS: i64 = 300;
pub const MAX_WEBHOOK_BODY_BYTES: usize = 1024 * 1024;

pub fn signed_webhook_message(timestamp: i64, body: &[u8]) -> Vec<u8> {
    let mut message = format!("{timestamp}.").into_bytes();
    message.extend_from_slice(body);
    message
}

pub fn parse_webhook_timestamp(raw: Option<&str>) -> Result<i64, String> {
    let raw = raw.map(str::trim).filter(|value| !value.is_empty()).ok_or_else(|| format!("missing {WEBHOOK_TIMESTAMP_HEADER} header"))?;
    raw.parse::<i64>().map_err(|_| format!("{WEBHOOK_TIMESTAMP_HEADER} must be unix seconds, got {raw:?}"))
}

pub struct WebhookConnector {
    config: ConnectorConfig,
    secret: Option<String>,
    callback_url: Option<String>,
    state: ConnectorRuntimeState,
    client: reqwest::Client,
}

impl WebhookConnector {
    pub fn new(config: ConnectorConfig, secret: Option<String>) -> Self {
        let callback_url = setting_str(&config.settings, "callback_url");
        Self { config, secret: secret.filter(|value| !value.is_empty()), callback_url, state: ConnectorRuntimeState::default(), client: http_client() }
    }

    pub fn path(&self) -> String {
        setting_str(&self.config.settings, "path").unwrap_or_else(|| format!("/hooks/{}", self.config.id))
    }

    pub fn verify(&self, body: &[u8], signature: Option<&str>) -> Result<(), InboundRejection> {
        let Some(secret) = &self.secret else {
            return Ok(());
        };
        let provided =
            signature.map(str::trim).ok_or_else(|| InboundRejection::Unauthorized(format!("missing {WEBHOOK_SIGNATURE_HEADER} signature header")))?;
        let digest = provided.strip_prefix("sha256=").unwrap_or(provided);
        if verify_hmac_sha256(secret.as_bytes(), body, digest) {
            Ok(())
        } else {
            Err(InboundRejection::Unauthorized(format!("{WEBHOOK_SIGNATURE_HEADER} signature does not match the payload")))
        }
    }

    pub fn verify_timestamped(secret: &str, timestamp: i64, body: &[u8], signature: &str, now_unix: i64, max_skew_secs: i64) -> Result<(), String> {
        let skew = now_unix.abs_diff(timestamp);
        if skew > max_skew_secs.unsigned_abs() {
            return Err(format!("{WEBHOOK_TIMESTAMP_HEADER} timestamp is {skew} s away from server time; the limit is {max_skew_secs} s"));
        }
        let provided = signature.trim();
        let digest = provided.strip_prefix("sha256=").unwrap_or(provided);
        if verify_hmac_sha256(secret.as_bytes(), &signed_webhook_message(timestamp, body), digest) {
            Ok(())
        } else {
            Err(format!("{WEBHOOK_SIGNATURE_HEADER} signature does not match timestamp.body"))
        }
    }

    pub fn parse(&self, body: &[u8], signature: Option<&str>) -> Result<InboundEvent, InboundRejection> {
        if body.len() > MAX_WEBHOOK_BODY_BYTES {
            return Err(InboundRejection::Invalid(format!("webhook body exceeds {MAX_WEBHOOK_BODY_BYTES} bytes")));
        }
        self.verify(body, signature)?;
        let payload: serde_json::Value =
            serde_json::from_slice(body).map_err(|error| InboundRejection::Invalid(format!("webhook body is not JSON: {error}")))?;
        let field = |keys: &[&str], fallback: &str| keys.iter().find_map(|key| setting_str(&payload, key)).unwrap_or_else(|| fallback.to_owned());
        Ok(InboundEvent {
            connector_id: self.config.id,
            agent_id: self.config.agent_id,
            kind: self.config.kind,
            channel: field(&["channel"], "webhook"),
            sender: field(&["sender", "user"], "webhook"),
            content: field(&["content", "text", "message"], ""),
            payload: payload.clone(),
            received_at: Utc::now(),
        })
    }

    pub async fn receive(&self, body: &[u8], signature: Option<&str>) -> Result<InboundEvent, InboundRejection> {
        let event = self.parse(body, signature)?;
        self.state.emit(event.clone()).await.map_err(|_| InboundRejection::NotRunning)?;
        Ok(event)
    }
}

#[async_trait]
impl Connector for WebhookConnector {
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
        let url = self.callback_url.clone().ok_or_else(|| ConnectorError::Config("webhook has no callback_url for outbound replies".to_owned()))?;
        let response = self.client.post(url).json(&action).send().await.map_err(|error| transport_failure("webhook callback", error))?;
        reject_unsuccessful("webhook callback", response).await.map(|_| ())
    }
}
