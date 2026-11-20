mod actor;
mod bridges;
mod copilot;
mod metered;
mod runner;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use magicklala_connectors::{ConnectorRegistry, InboundEvent, OutboundAction};
use magicklala_graph::{RunId, RunStatus, SpellGraph, TraceEvent};
use magicklala_store::Repository;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

pub use actor::{AgentRunStats, CHANNEL_LANE_CAPACITY, CHANNEL_LANE_IDLE, DEFAULT_RUN_TIMEOUT, DispatchError, MAILBOX_CAPACITY};
pub use bridges::{SharedEmbedder, StoreKnowledgeBridge, StoreMemoryBridge};
pub use copilot::{CopilotEngine, CopilotError, CopilotRequest, CopilotSuggestion, GraphPatchOp, PatchRejection};
pub use metered::{LlmCallScope, MeteredLlmGateway};
pub use runner::{
    AgentRelease, CORRELATION_ID_KEY, EngineReleaseRunner, ReleaseOrigin, ReleaseRunner, RunScope, StoreScope, TemplateReplyRunner, correlation_id,
    inbound_payload, ordered_replies, structural_errors, trigger_kinds, trigger_payload,
};

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
    RunCompleted { agent_id: Uuid, run_id: RunId, correlation_id: Option<String>, status: RunStatus, reply: Option<String>, error: Option<String> },
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
    runner: Arc<dyn ReleaseRunner>,
    connectors: Option<ConnectorRegistry>,
    run_timeout: Duration,
    releases: RwLock<HashMap<Uuid, Arc<AgentRelease>>>,
    actors: RwLock<HashMap<Uuid, actor::ActorHandle>>,
    stats: Arc<Mutex<HashMap<Uuid, AgentRunStats>>>,
    repository: Option<Arc<dyn Repository>>,
}

impl AgentSupervisor {
    pub fn new(bus: Arc<dyn EventBus>) -> Self {
        Self::with_runner(bus, Arc::new(TemplateReplyRunner))
    }

    pub fn with_runner(bus: Arc<dyn EventBus>, runner: Arc<dyn ReleaseRunner>) -> Self {
        Self {
            bus,
            agents: RwLock::new(HashMap::new()),
            runner,
            connectors: None,
            run_timeout: DEFAULT_RUN_TIMEOUT,
            releases: RwLock::new(HashMap::new()),
            actors: RwLock::new(HashMap::new()),
            stats: Arc::new(Mutex::new(HashMap::new())),
            repository: None,
        }
    }

    pub fn with_connectors(mut self, connectors: ConnectorRegistry) -> Self {
        self.connectors = Some(connectors);
        self
    }

    pub fn with_run_timeout(mut self, run_timeout: Duration) -> Self {
        self.run_timeout = run_timeout;
        self
    }

    pub fn with_repository(mut self, repository: Arc<dyn Repository>) -> Self {
        self.set_repository(repository);
        self
    }

    pub fn set_repository(&mut self, repository: Arc<dyn Repository>) {
        self.repository = Some(repository);
    }

    pub async fn deploy_release(&self, agent_id: Uuid, graph: SpellGraph) -> Result<u32, DispatchError> {
        self.deploy(agent_id, graph, None).await
    }

    pub async fn deploy_release_from(&self, agent_id: Uuid, graph: SpellGraph, origin: ReleaseOrigin) -> Result<u32, DispatchError> {
        self.deploy(agent_id, graph, Some(origin)).await
    }

    async fn deploy(&self, agent_id: Uuid, graph: SpellGraph, origin: Option<ReleaseOrigin>) -> Result<u32, DispatchError> {
        if self.status(agent_id).await.is_none() {
            return Err(RuntimeError::UnknownAgent(agent_id).into());
        }
        self.runner.validate(&graph).map_err(|errors| DispatchError::InvalidRelease(errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")))?;
        let mut releases = self.releases.write().await;
        let version = releases.get(&agent_id).map_or(1, |release| release.version.saturating_add(1));
        releases.insert(agent_id, Arc::new(actor::new_release(agent_id, version, graph, origin)));
        Ok(version)
    }

    pub async fn release(&self, agent_id: Uuid) -> Option<Arc<AgentRelease>> {
        self.releases.read().await.get(&agent_id).cloned()
    }

    pub async fn dispatch_inbound(&self, event: InboundEvent) -> Result<(), DispatchError> {
        let agent_id = event.agent_id;
        match self.status(agent_id).await {
            None => return Err(RuntimeError::UnknownAgent(agent_id).into()),
            Some(AgentStatus::Running) => {}
            Some(_) => return Err(DispatchError::NotRunning(agent_id)),
        }
        let release = self.release(agent_id).await.ok_or(DispatchError::NoRelease(agent_id))?;
        self.bus.publish(RuntimeEvent::Inbound { event: event.clone() });
        let mailbox = self.actors.read().await.get(&agent_id).map(actor::ActorHandle::mailbox).ok_or(DispatchError::NotRunning(agent_id))?;
        mailbox.try_send(actor::QueuedEvent::new(event, release)).map_err(|error| match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => DispatchError::Busy(agent_id),
            tokio::sync::mpsc::error::TrySendError::Closed(_) => DispatchError::NotRunning(agent_id),
        })
    }

    pub fn run_stats(&self, agent_id: Uuid) -> AgentRunStats {
        self.stats.lock().unwrap_or_else(PoisonError::into_inner).get(&agent_id).cloned().unwrap_or_default()
    }

    pub fn bus(&self) -> Arc<dyn EventBus> {
        Arc::clone(&self.bus)
    }

    pub async fn register(&self, agent_id: Uuid) {
        self.agents.write().await.entry(agent_id).or_insert(AgentStatus::Stopped);
    }

    pub async fn start(&self, agent_id: Uuid) -> Result<(), RuntimeError> {
        let mut actors = self.actors.write().await;
        self.transition(agent_id, AgentStatus::Running, |current| match current {
            AgentStatus::Running | AgentStatus::Starting => Err(RuntimeError::AlreadyRunning(agent_id)),
            _ => Ok(()),
        })
        .await?;
        let context = actor::ActorContext {
            agent_id,
            bus: Arc::clone(&self.bus),
            runner: Arc::clone(&self.runner),
            connectors: self.connectors.clone(),
            stats: Arc::clone(&self.stats),
            run_timeout: self.run_timeout,
            repository: self.repository.clone(),
        };
        let previous = actors.insert(agent_id, actor::spawn_actor(Arc::new(context)));
        drop(actors);
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
        Ok(())
    }

    pub async fn stop(&self, agent_id: Uuid) -> Result<(), RuntimeError> {
        let handle = {
            let mut actors = self.actors.write().await;
            self.transition(agent_id, AgentStatus::Stopped, |_| Ok(())).await?;
            actors.remove(&agent_id)
        };
        if let Some(handle) = handle {
            handle.shutdown().await;
        }
        Ok(())
    }

    pub async fn forget(&self, agent_id: Uuid) {
        let handle = self.actors.write().await.remove(&agent_id);
        if let Some(handle) = handle {
            handle.shutdown().await;
        }
        let known = self.agents.write().await.remove(&agent_id).is_some();
        self.releases.write().await.remove(&agent_id);
        self.stats.lock().unwrap_or_else(PoisonError::into_inner).remove(&agent_id);
        if known {
            self.bus.publish(RuntimeEvent::AgentStatusChanged { agent_id, status: AgentStatus::Stopped });
        }
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
