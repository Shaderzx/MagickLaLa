use serde::{Deserialize, Serialize};

use crate::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketType {
    Flow,
    String,
    Int,
    Float,
    Bool,
    Json,
    Array,
    Embedding,
    Event,
    Any,
}

impl SocketType {
    pub fn is_flow(self) -> bool {
        self == SocketType::Flow
    }

    pub fn accepts(self, incoming: SocketType) -> bool {
        match (self, incoming) {
            (SocketType::Flow, other) | (other, SocketType::Flow) => self == other,
            (SocketType::Any, _) | (_, SocketType::Any) => true,
            (SocketType::Float, SocketType::Int) => true,
            (SocketType::Json, _) => true,
            (expected, actual) => expected == actual,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocketSpec {
    pub name: String,
    pub label: String,
    pub ty: SocketType,
    pub direction: SocketDirection,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub required: bool,
}

impl SocketSpec {
    pub fn input(name: &str, ty: SocketType) -> Self {
        Self::new(name, ty, SocketDirection::Input)
    }

    pub fn output(name: &str, ty: SocketType) -> Self {
        Self::new(name, ty, SocketDirection::Output)
    }

    fn new(name: &str, ty: SocketType, direction: SocketDirection) -> Self {
        Self { name: name.to_owned(), label: name.to_owned(), ty, direction, default: None, required: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    TextArea,
    Number { min: Option<f64>, max: Option<f64> },
    Boolean,
    Select { options: Vec<String> },
    Json,
    Code { language: String },
    Credential { provider: String },
    Model { provider: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeConfigField {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub help: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSpec {
    pub kind: String,
    pub title: String,
    pub category: String,
    #[serde(default)]
    pub description: String,
    pub inputs: Vec<SocketSpec>,
    pub outputs: Vec<SocketSpec>,
    #[serde(default)]
    pub config: Vec<NodeConfigField>,
    #[serde(default)]
    pub is_trigger: bool,
}

pub const DYNAMIC_SOCKET: &str = "*";

impl NodeSpec {
    pub fn binds_setting(&self, key: &str) -> bool {
        !crate::engine::UNBOUND_SETTINGS.contains(&key) && self.config.iter().any(|field| field.key == key)
    }
}

impl NodeSpec {
    pub fn resolve_input(&self, name: &str) -> Option<&SocketSpec> {
        resolve_socket(&self.inputs, name)
    }

    pub fn resolve_output(&self, name: &str) -> Option<&SocketSpec> {
        resolve_socket(&self.outputs, name)
    }

    pub fn input(&self, name: &str) -> Option<&SocketSpec> {
        self.inputs.iter().find(|socket| socket.name == name)
    }

    pub fn output(&self, name: &str) -> Option<&SocketSpec> {
        self.outputs.iter().find(|socket| socket.name == name)
    }
}

fn resolve_socket<'a>(sockets: &'a [SocketSpec], name: &str) -> Option<&'a SocketSpec> {
    sockets.iter().find(|socket| socket.name == name).or_else(|| sockets.iter().find(|socket| socket.name == DYNAMIC_SOCKET))
}
