mod copilot;

use std::collections::HashMap;
use std::sync::Arc;

use magicklala_connectors::{InboundEvent, OutboundAction};
use magicklala_graph::TraceEvent;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

pub use copilot::{CopilotRequest, CopilotSuggestion, GraphPatchOp};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("agent {0} is unknown")]
    UnknownAgent(Uuid),
    #[error("agent {0} is already running")]
    AlreadyRunning(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum AgentStatus {
    Stopped,
    Starting,
    Running,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    AgentStatusChanged { agent_id: Uuid, status: AgentStatus },
    Inbound { event: InboundEvent },
    Outbound { action: OutboundAction },
    Trace { agent_id: Option<Uuid>, event: TraceEvent },
}

pub trait EventBus: Send + Sync {
    fn publish(&self, event: RuntimeEvent);
    fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent>;
}

pub struct InMemoryEventBus {
    sender: broadcast::Sender<RuntimeEvent>,
}

impl InMemoryEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new(1_024)
    }
}

impl EventBus for InMemoryEventBus {
    fn publish(&self, event: RuntimeEvent) {
        let _delivered = self.sender.send(event).unwrap_or_default();
    }

    fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.sender.subscribe()
    }
}

pub struct AgentSupervisor {
    bus: Arc<dyn EventBus>,
    agents: RwLock<HashMap<Uuid, AgentStatus>>,
}

impl AgentSupervisor {
    pub fn new(bus: Arc<dyn EventBus>) -> Self {
        Self { bus, agents: RwLock::new(HashMap::new()) }
    }

    pub fn bus(&self) -> Arc<dyn EventBus> {
        Arc::clone(&self.bus)
    }

    pub async fn register(&self, agent_id: Uuid) {
        self.agents.write().await.entry(agent_id).or_insert(AgentStatus::Stopped);
    }

    pub async fn start(&self, agent_id: Uuid) -> Result<(), RuntimeError> {
        self.transition(agent_id, AgentStatus::Running, |current| match current {
            AgentStatus::Running | AgentStatus::Starting => Err(RuntimeError::AlreadyRunning(agent_id)),
            _ => Ok(()),
        })
        .await
    }

    pub async fn stop(&self, agent_id: Uuid) -> Result<(), RuntimeError> {
        self.transition(agent_id, AgentStatus::Stopped, |_| Ok(())).await
    }

    pub async fn status(&self, agent_id: Uuid) -> Option<AgentStatus> {
        self.agents.read().await.get(&agent_id).cloned()
    }

    pub async fn list(&self) -> Vec<(Uuid, AgentStatus)> {
        let mut agents: Vec<_> = self.agents.read().await.iter().map(|(id, status)| (*id, status.clone())).collect();
        agents.sort_by_key(|(id, _)| *id);
        agents
    }

    async fn transition(&self, agent_id: Uuid, next: AgentStatus, allowed: impl FnOnce(&AgentStatus) -> Result<(), RuntimeError>) -> Result<(), RuntimeError> {
        {
            let mut agents = self.agents.write().await;
            let current = agents.get_mut(&agent_id).ok_or(RuntimeError::UnknownAgent(agent_id))?;
            allowed(current)?;
            *current = next.clone();
        }
        self.bus.publish(RuntimeEvent::AgentStatusChanged { agent_id, status: next });
        Ok(())
    }
}
