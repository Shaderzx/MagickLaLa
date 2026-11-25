use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::subgraphs::unresolvable_subgraphs;
use crate::{
    EmittedOutput, EventEmitter, KnowledgeService, LlmGateway, MAX_SUBGRAPH_DEPTH, MemoryService, NodeContext, NodeError, NodeExecutor, NodeId, NodeInstance,
    NodeOutcome, NodeSpec, ResolvedSubgraphs, RunId, RunStatus, RunSummary, SpellError, SpellGraph, SpellId, SpellValidator, SubgraphKey, TraceEvent, Value,
    Wire, WireId, resolve_subgraphs_from,
};

pub const WAIT_FOR_ALL_KIND: &str = "flow/wait_for_all";
pub const WHILE_LOOP_KIND: &str = "flow/while_loop";
pub const DO_ONCE_KIND: &str = "flow/do_once";
pub const SUBGRAPH_KIND: &str = "flow/subgraph";
pub const START_TRIGGER_KIND: &str = "event/on_start";
pub const TRIGGER_PAYLOAD_SOCKET: &str = "payload";

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type NodeOutputs = BTreeMap<String, Value>;
type SharedOutputs = Arc<NodeOutputs>;
type Memo = Arc<Mutex<HashMap<NodeId, SharedOutputs>>>;
type Scope = Arc<BTreeMap<NodeId, SharedOutputs>>;
type JoinQueues = HashMap<Option<WireId>, VecDeque<Scope>>;
type WaitCell = Arc<Mutex<Option<NodeId>>>;

pub const DEFAULT_WHILE_LOOP_MAX_ITERATIONS: u64 = 1000;
pub const MAX_ACTIVATION_LOG_BYTES: usize = 1024 * 1024;
pub const MAX_REPLIES: usize = 1000;
pub const MAX_REPLY_BYTES: usize = 1024 * 1024;

pub trait NodeCatalog: Send + Sync {
    fn executor(&self, kind: &str) -> Option<Arc<dyn NodeExecutor>>;
    fn specs(&self) -> Vec<NodeSpec>;
}

