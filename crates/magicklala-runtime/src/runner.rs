use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use magicklala_connectors::{InboundEvent, OutboundAction};
use magicklala_graph::{
    EventEmitter, NodeId, RunId, RunStatus, RunSummary, RunTrigger, SPELL_SCHEMA_VERSION, SpellEngine, SpellError, SpellGraph, TraceEvent, ValidationError,
    Value,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRelease {
    pub agent_id: Uuid,
    pub version: u32,
    pub graph: SpellGraph,
    pub deployed_at: DateTime<Utc>,
}

#[async_trait]
pub trait ReleaseRunner: Send + Sync {
    fn validate(&self, graph: &SpellGraph) -> Result<(), Vec<ValidationError>>;
    async fn run(&self, release: &AgentRelease, event: &InboundEvent, events: Arc<dyn EventEmitter>) -> Result<Vec<OutboundAction>, SpellError>;
}

pub fn inbound_payload(event: &InboundEvent) -> Value {
    Value::Event(BTreeMap::from([
        ("channel".to_owned(), Value::String(event.channel.clone())),
        ("sender".to_owned(), Value::String(event.sender.clone())),
        ("content".to_owned(), Value::String(event.content.clone())),
        ("payload".to_owned(), Value::Json(event.payload.clone())),
    ]))
}

fn reply_action(event: &InboundEvent, content: String) -> OutboundAction {
    OutboundAction { connector_id: event.connector_id, channel: event.channel.clone(), content, payload: serde_json::json!({ "in_reply_to": event.sender }) }
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

    async fn run(&self, release: &AgentRelease, event: &InboundEvent, events: Arc<dyn EventEmitter>) -> Result<Vec<OutboundAction>, SpellError> {
        let run_id = RunId::new();
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

const SENT_OUTPUT_SUFFIX: &str = ".sent";

pub struct EngineReleaseRunner {
    engine: Arc<SpellEngine>,
    reply_outputs: Vec<String>,
}

impl EngineReleaseRunner {
    pub fn new(engine: Arc<SpellEngine>) -> Self {
        Self { engine, reply_outputs: vec!["reply".to_owned(), "content".to_owned()] }
    }

    fn trigger(&self, graph: &SpellGraph, event: &InboundEvent) -> Result<RunTrigger, SpellError> {
        let validator = self.engine.validator();
        let node = graph
            .nodes
            .iter()
            .find(|node| validator.spec(&node.kind).is_some_and(|spec| spec.is_trigger))
            .ok_or(SpellError::Validation(vec![ValidationError::NoTrigger]))?;
        let socket = validator.spec(&node.kind).and_then(|spec| spec.outputs.first()).map_or_else(|| "out".to_owned(), |socket| socket.name.clone());
        Ok(RunTrigger { node: node.id.clone(), socket, payload: inbound_payload(event) })
    }
}

#[async_trait]
impl ReleaseRunner for EngineReleaseRunner {
    fn validate(&self, graph: &SpellGraph) -> Result<(), Vec<ValidationError>> {
        self.engine.validator().validate(graph)
    }

    async fn run(&self, release: &AgentRelease, event: &InboundEvent, events: Arc<dyn EventEmitter>) -> Result<Vec<OutboundAction>, SpellError> {
        let trigger = self.trigger(&release.graph, event)?;
        let summary = self.engine.run(&release.graph, trigger, events).await?;
        if summary.status != RunStatus::Succeeded {
            return Err(SpellError::Internal(summary.error.unwrap_or_else(|| format!("run ended {:?}", summary.status))));
        }
        Ok(self
            .reply_outputs
            .iter()
            .filter_map(|key| summary.outputs.get(key))
            .chain(summary.outputs.iter().filter(|(name, _)| name.ends_with(SENT_OUTPUT_SUFFIX)).map(|(_, value)| value))
            .filter_map(|value| value.as_str().map(str::to_owned))
            .map(|content| reply_action(event, content))
            .collect())
    }
}
