use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::FutureExt;
use magicklala_connectors::{ConnectorRegistry, InboundEvent};
use magicklala_graph::{EventEmitter, SpellGraph, TraceEvent};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::runner::{AgentRelease, ReleaseRunner};
use crate::{EventBus, RuntimeError, RuntimeEvent};

pub const MAILBOX_CAPACITY: usize = 1_024;
pub const CHANNEL_LANE_CAPACITY: usize = 256;
pub const CHANNEL_LANE_IDLE: Duration = Duration::from_secs(300);
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, PartialEq, Eq, Error)]
pub enum DispatchError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("agent {0} is not running")]
    NotRunning(Uuid),
    #[error("agent {0} has no deployed release")]
    NoRelease(Uuid),
    #[error("release rejected: {0}")]
    InvalidRelease(String),
    #[error("agent {0} mailbox is closed")]
    MailboxClosed(Uuid),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunStats {
    pub runs_started: u64,
    pub runs_completed: u64,
    pub runs_failed: u64,
    pub active_runs: u64,
    pub outbound_delivered: u64,
    pub outbound_failed: u64,
    pub last_error: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
}

pub(crate) struct ActorHandle {
    mailbox: mpsc::Sender<QueuedEvent>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

struct QueuedEvent {
    event: InboundEvent,
    release: Arc<AgentRelease>,
}

pub(crate) struct ActorContext {
    pub(crate) agent_id: Uuid,
    pub(crate) bus: Arc<dyn EventBus>,
    pub(crate) runner: Arc<dyn ReleaseRunner>,
    pub(crate) connectors: Option<ConnectorRegistry>,
    pub(crate) stats: Arc<Mutex<HashMap<Uuid, AgentRunStats>>>,
    pub(crate) run_timeout: Duration,
}

struct BusEmitter {
    agent_id: Uuid,
    bus: Arc<dyn EventBus>,
}

impl EventEmitter for BusEmitter {
    fn emit(&self, event: TraceEvent) {
        self.bus.publish(RuntimeEvent::Trace { agent_id: Some(self.agent_id), event });
    }
}

impl ActorContext {
    fn update(&self, change: impl FnOnce(&mut AgentRunStats)) {
        let mut stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        change(stats.entry(self.agent_id).or_default());
    }

    async fn run_once(&self, queued: QueuedEvent) {
        self.update(|stats| {
            stats.runs_started += 1;
            stats.active_runs += 1;
            stats.last_run_at = Some(Utc::now());
        });
        let emitter: Arc<dyn EventEmitter> = Arc::new(BusEmitter { agent_id: self.agent_id, bus: Arc::clone(&self.bus) });
        let guarded = AssertUnwindSafe(self.runner.run(&queued.release, &queued.event, emitter)).catch_unwind();
        let outcome = match tokio::time::timeout(self.run_timeout, guarded).await {
            Ok(Ok(Ok(actions))) => Ok(actions),
            Ok(Ok(Err(error))) => Err(error.to_string()),
            Ok(Err(_panic)) => Err("release runner panicked".to_owned()),
            Err(_elapsed) => Err(format!("run exceeded {} ms", self.run_timeout.as_millis())),
        };
        match outcome {
            Ok(actions) => {
                self.update(|stats| {
                    stats.active_runs = stats.active_runs.saturating_sub(1);
                    stats.runs_completed += 1;
                });
                for action in actions {
                    self.bus.publish(RuntimeEvent::Outbound { action: action.clone() });
                    if let Some(connectors) = &self.connectors {
                        let delivered = connectors.dispatch(action).await;
                        self.update(|stats| match delivered {
                            Ok(()) => stats.outbound_delivered += 1,
                            Err(error) => {
                                stats.outbound_failed += 1;
                                stats.last_error = Some(error.to_string());
                            }
                        });
                    }
                }
            }
            Err(error) => self.update(|stats| {
                stats.active_runs = stats.active_runs.saturating_sub(1);
                stats.runs_failed += 1;
                stats.last_error = Some(error);
            }),
        }
    }
}

async fn channel_lane(context: Arc<ActorContext>, mut lane: mpsc::Receiver<QueuedEvent>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            next = tokio::time::timeout(CHANNEL_LANE_IDLE, lane.recv()) => match next {
                Ok(Some(queued)) => tokio::select! {
                    () = cancel.cancelled() => break,
                    () = context.run_once(queued) => {}
                },
                Ok(None) | Err(_) => break,
            },
        }
    }
}

async fn agent_actor(context: Arc<ActorContext>, mut mailbox: mpsc::Receiver<QueuedEvent>, cancel: CancellationToken) {
    let mut lanes: HashMap<String, mpsc::Sender<QueuedEvent>> = HashMap::new();
    let mut lane_tasks = JoinSet::new();
    loop {
        let queued = tokio::select! {
            () = cancel.cancelled() => break,
            next = mailbox.recv() => match next {
                Some(queued) => queued,
                None => break,
            },
        };
        while lane_tasks.try_join_next().is_some() {}
        lanes.retain(|_, lane| !lane.is_closed());
        let mut pending = queued;
        loop {
            let channel = pending.event.channel.clone();
            let lane = lanes.entry(channel).or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(CHANNEL_LANE_CAPACITY);
                lane_tasks.spawn(channel_lane(Arc::clone(&context), receiver, cancel.child_token()));
                sender
            });
            match lane.send(pending).await {
                Ok(()) => break,
                Err(mpsc::error::SendError(returned)) => {
                    lanes.remove(&returned.event.channel);
                    pending = returned;
                }
            }
        }
    }
    cancel.cancel();
    drop(lanes);
    while lane_tasks.join_next().await.is_some() {}
}

pub(crate) fn spawn_actor(context: Arc<ActorContext>) -> ActorHandle {
    let (mailbox, receiver) = mpsc::channel(MAILBOX_CAPACITY);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(agent_actor(context, receiver, cancel.clone()));
    ActorHandle { mailbox, cancel, task }
}

impl ActorHandle {
    pub(crate) async fn shutdown(self) {
        self.cancel.cancel();
        drop(self.mailbox);
        let _finished = self.task.await;
    }

    pub(crate) async fn enqueue(&self, agent_id: Uuid, event: InboundEvent, release: Arc<AgentRelease>) -> Result<(), DispatchError> {
        self.mailbox.send(QueuedEvent { event, release }).await.map_err(|_| DispatchError::MailboxClosed(agent_id))
    }
}

pub(crate) fn new_release(agent_id: Uuid, version: u32, graph: SpellGraph) -> AgentRelease {
    AgentRelease { agent_id, version, graph, deployed_at: Utc::now() }
}
