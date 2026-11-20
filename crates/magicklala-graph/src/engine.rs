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
    EventEmitter, KnowledgeService, LlmGateway, MemoryService, NodeContext, NodeError, NodeExecutor, NodeId, NodeInstance, NodeSpec, RunId, RunStatus,
    RunSummary, SpellError, SpellGraph, SpellId, SpellValidator, TraceEvent, Value, Wire, WireId,
};

pub const WAIT_FOR_ALL_KIND: &str = "flow/wait_for_all";
pub const WHILE_LOOP_KIND: &str = "flow/while_loop";
pub const DO_ONCE_KIND: &str = "flow/do_once";
pub const TRIGGER_PAYLOAD_SOCKET: &str = "payload";

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type NodeOutputs = BTreeMap<String, Value>;
type Memo = Arc<Mutex<HashMap<NodeId, NodeOutputs>>>;
type Scope = Arc<BTreeMap<NodeId, NodeOutputs>>;
type JoinQueues = HashMap<Option<WireId>, VecDeque<Scope>>;

pub const DEFAULT_WHILE_LOOP_MAX_ITERATIONS: u64 = 1000;

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
        self.check_subgraphs(graph, &validator)?;
        let binding = RunBinding {
            agent_id: binding.agent_id.or_else(|| self.binding.agent_id.clone()),
            allow_private_http: binding.allow_private_http.or(self.binding.allow_private_http),
        };
        let state = Arc::new(RunState::prepare(self, graph, trigger, events, cancellation, binding)?);
        let started_at = Utc::now();
        state.emit(TraceEvent::RunStarted { run_id: state.run_id, spell_id: graph.id, at: started_at });
        let activated = state.clone().activate(state.trigger.node.clone(), state.trigger.socket.clone(), None, Scope::default()).await;
        let outcome = activated.and_then(|()| state.check_joins_complete());
        let summary = state.summary(started_at, &outcome);
        state.emit(TraceEvent::RunFinished { run_id: state.run_id, summary: summary.clone() });
        outcome.map(|()| summary)
    }

    fn check_subgraphs(&self, graph: &SpellGraph, validator: &SpellValidator) -> Result<(), SpellError> {
        let Some(resolver) = &self.subgraphs else {
            return Ok(());
        };
        let mut path = vec![graph.id];
        let mut finished = HashSet::new();
        visit_subgraphs(resolver.as_ref(), validator, graph, &mut path, &mut finished)
    }
}

fn visit_subgraphs(
    resolver: &dyn SubgraphResolver,
    validator: &SpellValidator,
    graph: &SpellGraph,
    path: &mut Vec<SpellId>,
    finished: &mut HashSet<SpellId>,
) -> Result<(), SpellError> {
    for reference in &graph.subgraphs {
        if let Some(start) = path.iter().position(|ancestor| *ancestor == reference.spell_id) {
            let chain: Vec<String> = path[start..].iter().chain(std::iter::once(&reference.spell_id)).map(|spell| spell.0.to_string()).collect();
            return Err(SpellError::Internal(format!("subgraph cycle through node {:?}: {}", reference.node_id.0, chain.join(" -> "))));
        }
        if finished.contains(&reference.spell_id) {
            continue;
        }
        let child = resolver
            .resolve(reference.spell_id, reference.release_version)
            .ok_or_else(|| SpellError::Internal(format!("subgraph {:?} for node {:?} cannot be resolved", reference.spell_id, reference.node_id)))?;
        validator.validate(&child).map_err(SpellError::Validation)?;
        path.push(reference.spell_id);
        let nested = visit_subgraphs(resolver, validator, &child, path, finished);
        path.pop();
        nested?;
        finished.insert(reference.spell_id);
    }
    Ok(())
}

fn with_outputs(scope: &Scope, node: &NodeId, outputs: NodeOutputs) -> Scope {
    let mut next = BTreeMap::clone(scope);
    next.insert(node.clone(), outputs);
    Arc::new(next)
}

