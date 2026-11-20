use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use magicklala_connectors::{ConnectorKind, InboundEvent, OutboundAction};
use magicklala_embedder::Embedder;
use magicklala_graph::{
    EngineServices, EventEmitter, LlmGateway, NodeCatalog, NodeId, NodeInstance, RunBinding, RunId, RunStatus, RunSummary, RunTrigger, SPELL_SCHEMA_VERSION,
    SpellEngine, SpellError, SpellGraph, SubgraphResolver, TraceEvent, ValidationError, Value,
};
use magicklala_store::{AgentId, ProjectId, ReleaseId, Repository, SpellRecordId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::bridges::{StoreKnowledgeBridge, StoreMemoryBridge};
use crate::metered::{LlmCallScope, MeteredLlmGateway};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseOrigin {
    pub spell_id: SpellRecordId,
    pub release_id: Option<ReleaseId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRelease {
    pub agent_id: Uuid,
    pub version: u32,
    pub graph: SpellGraph,
    pub deployed_at: DateTime<Utc>,
    #[serde(default)]
    pub origin: Option<ReleaseOrigin>,
}

impl AgentRelease {
    pub fn spell_record_id(&self) -> SpellRecordId {
        self.origin.map_or(SpellRecordId(self.graph.id.0), |origin| origin.spell_id)
    }

    pub fn release_record_id(&self) -> Option<ReleaseId> {
        self.origin.and_then(|origin| origin.release_id)
    }
}

#[derive(Debug, Clone)]
pub struct RunScope {
    pub run_id: RunId,
    pub cancellation: CancellationToken,
}

impl RunScope {
    pub fn new() -> Self {
        Self { run_id: RunId::new(), cancellation: CancellationToken::new() }
    }
}

impl Default for RunScope {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
pub trait ReleaseRunner: Send + Sync {
    fn validate(&self, graph: &SpellGraph) -> Result<(), Vec<ValidationError>>;
    async fn run(
        &self,
        release: &AgentRelease,
        event: &InboundEvent,
        events: Arc<dyn EventEmitter>,
        scope: RunScope,
    ) -> Result<Vec<OutboundAction>, SpellError>;
}

pub fn inbound_payload(event: &InboundEvent) -> Value {
    Value::Event(BTreeMap::from([
        ("channel".to_owned(), Value::String(event.channel.clone())),
        ("sender".to_owned(), Value::String(event.sender.clone())),
        ("content".to_owned(), Value::String(event.content.clone())),
        ("payload".to_owned(), Value::Json(event.payload.clone())),
    ]))
}

pub const CORRELATION_ID_KEY: &str = "correlation_id";

pub fn correlation_id(payload: &serde_json::Value) -> Option<&str> {
    payload.get(CORRELATION_ID_KEY).and_then(serde_json::Value::as_str)
}

fn reply_action(event: &InboundEvent, content: String) -> OutboundAction {
    let mut payload = serde_json::json!({ "in_reply_to": event.sender });
    if let Some(id) = correlation_id(&event.payload) {
        payload[CORRELATION_ID_KEY] = serde_json::Value::String(id.to_owned());
    }
    OutboundAction { connector_id: event.connector_id, channel: event.channel.clone(), content, payload }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateReplyRunner;

impl TemplateReplyRunner {
    pub const TEMPLATE_KEY: &'static str = "template";

    fn render(template: &str, event: &InboundEvent) -> String {
        template.replace("{content}", &event.content).replace("{channel}", &event.channel).replace("{sender}", &event.sender)
    }

    fn reply_nodes(graph: &SpellGraph) -> impl Iterator<Item = (&NodeId, &str)> {
        graph.nodes.iter().filter_map(|node| node.config.get(Self::TEMPLATE_KEY).and_then(serde_json::Value::as_str).map(|template| (&node.id, template)))
    }
}

#[async_trait]
impl ReleaseRunner for TemplateReplyRunner {
    fn validate(&self, graph: &SpellGraph) -> Result<(), Vec<ValidationError>> {
        let mut errors = structural_errors(graph);
        if Self::reply_nodes(graph).next().is_none() {
            errors.push(ValidationError::NoTrigger);
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    async fn run(
        &self,
        release: &AgentRelease,
        event: &InboundEvent,
        events: Arc<dyn EventEmitter>,
        scope: RunScope,
    ) -> Result<Vec<OutboundAction>, SpellError> {
        let run_id = scope.run_id;
        let started_at = Utc::now();
        events.emit(TraceEvent::RunStarted { run_id, spell_id: release.graph.id, at: started_at });
        let mut actions = Vec::new();
        let mut outputs = BTreeMap::new();
        for (node_id, template) in Self::reply_nodes(&release.graph) {
            let node_started = Utc::now();
            events.emit(TraceEvent::NodeStarted { run_id, node_id: node_id.clone(), at: node_started });
            let reply = Self::render(template, event);
            outputs.insert(node_id.0.clone(), Value::String(reply.clone()));
            actions.push(reply_action(event, reply));
            let duration_ms = u64::try_from((Utc::now() - node_started).num_milliseconds().max(0)).unwrap_or_default();
            events.emit(TraceEvent::NodeFinished { run_id, node_id: node_id.clone(), at: Utc::now(), duration_ms });
        }
        let summary = RunSummary {
            run_id,
            spell_id: release.graph.id,
            started_at,
            finished_at: Utc::now(),
            status: RunStatus::Succeeded,
            nodes_executed: u64::try_from(actions.len()).unwrap_or(u64::MAX),
            outputs,
            activations: Vec::new(),
            activations_truncated: false,
            replies: Vec::new(),
            error: None,
        };
        events.emit(TraceEvent::RunFinished { run_id, summary });
        Ok(actions)
    }
}

pub fn structural_errors(graph: &SpellGraph) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if graph.schema_version != SPELL_SCHEMA_VERSION {
        errors.push(ValidationError::SchemaVersion { found: graph.schema_version, expected: SPELL_SCHEMA_VERSION });
    }
    let mut nodes = std::collections::BTreeSet::new();
    for node in &graph.nodes {
        if !nodes.insert(&node.id) {
            errors.push(ValidationError::DuplicateNode(node.id.clone()));
        }
    }
    let mut wires = std::collections::BTreeSet::new();
    for wire in &graph.wires {
        if !wires.insert(&wire.id) {
            errors.push(ValidationError::DuplicateWire(wire.id.clone()));
        }
        for endpoint in [&wire.from_node, &wire.to_node] {
            if graph.node(endpoint).is_none() {
                errors.push(ValidationError::MissingNode { wire: wire.id.clone(), node: endpoint.clone() });
            }
        }
    }
    errors
}

const REPLY_OUTPUT_SOCKETS: [&str; 2] = ["sent", "reply"];
const MESSAGE_TRIGGER_KINDS: [&str; 2] = ["event/on_message", "core/on_message"];
const START_TRIGGER_KIND: &str = "event/on_start";

pub fn trigger_kinds(kind: ConnectorKind) -> &'static [&'static str] {
    match kind {
        ConnectorKind::Webhook => &["event/on_webhook"],
        ConnectorKind::Tick => &["event/on_tick"],
        ConnectorKind::Studio | ConnectorKind::Rest | ConnectorKind::Discord | ConnectorKind::Slack => &MESSAGE_TRIGGER_KINDS,
    }
}

fn webhook_envelope(event: &InboundEvent) -> serde_json::Value {
    let payload = &event.payload;
    let is_envelope = payload.as_object().is_some_and(|fields| fields.contains_key("method") || fields.contains_key("path"));
    let part = |key: &str, fallback: serde_json::Value| if is_envelope { payload.get(key).cloned().unwrap_or(fallback) } else { fallback };
    let body = if is_envelope { payload.get("body").cloned().unwrap_or(serde_json::Value::Null) } else { payload.clone() };
    serde_json::json!({
        "method": part("method", serde_json::Value::String("POST".to_owned())),
        "path": part("path", serde_json::Value::String(event.channel.clone())),
        "headers": part("headers", serde_json::json!({})),
        "query": part("query", serde_json::json!({})),
        "body": body,
        "channel": event.channel,
        "sender": event.sender,
        "content": event.content,
        CORRELATION_ID_KEY: correlation_id(payload),
    })
}

fn tick_envelope(event: &InboundEvent) -> serde_json::Value {
    let sequence = event.payload.get("tick").or_else(|| event.payload.get("sequence")).cloned().unwrap_or(serde_json::Value::from(0));
    serde_json::json!({
        "tick": sequence,
        "at": event.received_at.to_rfc3339(),
        "channel": event.channel,
        "payload": event.payload,
        CORRELATION_ID_KEY: correlation_id(&event.payload),
    })
}

pub fn trigger_payload(event: &InboundEvent) -> Value {
    match event.kind {
        ConnectorKind::Webhook => Value::Json(webhook_envelope(event)),
        ConnectorKind::Tick => Value::Json(tick_envelope(event)),
        ConnectorKind::Studio | ConnectorKind::Rest | ConnectorKind::Discord | ConnectorKind::Slack => inbound_payload(event),
    }
}

pub type ProjectLlmFactory = Arc<dyn Fn(ProjectId) -> Arc<dyn LlmGateway> + Send + Sync>;

#[async_trait]
pub trait SubgraphLoader: Send + Sync {
    async fn load(&self, project: ProjectId, graph: &SpellGraph) -> Result<Arc<dyn SubgraphResolver>, String>;
}

#[derive(Clone)]
pub struct StoreScope {
    pub repository: Arc<dyn Repository>,
    pub llm: Option<Arc<dyn LlmGateway>>,
    pub embedder: Option<Arc<dyn Embedder>>,
}

struct ScopedEngine {
    catalog: Arc<dyn NodeCatalog>,
    base: EngineServices,
    store: StoreScope,
    subgraphs: Option<Arc<dyn SubgraphResolver>>,
    subgraph_loader: Option<Arc<dyn SubgraphLoader>>,
    project_llm: Option<ProjectLlmFactory>,
}

impl ScopedEngine {
    async fn agent_project(&self, agent: AgentId) -> Option<ProjectId> {
        self.store.repository.get_agent(agent).await.ok().map(|agent| agent.project_id)
    }

    async fn services(&self, agent_id: Uuid, run_id: RunId, project: Option<ProjectId>) -> EngineServices {
        let agent = AgentId(agent_id);
        let repository = &self.store.repository;
        let project_llm = self.project_llm.as_ref().zip(project).map(|(factory, project)| factory(project));
        let upstream = project_llm.or_else(|| self.store.llm.clone());
        let llm = upstream.map_or_else(
            || self.base.llm.clone(),
            |llm| {
                let scope = LlmCallScope { agent_id: Some(agent), run_id: Some(run_id.0) };
                let metered: Arc<dyn LlmGateway> = Arc::new(MeteredLlmGateway::new(llm, Arc::clone(repository)).scoped(scope));
                Some(metered)
            },
        );
        let (memory, knowledge) = match &self.store.embedder {
            Some(embedder) => (
                Some(
                    Arc::new(StoreMemoryBridge::new(Arc::clone(repository), Arc::clone(embedder)).pinned_to(agent)) as Arc<dyn magicklala_graph::MemoryService>
                ),
                Some(Arc::new(StoreKnowledgeBridge::new(Arc::clone(repository), Arc::clone(embedder)).pinned_to(agent))
                    as Arc<dyn magicklala_graph::KnowledgeService>),
            ),
            None => (self.base.memory.clone(), self.base.knowledge.clone()),
        };
        EngineServices { llm, memory, knowledge }
    }

    async fn resolver(&self, project: Option<ProjectId>, graph: &SpellGraph) -> Result<Option<Arc<dyn SubgraphResolver>>, SpellError> {
        if let Some((loader, project)) = self.subgraph_loader.as_ref().zip(project) {
            return loader.load(project, graph).await.map(Some).map_err(|error| SpellError::Internal(format!("loading subgraphs failed: {error}")));
        }
        Ok(self.subgraphs.clone())
    }

    async fn engine(&self, agent_id: Uuid, run_id: RunId, step_limit: u64, graph: &SpellGraph) -> Result<SpellEngine, SpellError> {
        let project = self.agent_project(AgentId(agent_id)).await;
        let engine = SpellEngine::new(Arc::clone(&self.catalog), self.services(agent_id, run_id, project).await)
            .with_step_limit(step_limit)
            .with_agent_id(agent_id.to_string())
            .with_run_id(run_id);
        Ok(match self.resolver(project, graph).await? {
            Some(resolver) => engine.with_subgraph_resolver(resolver),
            None => engine,
        })
    }
}

struct FinishOrder {
    inner: Arc<dyn EventEmitter>,
    finished: Mutex<Vec<NodeId>>,
}

impl FinishOrder {
    fn nodes(&self) -> Vec<NodeId> {
        self.finished.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl EventEmitter for FinishOrder {
    fn emit(&self, event: TraceEvent) {
        if let TraceEvent::NodeFinished { node_id, .. } = &event {
            let mut finished = self.finished.lock().unwrap_or_else(PoisonError::into_inner);
            if !finished.contains(node_id) {
                finished.push(node_id.clone());
            }
        }
        self.inner.emit(event);
    }
}

fn activation_replies(summary: &RunSummary) -> Vec<String> {
    summary
        .activations
        .iter()
        .flat_map(|activation| REPLY_OUTPUT_SOCKETS.iter().filter_map(|socket| activation.outputs.get(*socket)))
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

fn finished_replies(summary: &RunSummary, finish_order: &[NodeId]) -> Vec<String> {
    finish_order
        .iter()
        .flat_map(|node| REPLY_OUTPUT_SOCKETS.iter().filter_map(move |socket| summary.outputs.get(&format!("{}.{socket}", node.0))))
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

pub fn ordered_replies(summary: &RunSummary, finish_order: &[NodeId]) -> Vec<String> {
    if !summary.replies.is_empty() {
        summary.replies.clone()
    } else if summary.activations.is_empty() {
        finished_replies(summary, finish_order)
    } else {
        activation_replies(summary)
    }
}

fn missing_trigger(kind: ConnectorKind) -> SpellError {
    let kind_name = serde_json::to_value(kind).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default();
    let accepted: Vec<&str> = trigger_kinds(kind).iter().copied().chain([START_TRIGGER_KIND]).collect();
    SpellError::Internal(format!("spell has no trigger for {kind_name} events: add one of {}", accepted.join(", ")))
}

pub struct EngineReleaseRunner {
    engine: Arc<SpellEngine>,
    scoped: Option<ScopedEngine>,
    allow_private_http: Option<bool>,
}

impl EngineReleaseRunner {
    pub fn new(engine: Arc<SpellEngine>) -> Self {
        Self { engine, scoped: None, allow_private_http: None }
    }

    pub fn scoped(catalog: Arc<dyn NodeCatalog>, base: EngineServices, store: StoreScope) -> Self {
        let engine = Arc::new(SpellEngine::new(Arc::clone(&catalog), base.clone()));
        let scoped = ScopedEngine { catalog, base, store, subgraphs: None, subgraph_loader: None, project_llm: None };
        Self { engine, scoped: Some(scoped), allow_private_http: None }
    }

    pub fn with_project_llm(mut self, factory: ProjectLlmFactory) -> Self {
        if let Some(scoped) = &mut self.scoped {
            scoped.project_llm = Some(factory);
        }
        self
    }

    pub fn with_private_http(mut self, allowed: bool) -> Self {
        self.allow_private_http = Some(allowed);
        self
    }

    pub fn with_subgraph_resolver(mut self, resolver: Arc<dyn SubgraphResolver>) -> Self {
        if let Some(scoped) = &mut self.scoped {
            scoped.subgraphs = Some(resolver);
        }
        self
    }

    pub fn with_subgraph_loader(mut self, loader: Arc<dyn SubgraphLoader>) -> Self {
        if let Some(scoped) = &mut self.scoped {
            scoped.subgraph_loader = Some(loader);
        }
        self
    }

    pub fn is_scoped(&self) -> bool {
        self.scoped.is_some()
    }

    fn trigger_node<'graph>(&self, graph: &'graph SpellGraph, kind: ConnectorKind) -> Option<&'graph NodeInstance> {
        let validator = self.engine.validator();
        let triggers: Vec<&NodeInstance> = graph.nodes.iter().filter(|node| validator.spec(&node.kind).is_some_and(|spec| spec.is_trigger)).collect();
        let wanted = trigger_kinds(kind);
        triggers.iter().find(|node| wanted.contains(&node.kind.as_str())).or_else(|| triggers.iter().find(|node| node.kind == START_TRIGGER_KIND)).copied()
    }

    fn trigger(&self, graph: &SpellGraph, event: &InboundEvent) -> Result<RunTrigger, SpellError> {
        let node = self.trigger_node(graph, event.kind).ok_or_else(|| missing_trigger(event.kind))?;
        let socket =
            self.engine.validator().spec(&node.kind).and_then(|spec| spec.outputs.first()).map_or_else(|| "out".to_owned(), |socket| socket.name.clone());
        Ok(RunTrigger { node: node.id.clone(), socket, payload: trigger_payload(event) })
    }
}

#[async_trait]
impl ReleaseRunner for EngineReleaseRunner {
    fn validate(&self, graph: &SpellGraph) -> Result<(), Vec<ValidationError>> {
        self.engine.validator().validate(graph)
    }

    async fn run(
        &self,
        release: &AgentRelease,
        event: &InboundEvent,
        events: Arc<dyn EventEmitter>,
        scope: RunScope,
    ) -> Result<Vec<OutboundAction>, SpellError> {
        let trigger = self.trigger(&release.graph, event)?;
        let order = Arc::new(FinishOrder { inner: events, finished: Mutex::new(Vec::new()) });
        let emitter: Arc<dyn EventEmitter> = Arc::clone(&order) as Arc<dyn EventEmitter>;
        let binding = RunBinding { agent_id: Some(release.agent_id.to_string()), allow_private_http: self.allow_private_http, run_id: Some(scope.run_id) };
        let summary = match &self.scoped {
            Some(scoped) => {
                let engine = scoped.engine(release.agent_id, scope.run_id, self.engine.step_limit(), &release.graph).await?;
                engine.run_bound(&release.graph, trigger, emitter, scope.cancellation, binding).await?
            }
            None => self.engine.run_bound(&release.graph, trigger, emitter, scope.cancellation, binding).await?,
        };
        if summary.status != RunStatus::Succeeded {
            return Err(SpellError::Internal(summary.error.unwrap_or_else(|| format!("run ended {:?}", summary.status))));
        }
        Ok(ordered_replies(&summary, &order.nodes()).into_iter().map(|content| reply_action(event, content)).collect())
    }
}
