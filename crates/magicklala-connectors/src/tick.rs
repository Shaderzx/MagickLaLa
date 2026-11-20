use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

use crate::state::ConnectorRuntimeState;
use crate::{Connector, ConnectorConfig, ConnectorError, ConnectorStatus, InboundEvent, OutboundAction};

pub const MIN_TICK_INTERVAL_MS: u64 = 10;

pub struct TickConnector {
    config: ConnectorConfig,
    interval: Duration,
    state: ConnectorRuntimeState,
}

impl TickConnector {
    pub fn new(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let interval_ms = config
            .settings
            .get("interval_ms")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| ConnectorError::Config("tick connector needs a numeric interval_ms setting".to_owned()))?;
        if interval_ms < MIN_TICK_INTERVAL_MS {
            return Err(ConnectorError::Config(format!("interval_ms must be at least {MIN_TICK_INTERVAL_MS}")));
        }
        Ok(Self { config, interval: Duration::from_millis(interval_ms), state: ConnectorRuntimeState::default() })
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    fn tick_event(&self, sequence: u64) -> InboundEvent {
        let interval_ms = u64::try_from(self.interval.as_millis()).unwrap_or(u64::MAX);
        InboundEvent {
            connector_id: self.config.id,
            agent_id: self.config.agent_id,
            kind: self.config.kind,
            channel: crate::http::setting_str(&self.config.settings, "channel").unwrap_or_else(|| "tick".to_owned()),
            sender: "scheduler".to_owned(),
            content: String::new(),
            payload: serde_json::json!({ "sequence": sequence, "interval_ms": interval_ms }),
            received_at: Utc::now(),
        }
    }
}

#[async_trait]
impl Connector for TickConnector {
    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn status(&self) -> ConnectorStatus {
        self.state.status()
    }

    async fn start(&self, inbound: mpsc::Sender<InboundEvent>) -> Result<(), ConnectorError> {
        let cancel = self.state.begin(None)?;
        let template = self.tick_event(0);
        let period = self.interval;
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut sequence = 0_u64;
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        sequence += 1;
                        let mut event = template.clone();
                        event.received_at = Utc::now();
                        event.payload["sequence"] = serde_json::json!(sequence);
                        tokio::select! {
                            () = cancel.cancelled() => break,
                            delivered = inbound.send(event) => if delivered.is_err() { break },
                        }
                    }
                }
            }
        });
        self.state.attach_task(task);
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.state.end().await;
        Ok(())
    }

    async fn send(&self, _action: OutboundAction) -> Result<(), ConnectorError> {
        Err(ConnectorError::Config("tick connector cannot send outbound messages".to_owned()))
    }
}
