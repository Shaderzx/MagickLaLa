use magicklala_graph::{NodeId, NodeInstance, SpellGraph, VariableDef, Wire, WireId};
use serde::{Deserialize, Serialize};

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
}

impl GraphPatchOp {
    pub fn apply(&self, graph: &mut SpellGraph) {
        match self {
            GraphPatchOp::AddNode { node } => graph.nodes.push(node.clone()),
            GraphPatchOp::RemoveNode { node_id } => {
                graph.nodes.retain(|node| &node.id != node_id);
                graph.wires.retain(|wire| &wire.from_node != node_id && &wire.to_node != node_id);
            }
            GraphPatchOp::MoveNode { node_id, position } => {
                graph.nodes.iter_mut().filter(|node| &node.id == node_id).for_each(|node| node.position = *position);
            }
            GraphPatchOp::SetConfig { node_id, config } => {
                graph.nodes.iter_mut().filter(|node| &node.id == node_id).for_each(|node| node.config = config.clone());
            }
            GraphPatchOp::AddWire { wire } => graph.wires.push(wire.clone()),
            GraphPatchOp::RemoveWire { wire_id } => graph.wires.retain(|wire| &wire.id != wire_id),
            GraphPatchOp::AddVariable { variable } => graph.variables.push(variable.clone()),
            GraphPatchOp::RemoveVariable { name } => graph.variables.retain(|variable| &variable.name != name),
        }
    }
}
