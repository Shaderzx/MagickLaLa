use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::FutureExt;
use magicklala_connectors::{ConnectorRegistry, InboundEvent, OutboundAction};
use magicklala_graph::{EventEmitter, RunId, RunStatus, SpellGraph, TraceEvent};
use magicklala_store::{AgentId, ExecutionRun, Repository};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::runner::{AgentRelease, ReleaseOrigin, ReleaseRunner, RunScope, correlation_id};
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
    #[error("agent {0} is busy")]
    Busy(Uuid),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunStats {
    pub runs_started: u64,
    pub runs_completed: u64,
    pub runs_failed: u64,
    pub runs_rejected: u64,
    pub active_runs: u64,
    pub outbound_delivered: u64,
    pub outbound_failed: u64,
    pub runs_unrecorded: u64,
    pub last_error: Option<String>,
    pub last_record_error: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
}

type SharedStats = Arc<Mutex<HashMap<Uuid, AgentRunStats>>>;

fn with_stats(stats: &SharedStats, agent_id: Uuid, change: impl FnOnce(&mut AgentRunStats)) {
    let mut stats = stats.lock().unwrap_or_else(PoisonError::into_inner);
    change(stats.entry(agent_id).or_default());
}

pub(crate) struct ActorHandle {
    mailbox: mpsc::Sender<QueuedEvent>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

pub(crate) struct QueuedEvent {
    event: InboundEvent,
    release: Arc<AgentRelease>,
}

impl QueuedEvent {
    pub(crate) fn new(event: InboundEvent, release: Arc<AgentRelease>) -> Self {
        Self { event, release }
    }
}

pub(crate) struct ActorContext {
    pub(crate) agent_id: Uuid,
    pub(crate) bus: Arc<dyn EventBus>,
    pub(crate) runner: Arc<dyn ReleaseRunner>,
    pub(crate) connectors: Option<ConnectorRegistry>,
    pub(crate) stats: SharedStats,
    pub(crate) run_timeout: Duration,
    pub(crate) repository: Option<Arc<dyn Repository>>,
}

struct BusEmitter {
    agent_id: Uuid,
    bus: Arc<dyn EventBus>,
    nodes_executed: Mutex<u64>,
}

impl BusEmitter {
    fn nodes_executed(&self) -> u64 {
        *self.nodes_executed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EventEmitter for BusEmitter {
    fn emit(&self, event: TraceEvent) {
        if matches!(event, TraceEvent::NodeFinished { .. }) {
            *self.nodes_executed.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        }
        self.bus.publish(RuntimeEvent::Trace { agent_id: Some(self.agent_id), event });
    }
}

struct ActiveRunGuard {
    stats: SharedStats,
    agent_id: Uuid,
}

impl ActiveRunGuard {
    fn enter(stats: &SharedStats, agent_id: Uuid) -> Self {
        with_stats(stats, agent_id, |stats| {
            stats.runs_started += 1;
            stats.active_runs += 1;
            stats.last_run_at = Some(Utc::now());
        });
        Self { stats: Arc::clone(stats), agent_id }
    }
}

impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        with_stats(&self.stats, self.agent_id, |stats| stats.active_runs = stats.active_runs.saturating_sub(1));
    }
}

struct RunOutcome {
    status: RunStatus,
    actions: Vec<OutboundAction>,
    error: Option<String>,
}

impl RunOutcome {
    fn failed(status: RunStatus, error: String) -> Self {
        Self { status, actions: Vec::new(), error: Some(error) }
    }
}

struct Completion<'run> {
    run_id: RunId,
    queued: &'run QueuedEvent,
    started_at: DateTime<Utc>,
    nodes_executed: u64,
    outcome: &'run RunOutcome,
}

impl ActorContext {
    fn update(&self, change: impl FnOnce(&mut AgentRunStats)) {
        with_stats(&self.stats, self.agent_id, change);
    }

    async fn execute(&self, queued: &QueuedEvent, emitter: Arc<dyn EventEmitter>, run_id: RunId, cancel: &CancellationToken) -> RunOutcome {
        let child = cancel.child_token();
        let scope = RunScope { run_id, cancellation: child.clone() };
        let guarded = AssertUnwindSafe(self.runner.run(&queued.release, &queued.event, emitter, scope)).catch_unwind();
        let outcome = tokio::select! {
            () = child.cancelled() => Err(RunOutcome::failed(RunStatus::Cancelled, "run cancelled because the agent stopped".to_owned())),
            finished = tokio::time::timeout(self.run_timeout, guarded) => Ok(finished),
        };
        match outcome {
            Err(cancelled) => cancelled,
            Ok(Ok(Ok(Ok(actions)))) => RunOutcome { status: RunStatus::Succeeded, actions, error: None },
            Ok(Ok(Ok(Err(error)))) => RunOutcome::failed(RunStatus::Failed, error.to_string()),
            Ok(Ok(Err(_panic))) => RunOutcome::failed(RunStatus::Failed, "release runner panicked".to_owned()),
            Ok(Err(_elapsed)) => {
                child.cancel();
                RunOutcome::failed(RunStatus::TimedOut, format!("run exceeded {} ms", self.run_timeout.as_millis()))
            }
        }
    }

