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

impl From<serde_json::Value> for Value {
    fn from(json: serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(flag) => Value::Bool(flag),
            serde_json::Value::Number(number) => number.as_i64().map_or_else(|| Value::Float(number.as_f64().unwrap_or_default()), Value::Int),
            serde_json::Value::String(text) => Value::String(text),
            other => Value::Json(other),
        }
    }
}

impl Value {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(flag) => serde_json::Value::Bool(*flag),
            Value::Int(number) => serde_json::Value::from(*number),
            Value::Float(number) => serde_json::Number::from_f64(*number).map_or(serde_json::Value::Null, serde_json::Value::Number),
            Value::String(text) => serde_json::Value::String(text.clone()),
            Value::Json(json) => json.clone(),
            Value::Array(items) => serde_json::Value::Array(items.iter().map(Value::to_json).collect()),
            Value::Embedding(vector) => serde_json::Value::Array(vector.iter().map(|item| serde_json::Value::from(f64::from(*item))).collect()),
            Value::Event(fields) => serde_json::Value::Object(fields.iter().map(|(key, value)| (key.clone(), value.to_json())).collect()),
        }
    }

    pub fn to_text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::String(text) | Value::Json(serde_json::Value::String(text)) => text.clone(),
            Value::Bool(flag) => flag.to_string(),
            Value::Int(number) => number.to_string(),
            Value::Float(number) => number.to_string(),
            other => other.to_json().to_string(),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            Value::Int(number) => *number != 0,
            Value::Float(number) => *number != 0.0,
            Value::String(text) => !text.is_empty() && text != "false",
            Value::Json(json) => !matches!(json, serde_json::Value::Null | serde_json::Value::Bool(false)),
            Value::Array(items) => !items.is_empty(),
            Value::Embedding(vector) => !vector.is_empty(),
            Value::Event(fields) => !fields.is_empty(),
        }
    }
}