fn merge_scopes(scopes: impl IntoIterator<Item = Scope>) -> Scope {
    let mut merged = BTreeMap::new();
    for scope in scopes {
        merged.extend(scope.iter().map(|(node, outputs)| (node.clone(), outputs.clone())));
    }
    Arc::new(merged)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct RunState {
    run_id: RunId,
    spell_id: SpellId,
    graph: Arc<SpellGraph>,
    executors: HashMap<NodeId, Arc<dyn NodeExecutor>>,
    instances: HashMap<NodeId, NodeInstance>,
    impure: HashSet<NodeId>,
    services: EngineServices,
    events: Arc<dyn EventEmitter>,
    cancellation: CancellationToken,
    step_limit: u64,
    steps: AtomicU64,
    executed: AtomicU64,
    trigger: RunTrigger,
    defaults: BTreeMap<String, Value>,
    variables: Mutex<BTreeMap<String, Value>>,
    outputs: Mutex<BTreeMap<NodeId, NodeOutputs>>,
    joins: Mutex<HashMap<NodeId, JoinQueues>>,
    once_fired: Mutex<HashSet<NodeId>>,
    binding: RunBinding,
}

impl RunState {
    fn prepare(
        engine: &SpellEngine,
        graph: &SpellGraph,
        trigger: RunTrigger,
        events: Arc<dyn EventEmitter>,
        cancellation: CancellationToken,
        binding: RunBinding,
    ) -> Result<Self, SpellError> {
        let mut executors = HashMap::new();
        let mut instances = HashMap::new();
        let mut impure = HashSet::new();
        for node in &graph.nodes {
            let executor = engine.catalog.executor(&node.kind).ok_or_else(|| SpellError::UnknownNodeKind(node.kind.clone()))?;
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
        let defaults: BTreeMap<String, Value> =
            graph.variables.iter().map(|variable| (variable.name.clone(), variable.default.clone().unwrap_or_default())).collect();
        Ok(Self {
            run_id: RunId::new(),
            spell_id: graph.id,
            graph: Arc::new(graph.clone()),
            executors,
            instances,
            impure,
            services: engine.services.clone(),
            events,
            cancellation,
            step_limit: engine.step_limit,
            steps: AtomicU64::new(0),
            executed: AtomicU64::new(0),
            trigger,
            variables: Mutex::new(defaults.clone()),
            defaults,
            outputs: Mutex::new(BTreeMap::new()),
            joins: Mutex::new(HashMap::new()),
            once_fired: Mutex::new(HashSet::new()),
            binding,
        })
    }

    fn emit(&self, event: TraceEvent) {
        self.events.emit(event);
    }

    fn summary(&self, started_at: chrono::DateTime<Utc>, outcome: &Result<(), SpellError>) -> RunSummary {
        let status = match outcome {
            Ok(()) => RunStatus::Succeeded,
            Err(SpellError::Cancelled) => RunStatus::Cancelled,
            Err(SpellError::Timeout) => RunStatus::TimedOut,
            Err(_) => RunStatus::Failed,
        };
        let outputs = locked(&self.outputs)
            .iter()
            .flat_map(|(node, sockets)| sockets.iter().map(move |(socket, value)| (format!("{}.{socket}", node.0), value.clone())))
            .collect();
        RunSummary {
            run_id: self.run_id,
            spell_id: self.spell_id,
            started_at,
            finished_at: Utc::now(),
            status,
            nodes_executed: self.executed.load(Ordering::SeqCst),
            outputs,
            error: outcome.as_ref().err().map(ToString::to_string),
        }
    }

    fn count_step(&self) -> Result<(), SpellError> {
        let taken = self.steps.fetch_add(1, Ordering::SeqCst) + 1;
        if taken > self.step_limit { Err(SpellError::StepLimitExceeded(self.step_limit)) } else { Ok(()) }
    }

    fn incoming_flow_wires(&self, node: &NodeId) -> usize {
        let Some(executor) = self.executors.get(node) else {
            return 0;
        };
        self.graph.wires_into(node).filter(|wire| executor.spec().resolve_input(&wire.to_socket).is_some_and(|socket| socket.ty.is_flow())).count()
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

    fn pull(self: Arc<Self>, node: NodeId, socket: String, memo: Memo, scope: Scope) -> BoxFuture<Result<Option<Value>, NodeError>> {
        Box::pin(async move {
            let wire = self.graph.wires.iter().find(|wire| wire.to_node == node && wire.to_socket == socket).cloned();
            let Some(wire) = wire else {
                return Ok(self.unwired_input(&node, &socket));
            };
            let value = if self.impure.contains(&wire.from_node) {
                match scope.get(&wire.from_node) {
                    Some(outputs) => outputs.get(&wire.from_socket).cloned(),
                    None => locked(&self.outputs).get(&wire.from_node).and_then(|outputs| outputs.get(&wire.from_socket)).cloned(),
                }
            } else {
                let cached = locked(&memo).get(&wire.from_node).cloned();
                let outputs = match cached {
                    Some(outputs) => outputs,
                    None => {
                        let outputs = self.clone().evaluate_pure(wire.from_node.clone(), memo.clone(), scope.clone()).await?;
                        locked(&memo).insert(wire.from_node.clone(), outputs.clone());
                        outputs
                    }
                };
                outputs.get(&wire.from_socket).cloned()
            };
            self.emit(TraceEvent::WireActivated { run_id: self.run_id, wire_id: wire.id.clone(), at: Utc::now(), value: value.clone() });
            Ok(value)
        })
    }

    fn evaluate_pure(self: Arc<Self>, node: NodeId, memo: Memo, scope: Scope) -> BoxFuture<Result<NodeOutputs, NodeError>> {
        Box::pin(async move {
            let (Some(instance), Some(executor)) = (self.instances.get(&node).cloned(), self.executors.get(&node).cloned()) else {
                return Err(NodeError::Other(format!("node {node:?} is not part of the spell")));
            };
            let started = Instant::now();
            self.emit(TraceEvent::NodeStarted { run_id: self.run_id, node_id: node.clone(), at: Utc::now() });
            self.executed.fetch_add(1, Ordering::SeqCst);
            let mut context = ExecContext { state: self.clone(), node: instance, trigger_socket: None, memo, scope };
            match executor.execute(&mut context).await {
                Ok(outcome) => {
                    locked(&self.outputs).insert(node.clone(), outcome.outputs.clone());
                    self.emit(TraceEvent::NodeFinished { run_id: self.run_id, node_id: node, at: Utc::now(), duration_ms: elapsed_ms(started) });
                    Ok(outcome.outputs)
                }
                Err(error) => {
                    self.emit(TraceEvent::NodeFailed { run_id: self.run_id, node_id: node, at: Utc::now(), error: error.to_string() });
                    Err(error)
                }
            }
        })
    }

    fn activate(self: Arc<Self>, node: NodeId, socket: String, via: Option<WireId>, scope: Scope) -> BoxFuture<Result<(), SpellError>> {
        Box::pin(async move {
            if self.cancellation.is_cancelled() {
                return Err(SpellError::Cancelled);
            }
            let kind = self.instances.get(&node).map(|instance| instance.kind.clone()).unwrap_or_default();
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
            let max_iterations = self.max_iterations(&node);
            let mut iterations = 0_u64;
            loop {
                let (fired, outputs) = self.execute_flow_node(&node, &socket, &scope).await?;
                let repeat = kind == WHILE_LOOP_KIND && fired.iter().any(|socket| socket == "body");
                if repeat {
                    iterations += 1;
                    if iterations > max_iterations {
                        return Err(self.fail_node(&node, NodeError::Other(format!("while loop exceeded max_iterations {max_iterations}"))));
                    }
                }
                self.clone().fire(node.clone(), fired, with_outputs(&scope, &node, outputs)).await?;
                if !repeat {
                    return Ok(());
                }
            }
        })
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
        self.emit(TraceEvent::NodeFailed { run_id: self.run_id, node_id: node.clone(), at: Utc::now(), error: error.to_string() });
        SpellError::Node { node: node.clone(), source: error }
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
        self.emit(TraceEvent::NodeFailed { run_id: self.run_id, node_id: (*node).clone(), at: Utc::now(), error: message.clone() });
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

    async fn execute_flow_node(self: &Arc<Self>, node: &NodeId, socket: &str, scope: &Scope) -> Result<(Vec<String>, NodeOutputs), SpellError> {
        self.count_step()?;
        let (Some(instance), Some(executor)) = (self.instances.get(node).cloned(), self.executors.get(node).cloned()) else {
            return Err(SpellError::Internal(format!("node {node:?} is not part of the spell")));
        };
        let started = Instant::now();
        self.emit(TraceEvent::NodeStarted { run_id: self.run_id, node_id: node.clone(), at: Utc::now() });
        self.executed.fetch_add(1, Ordering::SeqCst);
        let memo: Memo = Arc::default();
        let mut context = ExecContext { state: self.clone(), node: instance, trigger_socket: Some(socket.to_owned()), memo, scope: scope.clone() };
        let cancellation = self.cancellation.clone();
        let result = tokio::select! {
            result = executor.execute(&mut context) => result,
            () = cancellation.cancelled() => Err(NodeError::Cancelled),
        };
        match result {
            Ok(outcome) => {
                locked(&self.outputs).insert(node.clone(), outcome.outputs.clone());
                self.emit(TraceEvent::NodeFinished { run_id: self.run_id, node_id: node.clone(), at: Utc::now(), duration_ms: elapsed_ms(started) });
                Ok((outcome.fire, outcome.outputs))
            }
            Err(error) => {
                self.emit(TraceEvent::NodeFailed { run_id: self.run_id, node_id: node.clone(), at: Utc::now(), error: error.to_string() });
                if error == NodeError::Cancelled { Err(SpellError::Cancelled) } else { Err(SpellError::Node { node: node.clone(), source: error }) }
            }
        }
    }

    fn fire(self: Arc<Self>, node: NodeId, sockets: Vec<String>, scope: Scope) -> BoxFuture<Result<(), SpellError>> {
        Box::pin(async move {
            let wires: Vec<Wire> = self.graph.wires_out_of(&node).filter(|wire| sockets.contains(&wire.from_socket)).cloned().collect();
            for wire in &wires {
                self.emit(TraceEvent::WireActivated { run_id: self.run_id, wire_id: wire.id.clone(), at: Utc::now(), value: None });
            }
            if let [wire] = wires.as_slice() {
                return self.clone().activate(wire.to_node.clone(), wire.to_socket.clone(), Some(wire.id.clone()), scope).await;
            }
            let mut branches = JoinSet::new();
            for wire in wires {
                branches.spawn(self.clone().activate(wire.to_node, wire.to_socket, Some(wire.id), scope.clone()));
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
        })
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
        self.state.run_id
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
        self.state.emit(TraceEvent::VariableSet { run_id: self.state.run_id, name: name.to_owned(), value });
        Ok(())
    }

    fn llm(&self) -> Option<Arc<dyn LlmGateway>> {
        self.state.services.llm.clone()
    }

    fn memory(&self) -> Option<Arc<dyn MemoryService>> {
        self.state.services.memory.clone()
    }

    fn knowledge(&self) -> Option<Arc<dyn KnowledgeService>> {
        self.state.services.knowledge.clone()
    }

    fn events(&self) -> Arc<dyn EventEmitter> {
        self.state.events.clone()
    }

    fn cancellation(&self) -> CancellationToken {
        self.state.cancellation.clone()
    }

    fn agent_id(&self) -> Option<&str> {
        self.state.binding.agent_id.as_deref()
    }

    fn allow_private_http(&self) -> bool {
        self.state.binding.allow_private_http.unwrap_or(false)
    }
}
