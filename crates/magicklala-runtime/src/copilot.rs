use std::sync::Arc;

use magicklala_graph::{LlmGateway, LlmMessage, LlmRequest, LlmRole, NodeId, NodeInstance, SpellGraph, SpellValidator, VariableDef, Wire, WireId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotRequest {
    pub spell: SpellGraph,
    pub prompt: String,
    #[serde(default)]
    pub selection: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotSuggestion {
    pub summary: String,
    pub ops: Vec<GraphPatchOp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GraphPatchOp {
    AddNode { node: NodeInstance },
    RemoveNode { node_id: NodeId },
    MoveNode { node_id: NodeId, position: [f32; 2] },
    SetConfig { node_id: NodeId, config: serde_json::Value },
    AddWire { wire: Wire },
    RemoveWire { wire_id: WireId },
    AddVariable { variable: VariableDef },
    RemoveVariable { name: String },
    Connect { wire: Wire },
    Disconnect { wire_id: WireId },
    UpdateConfig { node_id: NodeId, config: serde_json::Value },
}

fn merge_config(target: &mut serde_json::Value, patch: &serde_json::Value) {
    match (target.as_object_mut(), patch.as_object()) {
        (Some(existing), Some(changes)) => changes.iter().for_each(|(key, value)| {
            if value.is_null() {
                existing.remove(key);
            } else {
                existing.insert(key.clone(), value.clone());
            }
        }),
        _ => *target = patch.clone(),
    }
}

impl GraphPatchOp {
    pub fn apply(&self, graph: &mut SpellGraph) {
        let _applied = self.try_apply(graph).is_ok() || self.apply_lenient(graph);
    }

    fn apply_lenient(&self, graph: &mut SpellGraph) -> bool {
        if let GraphPatchOp::AddNode { node } = self {
            graph.nodes.push(node.clone());
        }
        if let GraphPatchOp::AddWire { wire } | GraphPatchOp::Connect { wire } = self {
            graph.wires.push(wire.clone());
        }
        if let GraphPatchOp::AddVariable { variable } = self {
            graph.variables.push(variable.clone());
        }
        true
    }

    pub fn try_apply(&self, graph: &mut SpellGraph) -> Result<(), String> {
        let node_exists = |graph: &SpellGraph, id: &NodeId| graph.node(id).is_some();
        match self {
            GraphPatchOp::AddNode { node } => {
                if node_exists(graph, &node.id) {
                    return Err(format!("node {} already exists", node.id.0));
                }
                graph.nodes.push(node.clone());
            }
            GraphPatchOp::RemoveNode { node_id } => {
                if !node_exists(graph, node_id) {
                    return Err(format!("node {} does not exist", node_id.0));
                }
                graph.nodes.retain(|node| &node.id != node_id);
                graph.wires.retain(|wire| &wire.from_node != node_id && &wire.to_node != node_id);
            }
            GraphPatchOp::MoveNode { node_id, position } => {
                let node = graph.nodes.iter_mut().find(|node| &node.id == node_id).ok_or_else(|| format!("node {} does not exist", node_id.0))?;
                node.position = *position;
            }
            GraphPatchOp::SetConfig { node_id, config } => {
                let node = graph.nodes.iter_mut().find(|node| &node.id == node_id).ok_or_else(|| format!("node {} does not exist", node_id.0))?;
                node.config = config.clone();
            }
            GraphPatchOp::UpdateConfig { node_id, config } => {
                let node = graph.nodes.iter_mut().find(|node| &node.id == node_id).ok_or_else(|| format!("node {} does not exist", node_id.0))?;
                merge_config(&mut node.config, config);
            }
            GraphPatchOp::AddWire { wire } | GraphPatchOp::Connect { wire } => {
                if graph.wires.iter().any(|existing| existing.id == wire.id) {
                    return Err(format!("wire {} already exists", wire.id.0));
                }
                graph.wires.push(wire.clone());
            }
            GraphPatchOp::RemoveWire { wire_id } | GraphPatchOp::Disconnect { wire_id } => {
                if !graph.wires.iter().any(|wire| &wire.id == wire_id) {
                    return Err(format!("wire {} does not exist", wire_id.0));
                }
                graph.wires.retain(|wire| &wire.id != wire_id);
            }
            GraphPatchOp::AddVariable { variable } => {
                if graph.variables.iter().any(|existing| existing.name == variable.name) {
                    return Err(format!("variable {} already exists", variable.name));
                }
                graph.variables.push(variable.clone());
            }
            GraphPatchOp::RemoveVariable { name } => {
                if !graph.variables.iter().any(|variable| &variable.name == name) {
                    return Err(format!("variable {name} does not exist"));
                }
                graph.variables.retain(|variable| &variable.name != name);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("patch rejected: {}", errors.join("; "))]
pub struct PatchRejection {
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum CopilotError {
    #[error("no model is configured for copilot suggestions")]
    NoModel,
    #[error("model call failed: {0}")]
    Model(String),
    #[error("model reply is not a patch: {0}")]
    Unparseable(String),
    #[error(transparent)]
    Rejected(#[from] PatchRejection),
}

struct CopilotModel {
    gateway: Arc<dyn LlmGateway>,
    provider: String,
    model: String,
}

pub struct CopilotEngine {
    validator: SpellValidator,
    model: Option<CopilotModel>,
}

impl CopilotEngine {
    pub const MAX_OPS: usize = 200;

    pub fn new(validator: SpellValidator) -> Self {
        Self { validator, model: None }
    }

    pub fn with_model(mut self, gateway: Arc<dyn LlmGateway>, provider: impl Into<String>, model: impl Into<String>) -> Self {
        self.model = Some(CopilotModel { gateway, provider: provider.into(), model: model.into() });
        self
    }

    pub fn apply(&self, graph: &SpellGraph, ops: &[GraphPatchOp]) -> Result<SpellGraph, PatchRejection> {
        if ops.len() > Self::MAX_OPS {
            return Err(PatchRejection { errors: vec![format!("a patch may hold at most {} ops", Self::MAX_OPS)] });
        }
        let mut patched = graph.clone();
        let op_errors: Vec<String> =
            ops.iter().enumerate().filter_map(|(index, op)| op.try_apply(&mut patched).err().map(|error| format!("op {index}: {error}"))).collect();
        if !op_errors.is_empty() {
            return Err(PatchRejection { errors: op_errors });
        }
        self.validator.validate(&patched).map_err(|errors| PatchRejection { errors: errors.iter().map(ToString::to_string).collect() })?;
        Ok(patched)
    }

    pub fn plan(&self, request: &CopilotRequest) -> Option<CopilotSuggestion> {
        let words: Vec<&str> = request.prompt.split_whitespace().collect();
        let targets: Vec<NodeId> = match words.as_slice() {
            ["remove" | "delete", "selection" | "selected"] => request.selection.clone(),
            ["remove" | "delete", rest @ ..] if !rest.is_empty() => rest.iter().map(|id| NodeId::from(id.trim_matches(','))).collect(),
            [verb, ..] if matches!(*verb, "disconnect" | "isolate") && !request.selection.is_empty() => {
                let ops: Vec<GraphPatchOp> = request
                    .spell
                    .wires
                    .iter()
                    .filter(|wire| request.selection.iter().any(|id| id == &wire.from_node || id == &wire.to_node))
                    .map(|wire| GraphPatchOp::Disconnect { wire_id: wire.id.clone() })
                    .collect();
                return Some(CopilotSuggestion { summary: format!("Disconnect {} wire(s) from the selection", ops.len()), ops });
            }
            _ => return None,
        };
        let ops: Vec<GraphPatchOp> =
            targets.into_iter().filter(|id| request.spell.node(id).is_some()).map(|node_id| GraphPatchOp::RemoveNode { node_id }).collect();
        (!ops.is_empty()).then(|| CopilotSuggestion { summary: format!("Remove {} node(s)", ops.len()), ops })
    }

    pub async fn suggest(&self, request: &CopilotRequest) -> Result<(CopilotSuggestion, SpellGraph), CopilotError> {
        let suggestion = match self.plan(request) {
            Some(planned) => planned,
            None => self.ask_model(request).await?,
        };
        let patched = self.apply(&request.spell, &suggestion.ops)?;
        Ok((suggestion, patched))
    }

    async fn ask_model(&self, request: &CopilotRequest) -> Result<CopilotSuggestion, CopilotError> {
        let model = self.model.as_ref().ok_or(CopilotError::NoModel)?;
        let graph = serde_json::to_string(&request.spell).map_err(|error| CopilotError::Unparseable(error.to_string()))?;
        let selection = serde_json::to_string(&request.selection).map_err(|error| CopilotError::Unparseable(error.to_string()))?;
        let instructions = "You edit MagickLaLa spell graphs. Reply with one JSON object {\"summary\": string, \"ops\": [...]} and nothing else. \
            Each op has an \"op\" field: add_node {node}, remove_node {node_id}, move_node {node_id, position}, connect {wire}, disconnect {wire_id}, \
            update_config {node_id, config}, set_config {node_id, config}, add_variable {variable}, remove_variable {name}.";
        let llm_request = LlmRequest {
            provider: model.provider.clone(),
            model: model.model.clone(),
            messages: vec![
                LlmMessage { role: LlmRole::System, content: instructions.to_owned() },
                LlmMessage { role: LlmRole::User, content: format!("Graph: {graph}\nSelection: {selection}\nRequest: {}", request.prompt) },
            ],
            temperature: Some(0.0),
            max_tokens: Some(4_096),
            stop: Vec::new(),
            credential_id: None,
        };
        let reply = model.gateway.complete(llm_request).await.map_err(|error| CopilotError::Model(error.to_string()))?;
        parse_suggestion(&reply.text)
    }
}

pub fn parse_suggestion(text: &str) -> Result<CopilotSuggestion, CopilotError> {
    let start = text.find('{').ok_or_else(|| CopilotError::Unparseable("no JSON object in reply".to_owned()))?;
    let end = text.rfind('}').filter(|end| *end > start).ok_or_else(|| CopilotError::Unparseable("unterminated JSON object".to_owned()))?;
    serde_json::from_str(&text[start..=end]).map_err(|error| CopilotError::Unparseable(error.to_string()))
}
