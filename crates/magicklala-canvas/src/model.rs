use magicklala_graph::{NodeId, NodeSpec, SocketSpec, SocketType, SpellGraph, WireId};

use crate::Camera;

pub const CARD_WIDTH: f32 = 200.0;
pub const CARD_HEADER: f32 = 44.0;
pub const SOCKET_ROW: f32 = 22.0;

#[derive(Debug, Clone, PartialEq)]
pub struct SocketModel {
    pub name: String,
    pub ty: SocketType,
    pub offset: [f32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeCardModel {
    pub id: NodeId,
    pub title: String,
    pub kind: String,
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub inputs: Vec<SocketModel>,
    pub outputs: Vec<SocketModel>,
}

impl NodeCardModel {
    pub fn new(id: NodeId, kind: &str, position: [f32; 2], spec: Option<&NodeSpec>) -> Self {
        let place = |sockets: &[SocketSpec], x: f32| -> Vec<SocketModel> {
            sockets
                .iter()
                .enumerate()
                .map(|(row, socket)| SocketModel { name: socket.name.clone(), ty: socket.ty, offset: [x, CARD_HEADER + SOCKET_ROW * (row as f32 + 0.5)] })
                .collect()
        };
        let inputs = spec.map(|spec| place(&spec.inputs, 0.0)).unwrap_or_default();
        let outputs = spec.map(|spec| place(&spec.outputs, CARD_WIDTH)).unwrap_or_default();
        let rows = inputs.len().max(outputs.len()) as f32;
        Self {
            id,
            title: spec.map_or_else(|| kind.to_owned(), |spec| spec.title.clone()),
            kind: kind.to_owned(),
            position,
            size: [CARD_WIDTH, CARD_HEADER + SOCKET_ROW * rows + 8.0],
            inputs,
            outputs,
        }
    }

    pub fn socket_world(&self, name: &str, output: bool) -> Option<([f32; 2], SocketType)> {
        let sockets = if output { &self.outputs } else { &self.inputs };
        sockets.iter().find(|socket| socket.name == name).map(|socket| ([self.position[0] + socket.offset[0], self.position[1] + socket.offset[1]], socket.ty))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WireModel {
    pub id: WireId,
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub ty: SocketType,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CanvasEvent {
    Pan { dx: f32, dy: f32 },
    Zoom { pointer: [f32; 2], factor: f32 },
    Select { node: Option<NodeId> },
    MoveNode { node: NodeId, position: [f32; 2] },
    ConnectWire { from: (NodeId, String), to: (NodeId, String) },
    WireActivated { wire: WireId },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CanvasState {
    pub camera: Camera,
    pub nodes: Vec<NodeCardModel>,
    pub wires: Vec<WireModel>,
    pub selection: Option<NodeId>,
}

impl CanvasState {
    pub fn from_graph(graph: &SpellGraph, specs: &[NodeSpec]) -> Self {
        let nodes: Vec<NodeCardModel> = graph
            .nodes
            .iter()
            .map(|node| NodeCardModel::new(node.id.clone(), &node.kind, node.position, specs.iter().find(|spec| spec.kind == node.kind)))
            .collect();
        let mut state = Self { camera: Camera::default(), nodes, wires: Vec::new(), selection: None };
        state.wires = graph
            .wires
            .iter()
            .filter_map(|wire| {
                let (from, ty) = state.card(&wire.from_node)?.socket_world(&wire.from_socket, true)?;
                let (to, _) = state.card(&wire.to_node)?.socket_world(&wire.to_socket, false)?;
                Some(WireModel { id: wire.id.clone(), from, to, ty, active: false })
            })
            .collect();
        state
    }

    pub fn card(&self, id: &NodeId) -> Option<&NodeCardModel> {
        self.nodes.iter().find(|card| &card.id == id)
    }

    pub fn apply(&mut self, event: CanvasEvent) {
        match event {
            CanvasEvent::Pan { dx, dy } => self.camera.pan_by(dx, dy),
            CanvasEvent::Zoom { pointer, factor } => self.camera.zoom_at(pointer, factor),
            CanvasEvent::Select { node } => self.selection = node,
            CanvasEvent::MoveNode { node, position } => {
                self.nodes.iter_mut().filter(|card| card.id == node).for_each(|card| card.position = position);
            }
            CanvasEvent::WireActivated { wire } => {
                self.wires.iter_mut().filter(|model| model.id == wire).for_each(|model| model.active = true);
            }
            CanvasEvent::ConnectWire { .. } => {}
        }
    }
}
