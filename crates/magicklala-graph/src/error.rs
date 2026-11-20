use serde::Serialize;
use thiserror::Error;

use crate::{NodeId, SocketType, SpellId, WireId};

#[derive(Debug, Clone, PartialEq, Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum ValidationError {
    #[error("unsupported schema version {found}, expected {expected}")]
    SchemaVersion { found: u32, expected: u32 },
    #[error("duplicate node id {0:?}")]
    DuplicateNode(NodeId),
    #[error("duplicate wire id {0:?}")]
    DuplicateWire(WireId),
    #[error("node {node:?} has unknown kind {kind}")]
    UnknownNodeKind { node: NodeId, kind: String },
    #[error("wire {wire:?} references missing node {node:?}")]
    MissingNode { wire: WireId, node: NodeId },
    #[error("wire {wire:?} references missing socket {socket} on node {node:?}")]
    MissingSocket { wire: WireId, node: NodeId, socket: String },
    #[error("wire {wire:?} connects {from:?} to {to:?}")]
    TypeMismatch { wire: WireId, from: SocketType, to: SocketType },
    #[error("value input {socket} on node {node:?} has more than one wire")]
    MultipleValueInputs { node: NodeId, socket: String },
    #[error("required input {socket} on node {node:?} is not connected and has no default")]
    MissingRequiredInput { node: NodeId, socket: String },
    #[error("value wires form a cycle through {0:?}")]
    ValueCycle(Vec<NodeId>),
    #[error("spell has no trigger node")]
    NoTrigger,
    #[error("variable {0} is declared twice")]
    DuplicateVariable(String),
    #[error("wait_for_all {node:?} joins mutually exclusive outputs of {conditional:?} through wires {wires:?}, so it can never complete")]
    ExclusiveJoin {
        node: NodeId,
        #[serde(rename = "source")]
        conditional: NodeId,
        wires: Vec<WireId>,
    },
    #[error("subgraph node {node:?} calls its own spell {spell:?}")]
    SubgraphCycle { node: NodeId, spell: SpellId },
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum NodeError {
    #[error("missing input {0}")]
    MissingInput(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("expected {expected:?}, found {found:?}")]
    TypeMismatch { expected: SocketType, found: SocketType },
    #[error("llm error: {0}")]
    Llm(String),
    #[error("service error: {0}")]
    Service(String),
    #[error("script error: {0}")]
    Script(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum SpellError {
    #[error("spell failed validation: {0:?}")]
    Validation(Vec<ValidationError>),
    #[error("node {node:?} failed: {source}")]
    Node { node: NodeId, source: NodeError },
    #[error("unknown node kind {0}")]
    UnknownNodeKind(String),
    #[error("step limit {0} exceeded")]
    StepLimitExceeded(u64),
    #[error("run cancelled")]
    Cancelled,
    #[error("run timed out")]
    Timeout,
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
    #[error("{0}")]
    Internal(String),
}
