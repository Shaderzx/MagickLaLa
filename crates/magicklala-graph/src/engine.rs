use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{
    EmittedOutput, EventEmitter, KnowledgeService, LlmGateway, MemoryService, NodeContext, NodeError, NodeExecutor, NodeId, NodeInstance, NodeOutcome,
    NodeSpec, RunId, RunStatus, RunSummary, SpellError, SpellGraph, SpellId, SpellValidator, SubgraphRef, TraceEvent, ValidationError, Value, Wire, WireId,
};

pub const WAIT_FOR_ALL_KIND: &str = "flow/wait_for_all";
pub const WHILE_LOOP_KIND: &str = "flow/while_loop";
pub const DO_ONCE_KIND: &str = "flow/do_once";
pub const SUBGRAPH_KIND: &str = "flow/subgraph";
pub const START_TRIGGER_KIND: &str = "event/on_start";
pub const TRIGGER_PAYLOAD_SOCKET: &str = "payload";
pub const REPLY_SOCKETS: [&str; 2] = ["sent", "reply"];

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type NodeOutputs = BTreeMap<String, Value>;
type SharedOutputs = Arc<NodeOutputs>;
type Memo = Arc<Mutex<HashMap<NodeId, SharedOutputs>>>;
type Scope = Arc<BTreeMap<NodeId, SharedOutputs>>;
type JoinQueues = HashMap<Option<WireId>, VecDeque<Scope>>;
type SubgraphKey = (SpellId, Option<u32>);
type ResolvedSubgraphs = HashMap<SubgraphKey, Arc<SpellGraph>>;

pub const DEFAULT_WHILE_LOOP_MAX_ITERATIONS: u64 = 1000;
pub const MAX_ACTIVATION_LOG_BYTES: usize = 1024 * 1024;
pub const MAX_REPLIES: usize = 1000;
pub const MAX_REPLY_BYTES: usize = 1024 * 1024;
pub const MAX_SUBGRAPH_DEPTH: usize = 16;

pub trait NodeCatalog: Send + Sync {
    fn executor(&self, kind: &str) -> Option<Arc<dyn NodeExecutor>>;
    fn specs(&self) -> Vec<NodeSpec>;
}

pub trait SubgraphResolver: Send + Sync {
    fn resolve(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<SpellGraph>;
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
        let children = self.check_subgraphs(graph, &validator)?;
        let binding = RunBinding {
            agent_id: binding.agent_id.or_else(|| self.binding.agent_id.clone()),
            allow_private_http: binding.allow_private_http.or(self.binding.allow_private_http),
            run_id: binding.run_id.or(self.binding.run_id),
        };
        let shared = Arc::new(RunShared {
            run_id: binding.run_id.unwrap_or_default(),
            catalog: Arc::clone(&self.catalog),
            services: self.services.clone(),
            events,
            cancellation,
            step_limit: self.step_limit,
            steps: AtomicU64::new(0),
            executed: AtomicU64::new(0),
            replies: Mutex::new(ReplyLog::default()),
            children,
            binding,
        });
        let state = Arc::new(RunState::prepare(Arc::clone(&shared), Arc::new(graph.clone()), trigger, 0)?);
        let started_at = Utc::now();
        state.emit(TraceEvent::RunStarted { run_id: shared.run_id, spell_id: graph.id, at: started_at });
        let first = Activation { node: state.trigger.node.clone(), socket: state.trigger.socket.clone(), via: None, scope: Scope::default() };
        let activated = state.clone().run_lane(first).await;
        let outcome = activated.and_then(|()| state.check_joins_complete());
        let summary = state.summary(started_at, &outcome);
        state.emit(TraceEvent::RunFinished { run_id: shared.run_id, summary: summary.slim() });
        outcome.map(|()| summary)
    }

    fn check_subgraphs(&self, graph: &SpellGraph, validator: &SpellValidator) -> Result<ResolvedSubgraphs, SpellError> {
        let Some(resolver) = &self.subgraphs else {
            return unresolvable_subgraphs(graph).map(|()| ResolvedSubgraphs::new());
        };
        resolve_subgraphs(resolver.as_ref(), validator, graph)
    }
}

