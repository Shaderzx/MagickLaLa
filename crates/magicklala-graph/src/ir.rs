use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{SocketType, Value};

pub const SPELL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpellId(pub Uuid);

impl SpellId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for SpellId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl From<&str> for NodeId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireId(pub String);

impl From<&str> for WireId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpellGraph {
    pub schema_version: u32,
    pub id: SpellId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub variables: Vec<VariableDef>,
    #[serde(default)]
    pub nodes: Vec<NodeInstance>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    #[serde(default)]
    pub subgraphs: Vec<SubgraphRef>,
}

impl SpellGraph {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: SPELL_SCHEMA_VERSION,
            id: SpellId::new(),
            name: name.into(),
            description: String::new(),
            variables: Vec::new(),
            nodes: Vec::new(),
            wires: Vec::new(),
            subgraphs: Vec::new(),
        }
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeInstance> {
        self.nodes.iter().find(|node| &node.id == id)
    }

    pub fn wires_into<'a>(&'a self, node: &'a NodeId) -> impl Iterator<Item = &'a Wire> + 'a {
        self.wires.iter().filter(move |wire| &wire.to_node == node)
    }

    pub fn wires_out_of<'a>(&'a self, node: &'a NodeId) -> impl Iterator<Item = &'a Wire> + 'a {
        self.wires.iter().filter(move |wire| &wire.from_node == node)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInstance {
    pub id: NodeId,
    pub kind: String,
    pub position: [f32; 2],
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    pub id: WireId,
    pub from_node: NodeId,
    pub from_socket: String,
    pub to_node: NodeId,
    pub to_socket: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariableDef {
    pub name: String,
    pub ty: SocketType,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub exposed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubgraphRef {
    pub node_id: NodeId,
    pub spell_id: SpellId,
    #[serde(default)]
    pub release_version: Option<u32>,
}