    async fn deliver(&self, actions: &[OutboundAction]) {
        for action in actions {
            self.bus.publish(RuntimeEvent::Outbound { action: action.clone() });
            if let Some(connectors) = &self.connectors {
                let delivered = connectors.dispatch(action.clone()).await;
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

    fn execution_run(&self, completion: &Completion<'_>) -> ExecutionRun {
        let event = &completion.queued.event;
        let release = &completion.queued.release;
        ExecutionRun {
            run_id: completion.run_id.0,
            agent_id: Some(AgentId(self.agent_id)),
            spell_id: release.spell_record_id(),
            release_id: release.release_record_id(),
            status: completion.outcome.status,
            trigger: serde_json::to_value(event.kind).ok().and_then(|kind| kind.as_str().map(str::to_owned)).unwrap_or_default(),
            started_at: completion.started_at,
            finished_at: Some(Utc::now()),
            nodes_executed: completion.nodes_executed,
            error: completion.outcome.error.clone(),
            trace: serde_json::json!({
                "correlation_id": correlation_id(&event.payload),
                "channel": event.channel,
                "connector_id": event.connector_id,
                "release_version": release.version,
            }),
        }
    }

    async fn record(&self, run: ExecutionRun) {
        let Some(repository) = &self.repository else { return };
        if let Err(error) = repository.record_run(run).await {
            self.update(|stats| {
                stats.runs_unrecorded += 1;
                stats.last_record_error = Some(error.to_string());
            });
        }
    }

    fn publish_completion(&self, completion: &Completion<'_>) {
        let outcome = completion.outcome;
        self.bus.publish(RuntimeEvent::RunCompleted {
            agent_id: self.agent_id,
            run_id: completion.run_id,
            correlation_id: correlation_id(&completion.queued.event.payload).map(str::to_owned),
            status: outcome.status,
            reply: outcome.actions.first().map(|action| action.content.clone()),
            error: outcome.error.clone(),
        });
    }

    async fn finish(&self, completion: Completion<'_>) {
        self.record(self.execution_run(&completion)).await;
        self.publish_completion(&completion);
    }

    async fn run_once(&self, queued: QueuedEvent, cancel: &CancellationToken) {
        let run_id = RunId::new();
        let started_at = Utc::now();
        let _active = ActiveRunGuard::enter(&self.stats, self.agent_id);
        let emitter = Arc::new(BusEmitter { agent_id: self.agent_id, bus: Arc::clone(&self.bus), nodes_executed: Mutex::new(0) });
        let outcome = self.execute(&queued, Arc::clone(&emitter) as Arc<dyn EventEmitter>, run_id, cancel).await;
        match &outcome.error {
            None => self.update(|stats| stats.runs_completed += 1),
            Some(error) => self.update(|stats| {
                stats.runs_failed += 1;
                stats.last_error = Some(error.clone());
            }),
        }
        self.deliver(&outcome.actions).await;
        self.finish(Completion { run_id, queued: &queued, started_at, nodes_executed: emitter.nodes_executed(), outcome: &outcome }).await;
    }

    fn reject_busy(self: &Arc<Self>, queued: QueuedEvent) {
        let error = format!("channel {} of agent {} is busy", queued.event.channel, self.agent_id);
        self.reject(queued, RunStatus::Failed, error);
    }

    fn reject(self: &Arc<Self>, queued: QueuedEvent, status: RunStatus, error: String) {
        self.update(|stats| {
            stats.runs_rejected += 1;
            stats.last_error = Some(error.clone());
        });
        let outcome = RunOutcome::failed(status, error);
        let completion = Completion { run_id: RunId::new(), queued: &queued, started_at: Utc::now(), nodes_executed: 0, outcome: &outcome };
        let run = self.execution_run(&completion);
        self.publish_completion(&completion);
        if self.repository.is_some() {
            let context = Arc::clone(self);
            tokio::spawn(async move { context.record(run).await });
        }
    }
}

async fn channel_lane(context: Arc<ActorContext>, mut lane: mpsc::Receiver<QueuedEvent>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            next = tokio::time::timeout(CHANNEL_LANE_IDLE, lane.recv()) => match next {
                Ok(Some(queued)) => context.run_once(queued, &cancel).await,
                Ok(None) | Err(_) => break,
            },
        }
    }
    lane.close();
    while let Ok(abandoned) = lane.try_recv() {
        context.reject(abandoned, RunStatus::Cancelled, "run cancelled because the agent stopped before it started".to_owned());
    }
}

fn route(
    context: &Arc<ActorContext>,
    lanes: &mut HashMap<String, mpsc::Sender<QueuedEvent>>,
    lane_tasks: &mut JoinSet<()>,
    cancel: &CancellationToken,
    queued: QueuedEvent,
) {
    let mut pending = queued;
    loop {
        let channel = pending.event.channel.clone();
        let lane = lanes.entry(channel).or_insert_with(|| {
            let (sender, receiver) = mpsc::channel(CHANNEL_LANE_CAPACITY);
            lane_tasks.spawn(channel_lane(Arc::clone(context), receiver, cancel.child_token()));
            sender
        });
        match lane.try_send(pending) {
            Ok(()) => return,
            Err(mpsc::error::TrySendError::Full(rejected)) => return context.reject_busy(rejected),
            Err(mpsc::error::TrySendError::Closed(returned)) => {
                lanes.remove(&returned.event.channel);
                pending = returned;
            }
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
        route(&context, &mut lanes, &mut lane_tasks, &cancel, queued);
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

    pub(crate) fn mailbox(&self) -> mpsc::Sender<QueuedEvent> {
        self.mailbox.clone()
    }
}

pub(crate) fn new_release(agent_id: Uuid, version: u32, graph: SpellGraph, origin: Option<ReleaseOrigin>) -> AgentRelease {
    AgentRelease { agent_id, version, graph, deployed_at: Utc::now(), origin }
}