pub trait SubgraphResolver: Send + Sync {
    fn resolve(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<SpellGraph>;

    fn resolve_entry(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<(SpellId, SpellGraph)> {
        self.resolve(spell_id, release_version).map(|graph| (graph.id, graph))
    }
}

#[derive(Clone, Default)]
pub struct EngineServices {
    pub llm: Option<Arc<dyn LlmGateway>>,
    pub memory: Option<Arc<dyn MemoryService>>,
    pub knowledge: Option<Arc<dyn KnowledgeService>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunBinding {
    pub agent_id: Option<String>,
    pub allow_private_http: Option<bool>,
    pub run_id: Option<RunId>,
    pub root_subgraph: Option<SubgraphKey>,
}

impl RunBinding {
    pub fn with_root_subgraph(mut self, key: SubgraphKey) -> Self {
        self.root_subgraph = Some(key);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunTrigger {
    pub node: NodeId,
    pub socket: String,
    pub payload: Value,
}

pub struct SpellEngine {
    catalog: Arc<dyn NodeCatalog>,
    services: EngineServices,
    step_limit: u64,
    subgraphs: Option<Arc<dyn SubgraphResolver>>,
    binding: RunBinding,
}

impl SpellEngine {
    pub const DEFAULT_STEP_LIMIT: u64 = 10_000;

    pub fn new(catalog: Arc<dyn NodeCatalog>, services: EngineServices) -> Self {
        Self { catalog, services, step_limit: Self::DEFAULT_STEP_LIMIT, subgraphs: None, binding: RunBinding::default() }
    }

    pub fn with_step_limit(mut self, step_limit: u64) -> Self {
        self.step_limit = step_limit;
        self
    }

    pub fn with_subgraph_resolver(mut self, resolver: Arc<dyn SubgraphResolver>) -> Self {
        self.subgraphs = Some(resolver);
        self
    }

    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.binding.agent_id = Some(agent_id.into());
        self
    }

    pub fn with_private_http(mut self, allowed: bool) -> Self {
        self.binding.allow_private_http = Some(allowed);
        self
    }

    pub fn with_run_id(mut self, run_id: RunId) -> Self {
        self.binding.run_id = Some(run_id);
        self
    }

    pub fn with_root_subgraph(mut self, key: SubgraphKey) -> Self {
        self.binding.root_subgraph = Some(key);
        self
    }

    pub fn binding(&self) -> &RunBinding {
        &self.binding
    }

    pub fn step_limit(&self) -> u64 {
        self.step_limit
    }

    pub fn services(&self) -> &EngineServices {
        &self.services
    }

    pub fn validator(&self) -> SpellValidator {
        SpellValidator::new(self.catalog.specs())
    }

    pub async fn run(&self, graph: &SpellGraph, trigger: RunTrigger, events: Arc<dyn EventEmitter>) -> Result<RunSummary, SpellError> {
        self.run_with_cancellation(graph, trigger, events, CancellationToken::new()).await
    }

    pub async fn run_with_cancellation(
        &self,
        graph: &SpellGraph,
        trigger: RunTrigger,
        events: Arc<dyn EventEmitter>,
        cancellation: CancellationToken,
    ) -> Result<RunSummary, SpellError> {
        self.run_bound(graph, trigger, events, cancellation, RunBinding::default()).await
    }

    pub async fn run_bound(
        &self,
        graph: &SpellGraph,
        trigger: RunTrigger,
        events: Arc<dyn EventEmitter>,
        cancellation: CancellationToken,
        binding: RunBinding,
    ) -> Result<RunSummary, SpellError> {
        let validator = self.validator();
        validator.validate(graph).map_err(SpellError::Validation)?;
        let binding = RunBinding {
            agent_id: binding.agent_id.or_else(|| self.binding.agent_id.clone()),
            allow_private_http: binding.allow_private_http.or(self.binding.allow_private_http),
            run_id: binding.run_id.or(self.binding.run_id),
            root_subgraph: binding.root_subgraph.or(self.binding.root_subgraph),
        };
        let children = self.check_subgraphs(graph, binding.root_subgraph.unwrap_or((graph.id, None)), &validator)?;
        let shared = Arc::new(RunShared {
            run_id: binding.run_id.unwrap_or_default(),
            catalog: Arc::clone(&self.catalog),
            services: self.services.clone(),
            cancellation,
            step_limit: self.step_limit,
            steps: AtomicU64::new(0),
            executed: AtomicU64::new(0),
            replies: Mutex::new(ReplyLog::default()),
            children,
            binding,
        });
        let state = Arc::new(RunState::prepare(Arc::clone(&shared), Arc::new(graph.clone()), trigger, 0, events)?);
        let started_at = Utc::now();
        state.emit(TraceEvent::RunStarted { run_id: shared.run_id, spell_id: graph.id, at: started_at });
        let first = Activation { node: state.trigger.node.clone(), socket: state.trigger.socket.clone(), via: None, scope: Scope::default() };
        let activated = state.clone().run_lane(first).await;
        let outcome = activated.and_then(|()| state.check_joins_complete());
        let summary = state.summary(started_at, &outcome);
        state.emit(TraceEvent::RunFinished { run_id: shared.run_id, summary: summary.slim() });
        outcome.map(|()| summary)
    }

    fn check_subgraphs(&self, graph: &SpellGraph, root_key: SubgraphKey, validator: &SpellValidator) -> Result<ResolvedSubgraphs, SpellError> {
        let Some(resolver) = &self.subgraphs else {
            return unresolvable_subgraphs(graph).map(|()| ResolvedSubgraphs::new());
        };
        resolve_subgraphs_from(graph, root_key, resolver.as_ref(), validator)
    }
}

fn with_outputs(scope: &Scope, node: &NodeId, outputs: SharedOutputs) -> Scope {
    let mut next = BTreeMap::clone(scope);
    next.insert(node.clone(), outputs);
    Arc::new(next)
}

fn merge_scopes(scopes: impl IntoIterator<Item = Scope>) -> Scope {
    let mut merged = BTreeMap::new();
    for scope in scopes {
        merged.extend(scope.iter().map(|(node, outputs)| (node.clone(), Arc::clone(outputs))));
    }
    Arc::new(merged)
}

fn approx_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => 8,
        Value::String(text) => text.len(),
        Value::Json(json) => approx_json_bytes(json),
        Value::Array(items) => items.iter().map(approx_bytes).sum(),
        Value::Embedding(vector) => vector.len() * 4,
        Value::Event(fields) => fields.iter().map(|(key, field)| key.len() + approx_bytes(field)).sum(),
    }
}

fn approx_json_bytes(json: &serde_json::Value) -> usize {
    match json {
        serde_json::Value::String(text) => text.len(),
        serde_json::Value::Array(items) => items.iter().map(approx_json_bytes).sum(),
        serde_json::Value::Object(fields) => fields.iter().map(|(key, field)| key.len() + approx_json_bytes(field)).sum(),
        _ => 8,
    }
}

fn outputs_bytes(outputs: &NodeOutputs) -> usize {
    outputs.iter().map(|(socket, value)| socket.len() + approx_bytes(value)).sum()
}

enum Readiness {
    Ready(Option<Value>, Option<TraceEvent>),
    Waiting(NodeId),
}

struct SuspendUntilDataReady {
    memo: Memo,
    waiting_on: WaitCell,
    source: NodeId,
}

impl Future for SuspendUntilDataReady {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if locked(&self.memo).contains_key(&self.source) {
            return Poll::Ready(());
        }
        *locked(&self.waiting_on) = Some(self.source.clone());
        Poll::Pending
    }
}

struct DataFrame {
    node: NodeId,
    started_at: Instant,
    waiting_on: WaitCell,
    future: BoxFuture<Result<NodeOutcome, NodeError>>,
}

struct DataStack {
    state: Arc<RunState>,
    frames: Vec<DataFrame>,
    pending: HashSet<NodeId>,
    failure: String,
}

impl DataStack {
    fn new(state: Arc<RunState>) -> Self {
        Self { state, frames: Vec::new(), pending: HashSet::new(), failure: NodeError::Cancelled.to_string() }
    }

