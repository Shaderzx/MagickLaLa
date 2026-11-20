use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::SocketType;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Json(serde_json::Value),
    Array(Vec<Value>),
    Embedding(Vec<f32>),
    Event(BTreeMap<String, Value>),
}

impl Value {
    pub fn socket_type(&self) -> SocketType {
        match self {
            Value::Null => SocketType::Any,
            Value::Bool(_) => SocketType::Bool,
            Value::Int(_) => SocketType::Int,
            Value::Float(_) => SocketType::Float,
            Value::String(_) => SocketType::String,
            Value::Json(_) => SocketType::Json,
            Value::Array(_) => SocketType::Array,
            Value::Embedding(_) => SocketType::Embedding,
            Value::Event(_) => SocketType::Event,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}