struct SubgraphFrame {
    references: Vec<SubgraphRef>,
    cursor: usize,
    key: Option<SubgraphKey>,
}

fn resolve_subgraphs(resolver: &dyn SubgraphResolver, validator: &SpellValidator, root: &SpellGraph) -> Result<ResolvedSubgraphs, SpellError> {
    let mut resolved = ResolvedSubgraphs::new();
    let mut finished: HashSet<SubgraphKey> = HashSet::new();
    let mut path = vec![root.id];
    let mut frames = vec![SubgraphFrame { references: root.subgraphs.clone(), cursor: 0, key: None }];
    loop {
        let Some(frame) = frames.last_mut() else {
            return Ok(resolved);
        };
        let next = frame.references.get(frame.cursor).cloned();
        frame.cursor += 1;
        let Some(reference) = next else {
            if let Some(key) = frames.pop().and_then(|done| done.key) {
                finished.insert(key);
            }
            path.pop();
            continue;
        };
        let key = (reference.spell_id, reference.release_version);
        if let Some(start) = path.iter().position(|ancestor| *ancestor == reference.spell_id) {
            let chain: Vec<String> = path[start..].iter().chain(std::iter::once(&reference.spell_id)).map(|spell| spell.0.to_string()).collect();
            return Err(SpellError::Internal(format!("subgraph cycle through node {:?}: {}", reference.node_id.0, chain.join(" -> "))));
        }
        if finished.contains(&key) {
            continue;
        }
        if frames.len() > MAX_SUBGRAPH_DEPTH {
            return Err(SpellError::Internal(format!("subgraph nesting through node {:?} exceeds {MAX_SUBGRAPH_DEPTH} levels", reference.node_id.0)));
        }
        let child = resolver
            .resolve(reference.spell_id, reference.release_version)
            .ok_or_else(|| SpellError::Internal(format!("subgraph {:?} for node {:?} cannot be resolved", reference.spell_id, reference.node_id)))?;
        validator.validate(&child).map_err(SpellError::Validation)?;
        frames.push(SubgraphFrame { references: child.subgraphs.clone(), cursor: 0, key: Some(key) });
        path.push(reference.spell_id);
        resolved.insert(key, Arc::new(child));
    }
}