    fn fail(&mut self, fault: Fault) -> Fault {
        self.failure = match &fault {
            Fault::Node(error) => error.to_string(),
            Fault::Run(error) => error.to_string(),
        };
        fault
    }
}

impl Drop for DataStack {
    fn drop(&mut self) {
        while let Some(frame) = self.frames.pop() {
            self.state.emit(TraceEvent::NodeFailed {
                run_id: self.state.run_id(),
                node_id: frame.node,
                at: Utc::now(),
                error: self.failure.clone(),
                subgraph_path: Vec::new(),
            });
        }
    }
}

struct QualifiedEvents {
    inner: Arc<dyn EventEmitter>,
    caller: NodeId,
}

impl EventEmitter for QualifiedEvents {
    fn emit(&self, mut event: TraceEvent) {
        let Some(path) = event.subgraph_path_mut() else {
            return;
        };
        path.insert(0, self.caller.clone());
        self.inner.emit(event);
    }
}

struct Activation {
    node: NodeId,
    socket: String,
    via: Option<WireId>,
    scope: Scope,
}

struct Repeat {
    node: NodeId,
    socket: String,
    scope: Scope,
    iterations: u64,
}

enum Work {
    Activate(Activation),
    Repeat(Repeat),
}

enum Fault {
    Node(NodeError),
    Run(SpellError),
}

#[derive(Default)]
struct ActivationLog {
    entries: Vec<(NodeId, SharedOutputs)>,
    bytes: usize,
    truncated: bool,
}

impl ActivationLog {
    fn record(&mut self, node: &NodeId, outputs: &SharedOutputs) {
        let size = outputs_bytes(outputs);
        if self.truncated || self.bytes.saturating_add(size) > MAX_ACTIVATION_LOG_BYTES {
            self.truncated = true;
            self.entries.push((node.clone(), Arc::default()));
            return;
        }
        self.bytes += size;
        self.entries.push((node.clone(), Arc::clone(outputs)));
    }
}

#[derive(Default)]
struct ReplyLog {
    replies: Vec<String>,
    bytes: usize,
}

impl ReplyLog {
    fn record(&mut self, reply: &str) -> Result<(), NodeError> {
        if self.replies.len() >= MAX_REPLIES || self.bytes.saturating_add(reply.len()) > MAX_REPLY_BYTES {
            return Err(NodeError::Other(format!("run produced more than {MAX_REPLIES} replies or {MAX_REPLY_BYTES} bytes of replies")));
        }
        self.bytes += reply.len();
        self.replies.push(reply.to_owned());
        Ok(())
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct RunShared {
    run_id: RunId,
    catalog: Arc<dyn NodeCatalog>,
    services: EngineServices,
    cancellation: CancellationToken,
    step_limit: u64,
    steps: AtomicU64,
    executed: AtomicU64,
    replies: Mutex<ReplyLog>,
    children: ResolvedSubgraphs,
    binding: RunBinding,
}

impl RunShared {
    fn count_step(&self) -> Result<(), SpellError> {
        let taken = self.steps.fetch_add(1, Ordering::SeqCst) + 1;
        if taken > self.step_limit { Err(SpellError::StepLimitExceeded(self.step_limit)) } else { Ok(()) }
    }
}

struct RunState {
    shared: Arc<RunShared>,
    events: Arc<dyn EventEmitter>,
    depth: usize,
    spell_id: SpellId,
    graph: Arc<SpellGraph>,
    executors: HashMap<NodeId, Arc<dyn NodeExecutor>>,
    instances: HashMap<NodeId, NodeInstance>,
    impure: HashSet<NodeId>,
    wires_into: HashMap<NodeId, Vec<Wire>>,
    subgraph_targets: HashMap<NodeId, SubgraphKey>,
    trigger: RunTrigger,
    defaults: BTreeMap<String, Value>,
    variables: Mutex<BTreeMap<String, Value>>,
    outputs: Mutex<BTreeMap<NodeId, SharedOutputs>>,
    activations: Mutex<ActivationLog>,
    last_reply: Mutex<Option<String>>,
    joins: Mutex<HashMap<NodeId, JoinQueues>>,
    once_fired: Mutex<HashSet<NodeId>>,
}

fn child_trigger(shared: &RunShared, graph: &SpellGraph, payload: Value) -> Result<RunTrigger, SpellError> {
    let is_trigger = |node: &&NodeInstance| shared.catalog.executor(&node.kind).is_some_and(|executor| executor.spec().is_trigger);
    let node = graph
        .nodes
        .iter()
        .find(|node| node.kind == START_TRIGGER_KIND)
        .or_else(|| graph.nodes.iter().find(is_trigger))
        .ok_or_else(|| SpellError::Internal(format!("subgraph {:?} has no trigger node", graph.id)))?;
    Ok(RunTrigger { node: node.id.clone(), socket: "out".to_owned(), payload })
}

impl RunState {
    fn prepare(shared: Arc<RunShared>, graph: Arc<SpellGraph>, trigger: RunTrigger, depth: usize, events: Arc<dyn EventEmitter>) -> Result<Self, SpellError> {
        let mut executors = HashMap::new();
        let mut instances = HashMap::new();
        let mut impure = HashSet::new();
        for node in &graph.nodes {
            let executor = shared.catalog.executor(&node.kind).ok_or_else(|| SpellError::UnknownNodeKind(node.kind.clone()))?;
            let spec = executor.spec();
            if spec.is_trigger || spec.inputs.iter().any(|socket| socket.ty.is_flow()) {
                impure.insert(node.id.clone());
            }
            executors.insert(node.id.clone(), executor);
            instances.insert(node.id.clone(), node.clone());
        }
        if !instances.contains_key(&trigger.node) {
            return Err(SpellError::Internal(format!("trigger node {:?} is not part of the spell", trigger.node)));
        }
        let mut wires_into: HashMap<NodeId, Vec<Wire>> = HashMap::new();
        for wire in &graph.wires {
            wires_into.entry(wire.to_node.clone()).or_default().push(wire.clone());
        }
        let subgraph_targets = graph.subgraphs.iter().map(|reference| (reference.node_id.clone(), (reference.spell_id, reference.release_version))).collect();
        let defaults: BTreeMap<String, Value> =
            graph.variables.iter().map(|variable| (variable.name.clone(), variable.default.clone().unwrap_or_default())).collect();
        Ok(Self {
            shared,
            events,
            depth,
            spell_id: graph.id,
            graph,
            executors,
            instances,
            impure,
            wires_into,
            subgraph_targets,
            trigger,
            variables: Mutex::new(defaults.clone()),
            defaults,
            outputs: Mutex::new(BTreeMap::new()),
            activations: Mutex::new(ActivationLog::default()),
            last_reply: Mutex::new(None),
            joins: Mutex::new(HashMap::new()),
            once_fired: Mutex::new(HashSet::new()),
        })
    }

    fn run_id(&self) -> RunId {
        self.shared.run_id
    }

    fn emit(&self, event: TraceEvent) {
        self.events.emit(event);
    }

    fn flat_outputs(&self) -> BTreeMap<String, Value> {
        locked(&self.outputs)
            .iter()
            .flat_map(|(node, sockets)| sockets.iter().map(move |(socket, value)| (format!("{}.{socket}", node.0), value.clone())))
            .collect()
    }

    fn summary(&self, started_at: chrono::DateTime<Utc>, outcome: &Result<(), SpellError>) -> RunSummary {
        let status = match outcome {
            Ok(()) => RunStatus::Succeeded,
            Err(SpellError::Cancelled) => RunStatus::Cancelled,
            Err(SpellError::Timeout) => RunStatus::TimedOut,
            Err(_) => RunStatus::Failed,
        };
        let outputs = self.flat_outputs();
        let log = locked(&self.activations);
        let activations =
            log.entries.iter().map(|(node_id, outputs)| EmittedOutput { node_id: node_id.clone(), outputs: NodeOutputs::clone(outputs) }).collect();
        RunSummary {
            run_id: self.run_id(),
            spell_id: self.spell_id,
            started_at,
            finished_at: Utc::now(),
            status,
            nodes_executed: self.shared.executed.load(Ordering::SeqCst),
            outputs,
            activations,
            activations_truncated: log.truncated,
            replies: locked(&self.shared.replies).replies.clone(),
            error: outcome.as_ref().err().map(ToString::to_string),
        }
    }

    fn incoming_flow_wires(&self, node: &NodeId) -> usize {
        let Some(executor) = self.executors.get(node) else {
            return 0;
        };
        self.wires_into
            .get(node)
            .into_iter()
            .flatten()
            .filter(|wire| executor.spec().resolve_input(&wire.to_socket).is_some_and(|socket| socket.ty.is_flow()))
            .count()
    }

    fn unwired_input(&self, node: &NodeId, socket: &str) -> Option<Value> {
        if *node == self.trigger.node && socket == TRIGGER_PAYLOAD_SOCKET {
            return Some(self.trigger.payload.clone());
        }
        if let Some(configured) = self.instances.get(node).and_then(|instance| instance.config.get(socket)) {
            return Some(Value::from(configured.clone()));
        }
        self.executors.get(node).and_then(|executor| executor.spec().input(socket)).and_then(|spec| spec.default.clone())
    }

    fn data_wire(&self, node: &NodeId, socket: &str) -> Option<&Wire> {
        self.wires_into.get(node).and_then(|wires| wires.iter().find(|wire| wire.to_socket == socket))
    }

    fn ready_input(&self, node: &NodeId, socket: &str, memo: &Memo, scope: &Scope) -> Readiness {
        let Some(wire) = self.data_wire(node, socket) else {
            return Readiness::Ready(self.unwired_input(node, socket), None);
        };
        let outputs = if self.impure.contains(&wire.from_node) {
            scope.get(&wire.from_node).cloned()
        } else {
            let cached = locked(memo).get(&wire.from_node).cloned();
            match cached {
                Some(outputs) => Some(outputs),
                None => return Readiness::Waiting(wire.from_node.clone()),
            }
        };
        let value = outputs.and_then(|outputs| outputs.get(&wire.from_socket).cloned());
        let event =
            TraceEvent::WireActivated { run_id: self.run_id(), wire_id: wire.id.clone(), at: Utc::now(), value: value.clone(), subgraph_path: Vec::new() };
        Readiness::Ready(value, Some(event))
    }

    async fn pull_value(self: &Arc<Self>, node: &NodeId, socket: &str, memo: &Memo, scope: &Scope) -> Result<Option<Value>, Fault> {
        if let Readiness::Waiting(source) = self.ready_input(node, socket, memo, scope) {
            self.materialize(source, memo, scope).await?;
        }
        match self.ready_input(node, socket, memo, scope) {
            Readiness::Ready(value, event) => {
                event.into_iter().for_each(|event| self.emit(event));
                Ok(value)
            }
            Readiness::Waiting(source) => Err(Fault::Node(NodeError::Other(format!("data node {:?} produced no outputs", source.0)))),
        }
    }

    async fn materialize(self: &Arc<Self>, target: NodeId, memo: &Memo, scope: &Scope) -> Result<(), Fault> {
        let mut stack = DataStack::new(Arc::clone(self));
        self.push_frame(target, memo, scope, &mut stack)?;
        std::future::poll_fn(|cx| self.drive(cx, &mut stack, memo, scope)).await
    }

    fn push_frame(self: &Arc<Self>, node: NodeId, memo: &Memo, scope: &Scope, stack: &mut DataStack) -> Result<(), Fault> {
        if !stack.pending.insert(node.clone()) {
            return Err(stack.fail(Fault::Node(NodeError::Other(format!("value cycle through node {:?}", node.0)))));
        }
        if self.shared.cancellation.is_cancelled() {
            return Err(stack.fail(Fault::Run(SpellError::Cancelled)));
        }
        self.shared.count_step().map_err(|error| stack.fail(Fault::Run(error)))?;
        let (Some(instance), Some(executor)) = (self.instances.get(&node).cloned(), self.executors.get(&node).cloned()) else {
            return Err(stack.fail(Fault::Node(NodeError::Other(format!("node {node:?} is not part of the spell")))));
        };
        self.emit(TraceEvent::NodeStarted { run_id: self.run_id(), node_id: node.clone(), at: Utc::now(), subgraph_path: Vec::new() });
        self.shared.executed.fetch_add(1, Ordering::SeqCst);
        let waiting_on = WaitCell::default();
        let mut context = ExecContext::data(Arc::clone(self), instance, memo.clone(), scope.clone(), Arc::clone(&waiting_on));
        let future: BoxFuture<Result<NodeOutcome, NodeError>> = Box::pin(async move { executor.execute(&mut context).await });
        stack.frames.push(DataFrame { node, started_at: Instant::now(), waiting_on, future });
        Ok(())
    }

    fn drive(self: &Arc<Self>, cx: &mut Context<'_>, stack: &mut DataStack, memo: &Memo, scope: &Scope) -> Poll<Result<(), Fault>> {
        loop {
            if self.shared.cancellation.is_cancelled() {
                return Poll::Ready(Err(stack.fail(Fault::Run(SpellError::Cancelled))));
            }
            let Some(top) = stack.frames.last_mut() else {
                return Poll::Ready(Ok(()));
            };
            let result = match top.future.as_mut().poll(cx) {
                Poll::Ready(result) => result,
                Poll::Pending => {
                    let Some(source) = locked(&top.waiting_on).take() else {
                        return Poll::Pending;
                    };
                    if locked(memo).contains_key(&source) {
                        continue;
                    }
                    if let Err(fault) = self.push_frame(source, memo, scope, stack) {
                        return Poll::Ready(Err(fault));
                    }
                    continue;
                }
            };
            let Some(frame) = stack.frames.pop() else {
                continue;
            };
            stack.pending.remove(&frame.node);
            if let Err(error) = result.and_then(|outcome| self.finish_data_node(&frame, outcome, memo)) {
                self.emit(TraceEvent::NodeFailed {
                    run_id: self.run_id(),
                    node_id: frame.node.clone(),
                    at: Utc::now(),
                    error: error.to_string(),
                    subgraph_path: Vec::new(),
                });
                return Poll::Ready(Err(stack.fail(Fault::Node(error))));
            }
        }
    }

    fn finish_data_node(&self, frame: &DataFrame, outcome: NodeOutcome, memo: &Memo) -> Result<(), NodeError> {
        self.record_replies(&outcome.replies)?;
        let outputs = Arc::new(outcome.outputs);
        locked(memo).insert(frame.node.clone(), Arc::clone(&outputs));
        locked(&self.outputs).insert(frame.node.clone(), outputs);
        self.emit(TraceEvent::NodeFinished {
            run_id: self.run_id(),
            node_id: frame.node.clone(),
            at: Utc::now(),
            duration_ms: elapsed_ms(frame.started_at),
            subgraph_path: Vec::new(),
        });
        Ok(())
    }

    fn record_replies(&self, replies: &[String]) -> Result<(), NodeError> {
        for reply in replies.iter().filter(|reply| !reply.is_empty()) {
            locked(&self.shared.replies).record(reply)?;
            *locked(&self.last_reply) = Some(reply.clone());
        }
        Ok(())
    }
    fn run_lane(self: Arc<Self>, first: Activation) -> BoxFuture<Result<(), SpellError>> {
        Box::pin(async move {
            let mut work = VecDeque::from([Work::Activate(first)]);
            while let Some(next) = work.pop_back() {
                if self.shared.cancellation.is_cancelled() {
                    return Err(SpellError::Cancelled);
                }
                match next {
                    Work::Activate(activation) => self.start(activation, &mut work).await?,
                    Work::Repeat(repeat) => self.step(repeat, &mut work).await?,
                }
            }
            Ok(())
        })
    }

    async fn start(self: &Arc<Self>, activation: Activation, work: &mut VecDeque<Work>) -> Result<(), SpellError> {
        let Activation { node, socket, via, scope } = activation;
        let kind = self.kind_of(&node);
        let scope = if kind == WAIT_FOR_ALL_KIND {
            match self.join_arrive(&node, via, scope) {
                Some(joined) => joined,
                None => return Ok(()),
            }
        } else {
            scope
        };
        if kind == DO_ONCE_KIND && !self.claim_once(&node, &socket) {
            return Ok(());
        }
        self.step(Repeat { node, socket, scope, iterations: 0 }, work).await
    }

    async fn step(self: &Arc<Self>, repeat: Repeat, work: &mut VecDeque<Work>) -> Result<(), SpellError> {
        let Repeat { node, socket, scope, iterations } = repeat;
        let (fired, outputs) = self.execute_flow_node(&node, &socket, &scope).await?;
        let next_scope = with_outputs(&scope, &node, outputs);
        if self.kind_of(&node) == WHILE_LOOP_KIND && fired.iter().any(|fired_socket| fired_socket == "body") {
            let max_iterations = self.max_iterations(&node).map_err(|error| self.fail_node(&node, error))?;
            let iterations = iterations + 1;
            if iterations > max_iterations {
                return Err(self.fail_node(&node, NodeError::Other(format!("while loop exceeded max_iterations {max_iterations}"))));
            }
            work.push_back(Work::Repeat(Repeat { node: node.clone(), socket, scope, iterations }));
        }
        self.fan_out(&node, &fired, next_scope, work).await
    }

    fn kind_of(&self, node: &NodeId) -> &str {
        self.instances.get(node).map_or("", |instance| instance.kind.as_str())
    }

    fn max_iterations(&self, node: &NodeId) -> Result<u64, NodeError> {
        let Some(setting) = self.instances.get(node).and_then(|instance| instance.config.get("max_iterations")) else {
            return Ok(DEFAULT_WHILE_LOOP_MAX_ITERATIONS);
        };
        let Some(name) = bound_variable(setting) else {
            return Ok(whole_number(setting).unwrap_or(DEFAULT_WHILE_LOOP_MAX_ITERATIONS).max(1));
        };
        let value = locked(&self.variables).get(name).map(Value::to_json);
        let whole = value.as_ref().and_then(whole_number);
        whole.filter(|limit| *limit >= 1).ok_or_else(|| {
            let shown = value.map_or_else(|| "nothing".to_owned(), |value| value.to_string());
            NodeError::Other(format!("while loop max_iterations must be a whole number of at least 1, but {{{{{name}}}}} is {shown}"))
        })
    }

    fn fail_node(&self, node: &NodeId, error: NodeError) -> SpellError {
        self.emit(TraceEvent::NodeFailed { run_id: self.run_id(), node_id: node.clone(), at: Utc::now(), error: error.to_string(), subgraph_path: Vec::new() });
        SpellError::Node { node: node.clone(), source: error }
    }

    fn fault_on(&self, node: &NodeId, fault: Fault) -> SpellError {
        match fault {
            Fault::Node(NodeError::Cancelled) | Fault::Run(SpellError::Cancelled) => {
                self.emit(TraceEvent::NodeFailed {
                    run_id: self.run_id(),
                    node_id: node.clone(),
                    at: Utc::now(),
                    error: NodeError::Cancelled.to_string(),
                    subgraph_path: Vec::new(),
                });
                SpellError::Cancelled
            }
            Fault::Node(error) => self.fail_node(node, error),
            Fault::Run(error) => {
                self.emit(TraceEvent::NodeFailed {
                    run_id: self.run_id(),
                    node_id: node.clone(),
                    at: Utc::now(),
                    error: error.to_string(),
                    subgraph_path: Vec::new(),
                });
                error
            }
        }
    }

    fn join_arrive(&self, node: &NodeId, via: Option<WireId>, scope: Scope) -> Option<Scope> {
        let expected = self.incoming_flow_wires(node).max(1);
        let mut joins = locked(&self.joins);
        let queues = joins.entry(node.clone()).or_default();
        queues.entry(via).or_default().push_back(scope);
        let arrived = queues.values().filter(|queue| !queue.is_empty()).count();
        if arrived < expected {
            return None;
        }
        let round: Vec<Scope> = queues.values_mut().filter_map(VecDeque::pop_front).collect();
        queues.retain(|_, queue| !queue.is_empty());
        if queues.is_empty() {
            joins.remove(node);
        }
        Some(merge_scopes(round))
    }

    fn check_joins_complete(&self) -> Result<(), SpellError> {
        let joins = locked(&self.joins);
        let mut waiting: Vec<(&NodeId, usize)> =
            joins.iter().map(|(node, queues)| (node, queues.values().filter(|queue| !queue.is_empty()).count())).filter(|(_, arrived)| *arrived > 0).collect();
        waiting.sort();
        let Some((node, arrived)) = waiting.first() else {
            return Ok(());
        };
        let expected = self.incoming_flow_wires(node);
        let message = format!("join incomplete: node {:?} received {arrived} of {expected} branches", node.0);
        self.emit(TraceEvent::NodeFailed {
            run_id: self.run_id(),
            node_id: (*node).clone(),
            at: Utc::now(),
            error: message.clone(),
            subgraph_path: Vec::new(),
        });
        Err(SpellError::Internal(message))
    }

    fn claim_once(&self, node: &NodeId, socket: &str) -> bool {
        let mut fired = locked(&self.once_fired);
        if socket == "reset" {
            fired.remove(node);
            return false;
        }
        fired.insert(node.clone())
    }

    async fn execute_flow_node(self: &Arc<Self>, node: &NodeId, socket: &str, scope: &Scope) -> Result<(Vec<String>, SharedOutputs), SpellError> {
        self.shared.count_step()?;
        let (Some(instance), Some(executor)) = (self.instances.get(node).cloned(), self.executors.get(node).cloned()) else {
            return Err(SpellError::Internal(format!("node {node:?} is not part of the spell")));
        };
        let started = Instant::now();
        self.emit(TraceEvent::NodeStarted { run_id: self.run_id(), node_id: node.clone(), at: Utc::now(), subgraph_path: Vec::new() });
        self.shared.executed.fetch_add(1, Ordering::SeqCst);
        let mut context = ExecContext::flow(self.clone(), instance, socket, scope.clone());
        let cancellation = self.shared.cancellation.clone();
        let result = tokio::select! {
            result = self.run_node(&mut context, executor) => result,
            () = cancellation.cancelled() => Err(Fault::Node(NodeError::Cancelled)),
        };
        let outputs = match result.and_then(|outcome| self.accept_outcome(outcome)) {
            Ok((fire, outputs)) => (fire, outputs),
            Err(fault) => return Err(self.fault_on(node, fault)),
        };
        locked(&self.outputs).insert(node.clone(), Arc::clone(&outputs.1));
        locked(&self.activations).record(node, &outputs.1);
        self.emit(TraceEvent::NodeFinished {
            run_id: self.run_id(),
            node_id: node.clone(),
            at: Utc::now(),
            duration_ms: elapsed_ms(started),
            subgraph_path: Vec::new(),
        });
        Ok(outputs)
    }

    fn accept_outcome(&self, outcome: NodeOutcome) -> Result<(Vec<String>, SharedOutputs), Fault> {
        self.record_replies(&outcome.replies).map_err(Fault::Node)?;
        Ok((outcome.fire, Arc::new(outcome.outputs)))
    }

    async fn run_node(self: &Arc<Self>, context: &mut ExecContext, executor: Arc<dyn NodeExecutor>) -> Result<NodeOutcome, Fault> {
        let result = if context.node.kind == SUBGRAPH_KIND { self.run_subgraph(context).await } else { executor.execute(context).await.map_err(Fault::Node) };
        match context.take_fault() {
            Some(fault) => Err(Fault::Run(fault)),
            None => result,
        }
    }

    async fn run_subgraph(self: &Arc<Self>, context: &mut ExecContext) -> Result<NodeOutcome, Fault> {
        let node = context.node.id.clone();
        if self.depth >= MAX_SUBGRAPH_DEPTH {
            return Err(Fault::Node(NodeError::Other(format!("subgraph nesting exceeds {MAX_SUBGRAPH_DEPTH} levels"))));
        }
        let graph = self
            .subgraph_targets
            .get(&node)
            .and_then(|key| self.shared.children.get(key))
            .cloned()
            .ok_or_else(|| Fault::Node(NodeError::InvalidConfig(format!("subgraph called by node {:?} was not resolved before the run", node.0))))?;
        let payload = context.input(TRIGGER_PAYLOAD_SOCKET).await.map_err(Fault::Node)?.unwrap_or_default();
        let trigger = child_trigger(&self.shared, &graph, payload).map_err(Fault::Run)?;
        let events: Arc<dyn EventEmitter> = Arc::new(QualifiedEvents { inner: Arc::clone(&self.events), caller: node });
        let child = Arc::new(RunState::prepare(Arc::clone(&self.shared), graph, trigger, self.depth + 1, events).map_err(Fault::Run)?);
        let first = Activation { node: child.trigger.node.clone(), socket: child.trigger.socket.clone(), via: None, scope: Scope::default() };
        Arc::clone(&child).run_lane(first).await.map_err(Fault::Run)?;
        child.check_joins_complete().map_err(Fault::Run)?;
        let result = locked(&child.last_reply).clone().map_or(Value::Null, Value::String);
        let outputs: serde_json::Map<String, serde_json::Value> = child.flat_outputs().into_iter().map(|(key, value)| (key, value.to_json())).collect();
        Ok(NodeOutcome::fire("out").with_output("result", result).with_output("outputs", Value::Json(serde_json::Value::Object(outputs))))
    }
    async fn fan_out(self: &Arc<Self>, node: &NodeId, sockets: &[String], scope: Scope, work: &mut VecDeque<Work>) -> Result<(), SpellError> {
        let wires: Vec<Wire> = self.graph.wires_out_of(node).filter(|wire| sockets.contains(&wire.from_socket)).cloned().collect();
        for wire in &wires {
            self.emit(TraceEvent::WireActivated { run_id: self.run_id(), wire_id: wire.id.clone(), at: Utc::now(), value: None, subgraph_path: Vec::new() });
        }
        let activation = |wire: Wire, scope: Scope| Activation { node: wire.to_node, socket: wire.to_socket, via: Some(wire.id), scope };
        if wires.len() <= 1 {
            work.extend(wires.into_iter().map(|wire| Work::Activate(activation(wire, scope.clone()))));
            return Ok(());
        }
        let mut branches = JoinSet::new();
        for wire in wires {
            branches.spawn(Arc::clone(self).run_lane(activation(wire, scope.clone())));
        }
        while let Some(joined) = branches.join_next().await {
            let failure = match joined {
                Ok(Ok(())) => continue,
                Ok(Err(error)) => error,
                Err(join_error) => SpellError::Internal(join_error.to_string()),
            };
            branches.abort_all();
            return Err(failure);
        }
        Ok(())
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

enum Inputs {
    Flow { fault: Option<SpellError> },
    Data { waiting_on: WaitCell },
}

struct ExecContext {
    state: Arc<RunState>,
    node: NodeInstance,
    trigger_socket: Option<String>,
    memo: Memo,
    scope: Scope,
    inputs: Inputs,
}

pub const UNBOUND_SETTINGS: [&str; 3] = ["template", "prompt", "script"];

pub fn whole_number(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_f64().filter(|number| *number >= 0.0 && number.fract() == 0.0 && *number <= u64::MAX as f64).map(|number| number as u64))
}

pub fn bound_variable(value: &serde_json::Value) -> Option<&str> {
    value.as_str()?.trim().strip_prefix("{{")?.strip_suffix("}}").map(str::trim).filter(|name| !name.is_empty() && !name.contains(['{', '}']))
}

fn bound_settings(state: &RunState, mut node: NodeInstance) -> NodeInstance {
    if let Some(settings) = node.config.as_object_mut() {
        let variables = locked(&state.variables);
        for (_, value) in settings.iter_mut().filter(|(key, _)| !UNBOUND_SETTINGS.contains(&key.as_str())) {
            if let Some(found) = bound_variable(value).and_then(|name| variables.get(name)) {
                *value = found.to_json();
            }
        }
    }
    node
}

impl ExecContext {
    fn flow(state: Arc<RunState>, node: NodeInstance, socket: &str, scope: Scope) -> Self {
        let node = bound_settings(&state, node);
        Self { state, node, trigger_socket: Some(socket.to_owned()), memo: Memo::default(), scope, inputs: Inputs::Flow { fault: None } }
    }

    fn data(state: Arc<RunState>, node: NodeInstance, memo: Memo, scope: Scope, waiting_on: WaitCell) -> Self {
        let node = bound_settings(&state, node);
        Self { state, node, trigger_socket: None, memo, scope, inputs: Inputs::Data { waiting_on } }
    }

    async fn data_input(&self, socket: &str, waiting_on: WaitCell) -> Option<Value> {
        loop {
            match self.state.ready_input(&self.node.id, socket, &self.memo, &self.scope) {
                Readiness::Ready(value, event) => {
                    event.into_iter().for_each(|event| self.state.emit(event));
                    return value;
                }
                Readiness::Waiting(source) => SuspendUntilDataReady { memo: self.memo.clone(), waiting_on: Arc::clone(&waiting_on), source }.await,
            }
        }
    }

    fn take_fault(&mut self) -> Option<SpellError> {
        match &mut self.inputs {
            Inputs::Flow { fault } => fault.take(),
            Inputs::Data { .. } => None,
        }
    }
}

#[async_trait]
impl NodeContext for ExecContext {
    fn run_id(&self) -> RunId {
        self.state.run_id()
    }

    fn node(&self) -> &NodeInstance {
        &self.node
    }

    fn trigger_socket(&self) -> Option<&str> {
        self.trigger_socket.as_deref()
    }

    async fn input(&mut self, socket: &str) -> Result<Option<Value>, NodeError> {
        match &mut self.inputs {
            Inputs::Data { waiting_on } => {
                let waiting_on = Arc::clone(waiting_on);
                Ok(self.data_input(socket, waiting_on).await)
            }
            Inputs::Flow { fault } => match self.state.pull_value(&self.node.id, socket, &self.memo, &self.scope).await {
                Ok(value) => Ok(value),
                Err(Fault::Node(error)) => Err(error),
                Err(Fault::Run(error)) => {
                    let message = error.to_string();
                    *fault = Some(error);
                    Err(NodeError::Other(message))
                }
            },
        }
    }

    fn variable(&self, name: &str) -> Option<Value> {
        locked(&self.state.variables).get(name).cloned()
    }

    fn variable_default(&self, name: &str) -> Option<Value> {
        self.state.defaults.get(name).cloned()
    }

    fn set_variable(&mut self, name: &str, value: Value) -> Result<(), NodeError> {
        locked(&self.state.variables).insert(name.to_owned(), value.clone());
        self.state.emit(TraceEvent::VariableSet { run_id: self.state.run_id(), name: name.to_owned(), value, subgraph_path: Vec::new() });
        Ok(())
    }

    fn llm(&self) -> Option<Arc<dyn LlmGateway>> {
        self.state.shared.services.llm.clone()
    }

    fn memory(&self) -> Option<Arc<dyn MemoryService>> {
        self.state.shared.services.memory.clone()
    }

    fn knowledge(&self) -> Option<Arc<dyn KnowledgeService>> {
        self.state.shared.services.knowledge.clone()
    }

    fn events(&self) -> Arc<dyn EventEmitter> {
        Arc::clone(&self.state.events)
    }

    fn cancellation(&self) -> CancellationToken {
        self.state.shared.cancellation.clone()
    }

    fn agent_id(&self) -> Option<&str> {
        self.state.shared.binding.agent_id.as_deref()
    }

    fn allow_private_http(&self) -> bool {
        self.state.shared.binding.allow_private_http.unwrap_or(false)
    }
}
