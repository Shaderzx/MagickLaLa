use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ConnectorError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum PluginTransport {
    StdioJsonRpc {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env_keys: Vec<String>,
    },
    Wasm {
        module_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "capability", rename_all = "snake_case")]
pub enum PluginCapability {
    Connector { kind_name: String },
    Node { kind: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalPluginSpec {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub transport: PluginTransport,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: serde_json::Value) -> Self {
        Self { jsonrpc: "2.0".to_owned(), id, method: method.into(), params }
    }

    pub fn to_line(&self) -> Result<String, ConnectorError> {
        serde_json::to_string(self).map(|line| format!("{line}\n")).map_err(|error| ConnectorError::Config(error.to_string()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: BTreeMap<String, ExternalPluginSpec>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, spec: ExternalPluginSpec) -> Result<(), ConnectorError> {
        let valid_name = !spec.name.is_empty() && spec.name.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'));
        if !valid_name {
            return Err(ConnectorError::Config(format!("plugin name {:?} must be non-empty ASCII letters, digits, '-', '_' or '.'", spec.name)));
        }
        if spec.capabilities.is_empty() {
            return Err(ConnectorError::Config(format!("plugin {} declares no capabilities", spec.name)));
        }
        match &spec.transport {
            PluginTransport::StdioJsonRpc { command, .. } if command.trim().is_empty() => {
                return Err(ConnectorError::Config(format!("plugin {} has an empty command", spec.name)));
            }
            PluginTransport::Wasm { module_path } if !module_path.ends_with(".wasm") => {
                return Err(ConnectorError::Config(format!("plugin {} module must be a .wasm file", spec.name)));
            }
            _ => {}
        }
        if let Some(clash) = spec.capabilities.iter().find(|capability| self.provider_of(capability).is_some_and(|owner| owner != spec.name)) {
            return Err(ConnectorError::Config(format!("capability {clash:?} is already provided by another plugin")));
        }
        self.plugins.insert(spec.name.clone(), spec);
        Ok(())
    }

    pub fn unregister(&mut self, name: &str) -> Option<ExternalPluginSpec> {
        self.plugins.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<&ExternalPluginSpec> {
        self.plugins.get(name)
    }

    pub fn list(&self) -> impl Iterator<Item = &ExternalPluginSpec> {
        self.plugins.values()
    }

    pub fn provider_of(&self, capability: &PluginCapability) -> Option<&str> {
        self.plugins.values().find(|spec| spec.capabilities.contains(capability)).map(|spec| spec.name.as_str())
    }
}