fn unresolvable_subgraphs(graph: &SpellGraph) -> Result<(), SpellError> {
    if graph.subgraphs.is_empty() {
        return Ok(());
    }
    Err(SpellError::Validation(
        graph
            .subgraphs
            .iter()
            .map(|reference| ValidationError::SubgraphResolverUnavailable { node: reference.node_id.clone(), spell: reference.spell_id })
            .collect(),
    ))
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

fn reply_of(outputs: &NodeOutputs) -> Option<&str> {
    REPLY_SOCKETS.iter().find_map(|socket| match outputs.get(*socket) {
        Some(Value::String(text)) if !text.is_empty() => Some(text.as_str()),
        _ => None,
    })
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
    events: Arc<dyn EventEmitter>,
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
    fn prepare(shared: Arc<RunShared>, graph: Arc<SpellGraph>, trigger: RunTrigger, depth: usize) -> Result<Self, SpellError> {
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
        self.shared.events.emit(event);
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

    fn pure_sources<'a>(&'a self, node: &NodeId) -> impl Iterator<Item = &'a NodeId> + 'a {
        self.wires_into.get(node).into_iter().flatten().map(|wire| &wire.from_node).filter(|source| !self.impure.contains(*source))
    }

    fn pure_upstream(&self, starts: Vec<NodeId>, memo: &Memo) -> Vec<NodeId> {
        let cached: HashSet<NodeId> = locked(memo).keys().cloned().collect();
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut order = Vec::new();
        let mut stack: Vec<(NodeId, bool)> = starts.into_iter().rev().map(|node| (node, false)).collect();
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                order.push(node);
                continue;
            }
            if cached.contains(&node) || !seen.insert(node.clone()) {
                continue;
            }
            let sources: Vec<NodeId> = self.pure_sources(&node).filter(|source| !seen.contains(*source) && !cached.contains(*source)).cloned().collect();
            stack.push((node, true));
            stack.extend(sources.into_iter().rev().map(|source| (source, false)));
        }
        order
    }

    async fn evaluate_upstream(self: &Arc<Self>, starts: Vec<NodeId>, memo: &Memo, scope: &Scope) -> Result<(), Fault> {
        for node in self.pure_upstream(starts, memo) {
            if self.shared.cancellation.is_cancelled() {
                return Err(Fault::Run(SpellError::Cancelled));
            }
            self.shared.count_step().map_err(Fault::Run)?;
            let outputs = Arc::clone(self).evaluate_pure(node.clone(), memo.clone(), scope.clone()).await.map_err(Fault::Node)?;
            locked(memo).insert(node, outputs);
        }
        Ok(())
    }

    fn pull(self: Arc<Self>, node: NodeId, socket: String, memo: Memo, scope: Scope) -> BoxFuture<Result<Option<Value>, NodeError>> {
        Box::pin(async move {
            let wire = self.wires_into.get(&node).and_then(|wires| wires.iter().find(|wire| wire.to_socket == socket)).cloned();
            let Some(wire) = wire else {
                return Ok(self.unwired_input(&node, &socket));
            };
            let value = if self.impure.contains(&wire.from_node) {
                scope.get(&wire.from_node).and_then(|outputs| outputs.get(&wire.from_socket)).cloned()
            } else {
                if !locked(&memo).contains_key(&wire.from_node) {
                    self.evaluate_upstream(vec![wire.from_node.clone()], &memo, &scope).await.map_err(|fault| match fault {
                        Fault::Node(error) => error,
                        Fault::Run(error) => NodeError::Other(error.to_string()),
                    })?;
                }
                locked(&memo).get(&wire.from_node).and_then(|outputs| outputs.get(&wire.from_socket)).cloned()
            };
            self.emit(TraceEvent::WireActivated { run_id: self.run_id(), wire_id: wire.id.clone(), at: Utc::now(), value: value.clone() });
            Ok(value)
        })
    }

    fn evaluate_pure(self: Arc<Self>, node: NodeId, memo: Memo, scope: Scope) -> BoxFuture<Result<SharedOutputs, NodeError>> {
        Box::pin(async move {
            let (Some(instance), Some(executor)) = (self.instances.get(&node).cloned(), self.executors.get(&node).cloned()) else {
                return Err(NodeError::Other(format!("node {node:?} is not part of the spell")));
            };
            let started = Instant::now();
            self.emit(TraceEvent::NodeStarted { run_id: self.run_id(), node_id: node.clone(), at: Utc::now() });
            self.shared.executed.fetch_add(1, Ordering::SeqCst);
            let mut context = ExecContext { state: self.clone(), node: instance, trigger_socket: None, memo, scope };
            let result = executor.execute(&mut context).await;
            let recorded = result.and_then(|outcome| {
                let outputs = Arc::new(outcome.outputs);
                self.record_reply(&outputs)?;
                Ok(outputs)
            });
            match recorded {
                Ok(outputs) => {
                    locked(&self.outputs).insert(node.clone(), Arc::clone(&outputs));
                    self.emit(TraceEvent::NodeFinished { run_id: self.run_id(), node_id: node, at: Utc::now(), duration_ms: elapsed_ms(started) });
                    Ok(outputs)
                }
                Err(error) => {
                    self.emit(TraceEvent::NodeFailed { run_id: self.run_id(), node_id: node, at: Utc::now(), error: error.to_string() });
                    Err(error)
                }
            }
        })
    }

    fn record_reply(&self, outputs: &NodeOutputs) -> Result<(), NodeError> {
        let Some(reply) = reply_of(outputs) else {
            return Ok(());
        };
        locked(&self.shared.replies).record(reply)?;
        *locked(&self.last_reply) = Some(reply.to_owned());
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
            let max_iterations = self.max_iterations(&node);
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

    fn max_iterations(&self, node: &NodeId) -> u64 {
        self.instances
            .get(node)
            .and_then(|instance| instance.config.get("max_iterations"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_WHILE_LOOP_MAX_ITERATIONS)
            .max(1)
    }

    fn fail_node(&self, node: &NodeId, error: NodeError) -> SpellError {
        self.emit(TraceEvent::NodeFailed { run_id: self.run_id(), node_id: node.clone(), at: Utc::now(), error: error.to_string() });
        SpellError::Node { node: node.clone(), source: error }
    }

    fn fault_on(&self, node: &NodeId, fault: Fault) -> SpellError {
        match fault {
            Fault::Node(NodeError::Cancelled) | Fault::Run(SpellError::Cancelled) => {
                self.emit(TraceEvent::NodeFailed { run_id: self.run_id(), node_id: node.clone(), at: Utc::now(), error: NodeError::Cancelled.to_string() });
                SpellError::Cancelled
            }
            Fault::Node(error) => self.fail_node(node, error),
            Fault::Run(error) => {
                self.emit(TraceEvent::NodeFailed { run_id: self.run_id(), node_id: node.clone(), at: Utc::now(), error: error.to_string() });
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
        self.emit(TraceEvent::NodeFailed { run_id: self.run_id(), node_id: (*node).clone(), at: Utc::now(), error: message.clone() });
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
        self.emit(TraceEvent::NodeStarted { run_id: self.run_id(), node_id: node.clone(), at: Utc::now() });
        self.shared.executed.fetch_add(1, Ordering::SeqCst);
        let mut context =
            ExecContext { state: self.clone(), node: instance, trigger_socket: Some(socket.to_owned()), memo: Memo::default(), scope: scope.clone() };
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
        self.emit(TraceEvent::NodeFinished { run_id: self.run_id(), node_id: node.clone(), at: Utc::now(), duration_ms: elapsed_ms(started) });
        Ok(outputs)
    }

    fn accept_outcome(&self, outcome: NodeOutcome) -> Result<(Vec<String>, SharedOutputs), Fault> {
        let outputs = Arc::new(outcome.outputs);
        self.record_reply(&outputs).map_err(Fault::Node)?;
        Ok((outcome.fire, outputs))
    }

    async fn run_node(self: &Arc<Self>, context: &mut ExecContext, executor: Arc<dyn NodeExecutor>) -> Result<NodeOutcome, Fault> {
        let sources: Vec<NodeId> = self.pure_sources(&context.node.id).cloned().collect();
        self.evaluate_upstream(sources, &context.memo, &context.scope).await?;
        if context.node.kind == SUBGRAPH_KIND {
            return self.run_subgraph(context).await;
        }
        executor.execute(context).await.map_err(Fault::Node)
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
        let child = Arc::new(RunState::prepare(Arc::clone(&self.shared), graph, trigger, self.depth + 1).map_err(Fault::Run)?);
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
            self.emit(TraceEvent::WireActivated { run_id: self.run_id(), wire_id: wire.id.clone(), at: Utc::now(), value: None });
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

struct ExecContext {
    state: Arc<RunState>,
    node: NodeInstance,
    trigger_socket: Option<String>,
    memo: Memo,
    scope: Scope,
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
        self.state.clone().pull(self.node.id.clone(), socket.to_owned(), self.memo.clone(), self.scope.clone()).await
    }

    fn variable(&self, name: &str) -> Option<Value> {
        locked(&self.state.variables).get(name).cloned()
    }

    fn variable_default(&self, name: &str) -> Option<Value> {
        self.state.defaults.get(name).cloned()
    }

    fn set_variable(&mut self, name: &str, value: Value) -> Result<(), NodeError> {
        locked(&self.state.variables).insert(name.to_owned(), value.clone());
        self.state.emit(TraceEvent::VariableSet { run_id: self.state.run_id(), name: name.to_owned(), value });
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
        self.state.shared.events.clone()
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
