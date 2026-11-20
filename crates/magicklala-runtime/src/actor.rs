use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::FutureExt;
use magicklala_connectors::{ConnectorRegistry, InboundEvent, OutboundAction};
use magicklala_graph::{EventEmitter, RunId, RunStatus, RunSummary, SpellGraph, TraceEvent};
use magicklala_store::{AgentId, ExecutionRun, Repository};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::runner::{AgentRelease, ReleaseOrigin, ReleaseRunner, RunScope, correlation_id};
use crate::trace::BoundedTrace;
use crate::{EventBus, RuntimeError, RuntimeEvent};

pub const MAILBOX_CAPACITY: usize = 1_024;
pub const CHANNEL_LANE_CAPACITY: usize = 256;
pub const CHANNEL_LANE_IDLE: Duration = Duration::from_secs(300);
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_LANES_PER_AGENT: usize = 64;
pub const REJECTED_RUN_QUEUE_CAPACITY: usize = 256;

const AGENT_STOPPED: &str = "run cancelled because the agent stopped";
const STOPPED_BEFORE_START: &str = "run cancelled because the agent stopped before it started";
const DEADLINE_EXPIRED: &str = "run deadline expired while queued; it was never started";
const REJECTED_QUEUE_FULL: &str = "the rejected-run persistence queue is full; this busy rejection was not recorded";

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
    pub runs_expired: u64,
    pub runs_cancelled_before_start: u64,
    pub active_runs: u64,
    pub live_lanes: u64,
    pub outbound_delivered: u64,
    pub outbound_failed: u64,
    pub runs_unrecorded: u64,
    pub last_error: Option<String>,
    pub last_record_error: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCompletion {
    pub agent_id: Uuid,
    pub run_id: RunId,
    pub correlation_id: Option<String>,
    pub status: RunStatus,
    pub reply: Option<String>,
    pub replies: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DispatchOptions {
    pub deadline: Option<Instant>,
}

impl DispatchOptions {
    pub fn within(budget: Duration) -> Self {
        Self { deadline: Instant::now().checked_add(budget) }
    }
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
    persistence: Option<JoinHandle<()>>,
}

pub(crate) struct QueuedEvent {
    event: InboundEvent,
    release: Arc<AgentRelease>,
    deadline: Option<Instant>,
    reply: Option<oneshot::Sender<RunCompletion>>,
    admitted: Option<oneshot::Sender<Result<(), DispatchError>>>,
}

impl QueuedEvent {
    pub(crate) fn new(event: InboundEvent, release: Arc<AgentRelease>, options: DispatchOptions, reply: Option<oneshot::Sender<RunCompletion>>) -> Self {
        Self { event, release, deadline: options.deadline, reply, admitted: None }
    }

    pub(crate) fn with_admission(mut self, admitted: Option<oneshot::Sender<Result<(), DispatchError>>>) -> Self {
        self.admitted = admitted;
        self
    }

    fn admit(&mut self, verdict: Result<(), DispatchError>) {
        if let Some(admitted) = self.admitted.take() {
            let _notified = admitted.send(verdict).is_ok();
        }
    }

    fn closing_summary(&self, run_id: RunId, started_at: DateTime<Utc>, nodes_executed: u64, outcome: &RunOutcome) -> RunSummary {
        RunSummary {
            run_id,
            spell_id: self.release.graph.id,
            started_at,
            finished_at: Utc::now(),
            status: outcome.status,
            nodes_executed,
            outputs: std::collections::BTreeMap::new(),
            activations: Vec::new(),
            activations_truncated: false,
            replies: Vec::new(),
            error: outcome.error.clone(),
        }
    }

    fn expired(&self) -> bool {
        self.deadline.is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn correlation(&self) -> Option<String> {
        correlation_id(&self.event.payload).map(str::to_owned)
    }
}

pub(crate) struct ActorContext {
    pub(crate) agent_id: Uuid,
    pub(crate) bus: Arc<dyn EventBus>,
    pub(crate) runner: Arc<dyn ReleaseRunner>,
    pub(crate) connectors: Option<ConnectorRegistry>,
    pub(crate) stats: SharedStats,
    pub(crate) run_timeout: Duration,
    pub(crate) lane_idle: Duration,
    pub(crate) max_lanes: usize,
    pub(crate) repository: Option<Arc<dyn Repository>>,
}

struct BusEmitter {
    agent_id: Uuid,
    bus: Arc<dyn EventBus>,
    nodes_executed: Mutex<u64>,
    trace: Arc<BoundedTrace>,
}

impl BusEmitter {
    fn new(agent_id: Uuid, bus: Arc<dyn EventBus>) -> Self {
        Self { agent_id, bus, nodes_executed: Mutex::new(0), trace: Arc::new(BoundedTrace::new()) }
    }

    fn nodes_executed(&self) -> u64 {
        *self.nodes_executed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EventEmitter for BusEmitter {
    fn emit(&self, event: TraceEvent) {
        if matches!(event, TraceEvent::NodeFinished { .. }) {
            *self.nodes_executed.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        }
        self.trace.push(&event);
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

struct Finished {
    run_id: RunId,
    started_at: DateTime<Utc>,
    nodes_executed: u64,
    outcome: RunOutcome,
    trace: Arc<BoundedTrace>,
}

impl Finished {
    fn unstarted(queued: &QueuedEvent, outcome: RunOutcome) -> Self {
        let (run_id, started_at) = (RunId::new(), Utc::now());
        let trace = Arc::new(BoundedTrace::new());
        trace.close(&queued.closing_summary(run_id, started_at, 0, &outcome));
        Self { run_id, started_at, nodes_executed: 0, outcome, trace }
    }

    fn first_reply(&self) -> Option<&str> {
        self.outcome.actions.first().map(|action| action.content.as_str())
    }
}

impl ActorContext {
    fn update(&self, change: impl FnOnce(&mut AgentRunStats)) {
        with_stats(&self.stats, self.agent_id, change);
    }

    fn budget(&self, queued: &QueuedEvent) -> Duration {
        queued.deadline.map_or(self.run_timeout, |deadline| deadline.saturating_duration_since(Instant::now()).min(self.run_timeout))
    }

    async fn execute(&self, queued: &QueuedEvent, emitter: Arc<dyn EventEmitter>, run_id: RunId, cancel: &CancellationToken) -> RunOutcome {
        let child = cancel.child_token();
        let budget = self.budget(queued);
        let scope = RunScope { run_id, cancellation: child.clone() };
        let guarded = AssertUnwindSafe(self.runner.run(&queued.release, &queued.event, emitter, scope)).catch_unwind();
        let outcome = tokio::select! {
            biased;
            () = child.cancelled() => Err(RunOutcome::failed(RunStatus::Cancelled, AGENT_STOPPED.to_owned())),
            finished = tokio::time::timeout(budget, guarded) => Ok(finished),
        };
        if cancel.is_cancelled() && !matches!(outcome, Ok(Ok(Ok(Ok(_))))) {
            return RunOutcome::failed(RunStatus::Cancelled, AGENT_STOPPED.to_owned());
        }
        match outcome {
            Err(cancelled) => cancelled,
            Ok(Ok(Ok(Ok(actions)))) => RunOutcome { status: RunStatus::Succeeded, actions, error: None },
            Ok(Ok(Ok(Err(error)))) => RunOutcome::failed(RunStatus::Failed, error.to_string()),
            Ok(Ok(Err(_panic))) => RunOutcome::failed(RunStatus::Failed, "release runner panicked".to_owned()),
            Ok(Err(_elapsed)) => {
                child.cancel();
                RunOutcome::failed(RunStatus::TimedOut, format!("run exceeded {} ms", budget.as_millis()))
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

    fn execution_run(&self, queued: &QueuedEvent, finished: &Finished) -> ExecutionRun {
        let event = &queued.event;
        let release = &queued.release;
        ExecutionRun {
            run_id: finished.run_id.0,
            agent_id: Some(AgentId(self.agent_id)),
            spell_id: release.spell_record_id(),
            release_id: release.release_record_id(),
            status: finished.outcome.status,
            trigger: serde_json::to_value(event.kind).ok().and_then(|kind| kind.as_str().map(str::to_owned)).unwrap_or_default(),
            started_at: finished.started_at,
            finished_at: Some(Utc::now()),
            nodes_executed: finished.nodes_executed,
            error: finished.outcome.error.clone(),
            reply: finished.first_reply().map(str::to_owned),
            trace: finished.trace.seal(serde_json::Map::from_iter([
                ("correlation_id".to_owned(), serde_json::json!(correlation_id(&event.payload))),
                ("channel".to_owned(), serde_json::json!(event.channel)),
                ("connector_id".to_owned(), serde_json::json!(event.connector_id)),
                ("release_version".to_owned(), serde_json::json!(release.version)),
            ])),
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

    fn completion(&self, queued: &QueuedEvent, finished: &Finished) -> RunCompletion {
        let replies: Vec<String> = finished.outcome.actions.iter().map(|action| action.content.clone()).collect();
        RunCompletion {
            agent_id: self.agent_id,
            run_id: finished.run_id,
            correlation_id: queued.correlation(),
            status: finished.outcome.status,
            reply: replies.first().cloned(),
            replies,
            error: finished.outcome.error.clone(),
        }
    }

    fn announce(&self, mut queued: QueuedEvent, finished: &Finished) {
        let completion = self.completion(&queued, finished);
        self.bus.publish(RuntimeEvent::RunCompleted {
            agent_id: self.agent_id,
            run_id: completion.run_id,
            correlation_id: completion.correlation_id.clone(),
            status: completion.status,
            reply: completion.reply.clone(),
            error: completion.error.clone(),
        });
        if let Some(reply) = queued.reply.take() {
            let _delivered = reply.send(completion).is_ok();
        }
    }

    async fn finish(&self, queued: QueuedEvent, finished: Finished) {
        self.record(self.execution_run(&queued, &finished)).await;
        self.announce(queued, &finished);
    }

    async fn run_once(&self, queued: QueuedEvent, cancel: &CancellationToken) {
        let run_id = RunId::new();
        let started_at = Utc::now();
        let _active = ActiveRunGuard::enter(&self.stats, self.agent_id);
        let emitter = Arc::new(BusEmitter::new(self.agent_id, Arc::clone(&self.bus)));
        let outcome = self.execute(&queued, Arc::clone(&emitter) as Arc<dyn EventEmitter>, run_id, cancel).await;
        match &outcome.error {
            None => self.update(|stats| stats.runs_completed += 1),
            Some(error) => self.update(|stats| {
                stats.runs_failed += 1;
                stats.last_error = Some(error.clone());
            }),
        }
        self.deliver(&outcome.actions).await;
        let nodes_executed = emitter.nodes_executed();
        emitter.trace.close(&queued.closing_summary(run_id, started_at, nodes_executed, &outcome));
        let trace = Arc::clone(&emitter.trace);
        self.finish(queued, Finished { run_id, started_at, nodes_executed, outcome, trace }).await;
    }

    async fn settle_unstarted(&self, queued: QueuedEvent, status: RunStatus, error: &str) {
        self.update(|stats| {
            match status {
                RunStatus::TimedOut => stats.runs_expired += 1,
                _ => stats.runs_cancelled_before_start += 1,
            }
            stats.last_error = Some(error.to_owned());
        });
        let finished = Finished::unstarted(&queued, RunOutcome::failed(status, error.to_owned()));
        self.finish(queued, finished).await;
    }

    async fn process(&self, queued: QueuedEvent, cancel: &CancellationToken) {
        if cancel.is_cancelled() {
            self.settle_unstarted(queued, RunStatus::Cancelled, STOPPED_BEFORE_START).await;
        } else if queued.expired() {
            self.settle_unstarted(queued, RunStatus::TimedOut, DEADLINE_EXPIRED).await;
        } else {
            self.run_once(queued, cancel).await;
        }
    }

    fn reject(&self, queued: QueuedEvent, error: String, rejected_runs: Option<&mpsc::Sender<ExecutionRun>>) {
        self.update(|stats| {
            stats.runs_rejected += 1;
            stats.last_error = Some(error.clone());
        });
        let finished = Finished::unstarted(&queued, RunOutcome::failed(RunStatus::Failed, error));
        if let Some(queue) = rejected_runs
            && queue.try_send(self.execution_run(&queued, &finished)).is_err()
        {
            self.update(|stats| {
                stats.runs_unrecorded += 1;
                stats.last_record_error = Some(REJECTED_QUEUE_FULL.to_owned());
            });
        }
        self.announce(queued, &finished);
    }
}

async fn persist_rejected_runs(context: Arc<ActorContext>, mut runs: mpsc::Receiver<ExecutionRun>) {
    while let Some(run) = runs.recv().await {
        context.record(run).await;
    }
}

async fn channel_lane(
    context: Arc<ActorContext>,
    mut lane: mpsc::Receiver<QueuedEvent>,
    cancel: CancellationToken,
    predecessor: Option<oneshot::Receiver<()>>,
    finished: oneshot::Sender<()>,
) {
    if let Some(previous) = predecessor {
        let _drained = previous.await;
    }
    loop {
        let next = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            next = tokio::time::timeout(context.lane_idle, lane.recv()) => next,
        };
        match next {
            Ok(Some(queued)) => context.process(queued, &cancel).await,
            Ok(None) => break,
            Err(_idle) => break,
        }
    }
    lane.close();
    while let Some(queued) = lane.recv().await {
        context.process(queued, &cancel).await;
    }
    drop(finished);
}

struct Lane {
    sender: mpsc::Sender<QueuedEvent>,
    drained: oneshot::Receiver<()>,
}

struct LaneRouter {
    context: Arc<ActorContext>,
    cancel: CancellationToken,
    lanes: HashMap<String, Lane>,
    draining: HashMap<String, oneshot::Receiver<()>>,
    tasks: JoinSet<()>,
    rejected_runs: Option<mpsc::Sender<ExecutionRun>>,
}

impl LaneRouter {
    fn new(context: Arc<ActorContext>, cancel: CancellationToken, rejected_runs: Option<mpsc::Sender<ExecutionRun>>) -> Self {
        Self { context, cancel, lanes: HashMap::new(), draining: HashMap::new(), tasks: JoinSet::new(), rejected_runs }
    }

    fn reject(&self, queued: QueuedEvent, error: String) {
        self.context.reject(queued, error, self.rejected_runs.as_ref());
    }

    fn retire(&mut self, channel: &str) {
        if let Some(lane) = self.lanes.remove(channel) {
            self.draining.insert(channel.to_owned(), lane.drained);
        }
    }

    fn sweep(&mut self) {
        while self.tasks.try_join_next().is_some() {}
        let closed: Vec<String> = self.lanes.iter().filter(|(_, lane)| lane.sender.is_closed()).map(|(channel, _)| channel.clone()).collect();
        for channel in closed {
            self.retire(&channel);
        }
        self.draining.retain(|_, drained| matches!(drained.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    }

    fn publish_live_lanes(&self) {
        let live = u64::try_from(self.lanes.len()).unwrap_or(u64::MAX);
        self.context.update(|stats| stats.live_lanes = live);
    }

    fn open(&mut self, channel: &str) {
        let (sender, receiver) = mpsc::channel(CHANNEL_LANE_CAPACITY);
        let (finished, drained) = oneshot::channel();
        let predecessor = self.draining.remove(channel);
        self.tasks.spawn(channel_lane(Arc::clone(&self.context), receiver, self.cancel.child_token(), predecessor, finished));
        self.lanes.insert(channel.to_owned(), Lane { sender, drained });
    }

    fn refuse(&self, mut queued: QueuedEvent, error: String) {
        queued.admit(Err(DispatchError::Busy(self.context.agent_id)));
        self.reject(queued, error);
    }

    fn route(&mut self, mut queued: QueuedEvent) {
        loop {
            let channel = queued.event.channel.clone();
            if !self.lanes.contains_key(&channel) {
                if self.lanes.len() >= self.context.max_lanes {
                    let error = format!("agent {} is busy: {} channel lanes are already live", self.context.agent_id, self.context.max_lanes);
                    return self.refuse(queued, error);
                }
                self.open(&channel);
            }
            let Some(lane) = self.lanes.get(&channel) else {
                return queued.admit(Err(DispatchError::NotRunning(self.context.agent_id)));
            };
            match lane.sender.try_reserve() {
                Ok(permit) => {
                    queued.admit(Ok(()));
                    return permit.send(queued);
                }
                Err(mpsc::error::TrySendError::Full(())) => {
                    let error = format!("channel {channel} of agent {} is busy", self.context.agent_id);
                    return self.refuse(queued, error);
                }
                Err(mpsc::error::TrySendError::Closed(())) => {}
            }
            self.retire(&channel);
        }
    }

    async fn shutdown(mut self) {
        self.cancel.cancel();
        self.lanes.clear();
        while self.tasks.join_next().await.is_some() {}
        self.publish_live_lanes();
    }
}

async fn agent_actor(
    context: Arc<ActorContext>,
    mut mailbox: mpsc::Receiver<QueuedEvent>,
    cancel: CancellationToken,
    rejected_runs: Option<mpsc::Sender<ExecutionRun>>,
) {
    let mut router = LaneRouter::new(Arc::clone(&context), cancel.clone(), rejected_runs);
    loop {
        let queued = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            next = mailbox.recv() => match next {
                Some(queued) => queued,
                None => break,
            },
        };
        router.sweep();
        router.route(queued);
        router.publish_live_lanes();
    }
    cancel.cancel();
    mailbox.close();
    while let Some(mut queued) = mailbox.recv().await {
        queued.admit(Err(DispatchError::NotRunning(context.agent_id)));
        context.settle_unstarted(queued, RunStatus::Cancelled, STOPPED_BEFORE_START).await;
    }
    router.shutdown().await;
}

pub(crate) fn spawn_actor(context: Arc<ActorContext>) -> ActorHandle {
    let (mailbox, receiver) = mpsc::channel(MAILBOX_CAPACITY);
    let cancel = CancellationToken::new();
    let (rejected_runs, persistence) = match context.repository {
        Some(_) => {
            let (sender, runs) = mpsc::channel(REJECTED_RUN_QUEUE_CAPACITY);
            (Some(sender), Some(tokio::spawn(persist_rejected_runs(Arc::clone(&context), runs))))
        }
        None => (None, None),
    };
    let task = tokio::spawn(agent_actor(context, receiver, cancel.clone(), rejected_runs));
    ActorHandle { mailbox, cancel, task, persistence }
}

impl ActorHandle {
    pub(crate) async fn shutdown(self) {
        self.cancel.cancel();
        drop(self.mailbox);
        let _finished = self.task.await;
        if let Some(persistence) = self.persistence {
            let _drained = persistence.await;
        }
    }

    pub(crate) fn mailbox(&self) -> mpsc::Sender<QueuedEvent> {
        self.mailbox.clone()
    }
}

pub(crate) fn new_release(agent_id: Uuid, version: u32, graph: SpellGraph, origin: Option<ReleaseOrigin>) -> AgentRelease {
    AgentRelease { agent_id, version, graph, deployed_at: Utc::now(), origin }
}
