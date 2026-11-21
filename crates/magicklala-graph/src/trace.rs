use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{NodeId, SpellId, Value, WireId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TraceEvent {
    RunStarted {
        run_id: RunId,
        spell_id: SpellId,
        at: DateTime<Utc>,
    },
    NodeStarted {
        run_id: RunId,
        node_id: NodeId,
        at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subgraph_path: Vec<NodeId>,
    },
    NodeFinished {
        run_id: RunId,
        node_id: NodeId,
        at: DateTime<Utc>,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subgraph_path: Vec<NodeId>,
    },
    NodeFailed {
        run_id: RunId,
        node_id: NodeId,
        at: DateTime<Utc>,
        error: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subgraph_path: Vec<NodeId>,
    },
    WireActivated {
        run_id: RunId,
        wire_id: WireId,
        at: DateTime<Utc>,
        value: Option<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subgraph_path: Vec<NodeId>,
    },
    StreamChunk {
        run_id: RunId,
        node_id: NodeId,
        delta: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subgraph_path: Vec<NodeId>,
    },
    VariableSet {
        run_id: RunId,
        name: String,
        value: Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subgraph_path: Vec<NodeId>,
    },
    RunFinished {
        run_id: RunId,
        summary: RunSummary,
    },
}

impl TraceEvent {
    pub fn run_id(&self) -> RunId {
        match self {
            TraceEvent::RunStarted { run_id, .. }
            | TraceEvent::NodeStarted { run_id, .. }
            | TraceEvent::NodeFinished { run_id, .. }
            | TraceEvent::NodeFailed { run_id, .. }
            | TraceEvent::WireActivated { run_id, .. }
            | TraceEvent::StreamChunk { run_id, .. }
            | TraceEvent::VariableSet { run_id, .. }
            | TraceEvent::RunFinished { run_id, .. } => *run_id,
        }
    }

    pub fn subgraph_path(&self) -> &[NodeId] {
        match self {
            TraceEvent::NodeStarted { subgraph_path, .. }
            | TraceEvent::NodeFinished { subgraph_path, .. }
            | TraceEvent::NodeFailed { subgraph_path, .. }
            | TraceEvent::WireActivated { subgraph_path, .. }
            | TraceEvent::StreamChunk { subgraph_path, .. }
            | TraceEvent::VariableSet { subgraph_path, .. } => subgraph_path,
            TraceEvent::RunStarted { .. } | TraceEvent::RunFinished { .. } => &[],
        }
    }

    pub fn subgraph_path_mut(&mut self) -> Option<&mut Vec<NodeId>> {
        match self {
            TraceEvent::NodeStarted { subgraph_path, .. }
            | TraceEvent::NodeFinished { subgraph_path, .. }
            | TraceEvent::NodeFailed { subgraph_path, .. }
            | TraceEvent::WireActivated { subgraph_path, .. }
            | TraceEvent::StreamChunk { subgraph_path, .. }
            | TraceEvent::VariableSet { subgraph_path, .. } => Some(subgraph_path),
            TraceEvent::RunStarted { .. } | TraceEvent::RunFinished { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: RunId,
    pub spell_id: SpellId,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub status: RunStatus,
    pub nodes_executed: u64,
    pub outputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub activations: Vec<EmittedOutput>,
    #[serde(default)]
    pub activations_truncated: bool,
    #[serde(default)]
    pub replies: Vec<String>,
    pub error: Option<String>,
}

impl RunSummary {
    pub fn slim(&self) -> Self {
        Self {
            run_id: self.run_id,
            spell_id: self.spell_id,
            started_at: self.started_at,
            finished_at: self.finished_at,
            status: self.status,
            nodes_executed: self.nodes_executed,
            outputs: BTreeMap::new(),
            activations: Vec::new(),
            activations_truncated: self.activations_truncated,
            replies: Vec::new(),
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmittedOutput {
    pub node_id: NodeId,
    pub outputs: BTreeMap<String, Value>,
}
