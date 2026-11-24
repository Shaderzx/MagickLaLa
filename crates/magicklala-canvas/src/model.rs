use magicklala_graph::{DYNAMIC_SOCKET, NodeId, NodeSpec, RunStatus, SocketSpec, SocketType, SpellGraph, TraceEvent, WireId};

use crate::Camera;

pub const CARD_WIDTH: f32 = 218.0;
pub const CARD_HEADER: f32 = 44.0;
pub const SOCKET_ROW: f32 = 22.0;
pub const SOCKET_HIT_RADIUS: f32 = 14.0;
const BODY_TOP: f32 = 6.0;

#[derive(Debug, Clone, PartialEq)]
pub struct SocketModel {
    pub name: String,
    pub ty: SocketType,
    pub offset: [f32; 2],
    pub flagged: bool,
    pub mistyped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionStatus {
    #[default]
    Idle,
    Running,
    Succeeded,
    Failed,
}

impl ExecutionStatus {
    pub fn label(self) -> &'static str {
        match self {
            ExecutionStatus::Idle => "idle",
            ExecutionStatus::Running => "running",
            ExecutionStatus::Succeeded => "done",
            ExecutionStatus::Failed => "failed",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            ExecutionStatus::Idle => "Idle",
            ExecutionStatus::Running => "Simmering",
            ExecutionStatus::Succeeded => "Brewed",
            ExecutionStatus::Failed => "Hexed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkbenchMode {
    #[default]
    Cauldron,
    Grimoire,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeCardModel {
    pub id: NodeId,
    pub title: String,
    pub kind: String,
    pub category: String,
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub inputs: Vec<SocketModel>,
    pub outputs: Vec<SocketModel>,
    pub selected: bool,
    pub status: ExecutionStatus,
    pub duration_ms: Option<u64>,
    pub step_order: Option<usize>,
}

impl NodeCardModel {
    pub fn new(id: NodeId, kind: &str, position: [f32; 2], spec: Option<&NodeSpec>) -> Self {
        let place = |sockets: &[SocketSpec], x: f32| -> Vec<SocketModel> {
            sockets
                .iter()
                .filter(|socket| socket.name != DYNAMIC_SOCKET)
                .enumerate()
                .map(|(row, socket)| SocketModel {
                    name: socket.name.clone(),
                    ty: socket.ty,
                    offset: [x, CARD_HEADER + BODY_TOP + SOCKET_ROW * (row as f32 + 0.5)],
                    flagged: false,
                    mistyped: false,
                })
                .collect()
        };
        let inputs = spec.map(|spec| place(&spec.inputs, 0.0)).unwrap_or_default();
        let outputs = spec.map(|spec| place(&spec.outputs, CARD_WIDTH)).unwrap_or_default();
        Self {
            id,
            title: spec.map_or_else(|| kind.to_owned(), |spec| spec.title.clone()),
            kind: kind.to_owned(),
            category: spec.map_or_else(String::new, |spec| spec.category.clone()),
            position,
            size: [CARD_WIDTH, card_height(inputs.len().max(outputs.len()))],
            inputs,
            outputs,
            selected: false,
            status: ExecutionStatus::Idle,
            duration_ms: None,
            step_order: None,
        }
    }

    pub fn ensure_socket(&mut self, name: &str, ty: SocketType, output: bool) {
        let (sockets, x) = if output { (&mut self.outputs, CARD_WIDTH) } else { (&mut self.inputs, 0.0) };
        if sockets.iter().any(|socket| socket.name == name) {
            return;
        }
        let socket = SocketModel { name: name.to_owned(), ty, offset: [x, 0.0], flagged: false, mistyped: false };
        if output && name != "default" && sockets.last().is_some_and(|last| last.name == "default") {
            let insert_at = sockets.len() - 1;
            sockets.insert(insert_at, socket);
        } else {
            sockets.push(socket);
        }
        for (row, entry) in sockets.iter_mut().enumerate() {
            entry.offset = [x, CARD_HEADER + BODY_TOP + SOCKET_ROW * (row as f32 + 0.5)];
        }
        self.size[1] = card_height(self.inputs.len().max(self.outputs.len()));
    }

    pub fn socket_world(&self, name: &str, output: bool) -> Option<([f32; 2], SocketType)> {
        let sockets = if output { &self.outputs } else { &self.inputs };
        sockets.iter().find(|socket| socket.name == name).map(|socket| ([self.position[0] + socket.offset[0], self.position[1] + socket.offset[1]], socket.ty))
    }

    pub fn contains(&self, world: [f32; 2]) -> bool {
        world[0] >= self.position[0]
            && world[0] <= self.position[0] + self.size[0]
            && world[1] >= self.position[1]
            && world[1] <= self.position[1] + self.size[1]
    }
}

fn card_height(rows: usize) -> f32 {
    CARD_HEADER + BODY_TOP * 2.0 + SOCKET_ROW * rows.max(1) as f32
}

fn resolved_type(spec: Option<&NodeSpec>, name: &str, output: bool) -> Option<SocketType> {
    let spec = spec?;
    let socket = if output { spec.resolve_output(name) } else { spec.resolve_input(name) };
    socket.map(|socket| socket.ty)
}

#[derive(Debug, Clone, PartialEq)]
pub struct WireModel {
    pub id: WireId,
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub ty: SocketType,
    pub active: bool,
    pub from_node: NodeId,
    pub from_socket: String,
    pub to_node: NodeId,
    pub to_socket: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SocketRef {
    pub node: NodeId,
    pub socket: String,
}

impl SocketRef {
    pub fn new(node: impl Into<NodeId>, socket: impl Into<String>) -> Self {
        Self { node: node.into(), socket: socket.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum DragState {
    #[default]
    Idle,
    DraggingNode {
        node: NodeId,
        origin: [f32; 2],
    },
    Panning {
        origin: [f32; 2],
    },
    LiftingVial {
        node: NodeId,
        offset: [f32; 2],
    },
    ConnectingWire {
        from: SocketRef,
        ty: SocketType,
        anchor: [f32; 2],
        cursor: [f32; 2],
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    UnknownSocket { node: NodeId, socket: String },
    FlowMismatch { from: SocketType, to: SocketType },
    TypeMismatch { from: SocketType, to: SocketType },
    SameNode,
    Duplicate,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::UnknownSocket { node, socket } => write!(f, "no socket {socket} on {}", node.0),
            ConnectError::FlowMismatch { from, to } => write!(f, "flow and value sockets cannot meet ({from:?} to {to:?})"),
            ConnectError::TypeMismatch { from, to } => write!(f, "{from:?} does not fit {to:?}"),
            ConnectError::SameNode => write!(f, "a node cannot wire into itself"),
            ConnectError::Duplicate => write!(f, "that wire already exists"),
        }
    }
}

impl std::error::Error for ConnectError {}

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
    pub brew_camera: Camera,
    pub nodes: Vec<NodeCardModel>,
    pub wires: Vec<WireModel>,
    pub selection: Option<NodeId>,
    pub hovered: Option<NodeId>,
    pub drag: DragState,
    pub context_menu: Option<[f32; 2]>,
    pub context_card: Option<[f32; 4]>,
    pub grimoire_fitted: bool,
    pub last_connect_error: Option<ConnectError>,
    pub light: bool,
    pub mode: WorkbenchMode,
    pub spell_name: String,
    pub release_badge: Option<String>,
    pub brew_summary: Option<String>,
    pub validated: bool,
    pub invalid_brew: bool,
    pub running_brew: bool,
    pub last_brew: Option<RunStatus>,
    pub toss_count: u32,
    pub pot_hover: bool,
    pub shelved: Vec<NodeId>,
    next_id: u64,
}

impl CanvasState {
    pub fn from_graph(graph: &SpellGraph, specs: &[NodeSpec]) -> Self {
        let nodes: Vec<NodeCardModel> = graph
            .nodes
            .iter()
            .map(|node| NodeCardModel::new(node.id.clone(), &node.kind, node.position, specs.iter().find(|spec| spec.kind == node.kind)))
            .collect();
        let mut state = Self { nodes, light: true, mode: WorkbenchMode::Cauldron, spell_name: graph.name.clone(), ..Self::default() };
        let spec_of = |id: &NodeId| graph.nodes.iter().find(|node| &node.id == id).and_then(|node| specs.iter().find(|spec| spec.kind == node.kind));
        for wire in &graph.wires {
            let from_ty = resolved_type(spec_of(&wire.from_node), &wire.from_socket, true);
            let to_ty = resolved_type(spec_of(&wire.to_node), &wire.to_socket, false);
            let fallback = from_ty.or(to_ty).unwrap_or(SocketType::Any);
            if let Some(card) = state.card_mut(&wire.from_node) {
                card.ensure_socket(&wire.from_socket, from_ty.unwrap_or(fallback), true);
            }
            if let Some(card) = state.card_mut(&wire.to_node) {
                card.ensure_socket(&wire.to_socket, to_ty.unwrap_or(fallback), false);
            }
        }
        state.wires = graph
            .wires
            .iter()
            .filter_map(|wire| {
                let from = SocketRef::new(wire.from_node.clone(), wire.from_socket.clone());
                let to = SocketRef::new(wire.to_node.clone(), wire.to_socket.clone());
                state.wire_model(wire.id.clone(), &from, &to)
            })
            .collect();
        state
    }

    fn wire_model(&self, id: WireId, from: &SocketRef, to: &SocketRef) -> Option<WireModel> {
        let (from_point, ty) = self.card(&from.node)?.socket_world(&from.socket, true)?;
        let (to_point, _) = self.card(&to.node)?.socket_world(&to.socket, false)?;
        Some(WireModel {
            id,
            from: from_point,
            to: to_point,
            ty,
            active: false,
            from_node: from.node.clone(),
            from_socket: from.socket.clone(),
            to_node: to.node.clone(),
            to_socket: to.socket.clone(),
        })
    }

    pub fn display_title(&self, id: &NodeId) -> String {
        let Some(card) = self.card(id) else { return id.0.clone() };
        let twins: Vec<&NodeId> = self.nodes.iter().filter(|other| other.title == card.title).map(|other| &other.id).collect();
        if twins.len() < 2 {
            return card.title.clone();
        }
        let short = card.title.split_whitespace().next().unwrap_or(&card.title);
        let route =
            self.wires.iter().find(|wire| &wire.to_node == id && wire.ty.is_flow()).map(|wire| wire.from_socket.as_str()).filter(|socket| *socket != "out");
        match route {
            Some(route) => format!("{short} · {route}"),
            None => format!("{short} · {}", twins.iter().position(|twin| *twin == id).unwrap_or_default() + 1),
        }
    }

    pub fn card(&self, id: &NodeId) -> Option<&NodeCardModel> {
        self.nodes.iter().find(|card| &card.id == id)
    }

    pub fn card_mut(&mut self, id: &NodeId) -> Option<&mut NodeCardModel> {
        self.nodes.iter_mut().find(|card| &card.id == id)
    }

    pub fn selected_card(&self) -> Option<&NodeCardModel> {
        self.selection.as_ref().and_then(|id| self.card(id))
    }

    pub fn select(&mut self, node: Option<NodeId>) {
        self.nodes.iter_mut().for_each(|card| card.selected = Some(&card.id) == node.as_ref());
        self.selection = node;
        self.context_menu = None;
    }

    pub fn free_spot(&self, id: &NodeId, near: [f32; 2]) -> [f32; 2] {
        let Some(size) = self.card(id).map(|card| card.size) else { return near };
        let clear = |at: [f32; 2]| {
            self.nodes.iter().filter(|other| &other.id != id).all(|other| {
                at[0] + size[0] + 24.0 <= other.position[0]
                    || other.position[0] + other.size[0] + 24.0 <= at[0]
                    || at[1] + size[1] + 24.0 <= other.position[1]
                    || other.position[1] + other.size[1] + 24.0 <= at[1]
            })
        };
        let ring = |step: usize| {
            let lap = (step / 8 + 1) as f32;
            let angle = (step % 8) as f32 * std::f32::consts::FRAC_PI_4;
            [near[0] + angle.cos() * lap * (size[0] + 32.0) * 0.5, near[1] + angle.sin() * lap * (size[1] + 32.0)]
        };
        std::iter::once(near).chain((0..48).map(ring)).find(|at| clear(*at)).unwrap_or(near)
    }

    pub fn move_node(&mut self, id: &NodeId, position: [f32; 2]) -> bool {
        let Some(card) = self.card_mut(id) else { return false };
        card.position = position;
        let card = card.clone();
        for wire in self.wires.iter_mut() {
            if wire.from_node == card.id
                && let Some((point, _)) = card.socket_world(&wire.from_socket, true)
            {
                wire.from = point;
            }
            if wire.to_node == card.id
                && let Some((point, _)) = card.socket_world(&wire.to_socket, false)
            {
                wire.to = point;
            }
        }
        true
    }

    fn fresh_id(&mut self, prefix: &str) -> String {
        loop {
            self.next_id += 1;
            let candidate = format!("{prefix}-{}", self.next_id);
            let taken = self.nodes.iter().any(|card| card.id.0 == candidate) || self.wires.iter().any(|wire| wire.id.0 == candidate);
            if !taken {
                return candidate;
            }
        }
    }

    pub fn add_node_from_spec(&mut self, spec: &NodeSpec, position: [f32; 2]) -> NodeId {
        let slug = spec.kind.rsplit('/').next().unwrap_or(&spec.kind).replace('_', "-");
        let id = NodeId::from(self.fresh_id(&slug).as_str());
        self.nodes.push(NodeCardModel::new(id.clone(), &spec.kind, position, Some(spec)));
        self.select(Some(id.clone()));
        id
    }

    pub fn remove_selected_node(&mut self) -> Option<NodeCardModel> {
        let id = self.selection.take()?;
        let index = self.nodes.iter().position(|card| card.id == id)?;
        self.wires.retain(|wire| wire.from_node != id && wire.to_node != id);
        self.unshelve(&id);
        Some(self.nodes.remove(index))
    }

    pub fn restore_node(&mut self, card: NodeCardModel, wires: Vec<WireModel>) {
        let id = card.id.clone();
        if !self.nodes.iter().any(|existing| existing.id == id) {
            self.nodes.push(card);
        }
        for wire in wires {
            if self.card(&wire.from_node).is_some() && self.card(&wire.to_node).is_some() && !self.wires.iter().any(|existing| existing.id == wire.id) {
                self.wires.push(wire);
            }
        }
        self.select(Some(id));
    }

    pub fn connect_sockets(&mut self, from: &SocketRef, to: &SocketRef) -> Result<WireId, ConnectError> {
        let unknown = |socket: &SocketRef| ConnectError::UnknownSocket { node: socket.node.clone(), socket: socket.socket.clone() };
        let (_, from_ty) = self.card(&from.node).and_then(|card| card.socket_world(&from.socket, true)).ok_or_else(|| unknown(from))?;
        let (_, to_ty) = self.card(&to.node).and_then(|card| card.socket_world(&to.socket, false)).ok_or_else(|| unknown(to))?;
        if from.node == to.node {
            return Err(ConnectError::SameNode);
        }
        if from_ty.is_flow() != to_ty.is_flow() {
            return Err(ConnectError::FlowMismatch { from: from_ty, to: to_ty });
        }
        if !to_ty.accepts(from_ty) {
            return Err(ConnectError::TypeMismatch { from: from_ty, to: to_ty });
        }
        let exists = self
            .wires
            .iter()
            .any(|wire| wire.from_node == from.node && wire.from_socket == from.socket && wire.to_node == to.node && wire.to_socket == to.socket);
        if exists {
            return Err(ConnectError::Duplicate);
        }
        self.unshelve(&from.node);
        self.unshelve(&to.node);
        if !to_ty.is_flow() {
            self.wires.retain(|wire| !(wire.to_node == to.node && wire.to_socket == to.socket));
        }
        let id = WireId::from(self.fresh_id("wire").as_str());
        let model = self.wire_model(id.clone(), from, to).ok_or_else(|| unknown(from))?;
        self.wires.push(model);
        Ok(id)
    }

    pub fn input_socket_at(&self, world: [f32; 2]) -> Option<SocketRef> {
        self.nodes.iter().rev().find_map(|card| {
            card.inputs.iter().find_map(|socket| {
                let x = card.position[0] + socket.offset[0];
                let y = card.position[1] + socket.offset[1];
                let near = (world[0] - x).hypot(world[1] - y) <= SOCKET_HIT_RADIUS;
                near.then(|| SocketRef::new(card.id.clone(), socket.name.clone()))
            })
        })
    }

    pub fn node_at(&self, world: [f32; 2]) -> Option<NodeId> {
        self.nodes.iter().rev().find(|card| card.contains(world)).map(|card| card.id.clone())
    }

    pub fn reset_execution(&mut self) {
        self.nodes.iter_mut().for_each(|card| {
            card.status = ExecutionStatus::Idle;
            card.duration_ms = None;
            card.step_order = None;
        });
        self.wires.iter_mut().for_each(|wire| wire.active = false);
    }

    pub fn apply_trace_event(&mut self, event: &TraceEvent) {
        if !event.subgraph_path().is_empty() {
            return;
        }
        match event {
            TraceEvent::RunStarted { .. } => self.reset_execution(),
            TraceEvent::NodeStarted { node_id, .. } => self.set_status(node_id, ExecutionStatus::Running),
            TraceEvent::NodeFinished { node_id, duration_ms, .. } => {
                let next_step = self.nodes.iter().filter_map(|card| card.step_order).max().unwrap_or(0) + 1;
                if let Some(card) = self.card_mut(node_id) {
                    card.status = ExecutionStatus::Succeeded;
                    card.duration_ms = Some(*duration_ms);
                    if card.step_order.is_none() {
                        card.step_order = Some(next_step);
                    }
                }
            }
            TraceEvent::NodeFailed { node_id, .. } => self.set_status(node_id, ExecutionStatus::Failed),
            TraceEvent::WireActivated { wire_id, .. } => self.wires.iter_mut().filter(|wire| &wire.id == wire_id).for_each(|wire| wire.active = true),
            TraceEvent::RunFinished { summary, .. } if summary.status != RunStatus::Succeeded => {
                self.nodes.iter_mut().filter(|card| card.status == ExecutionStatus::Running).for_each(|card| card.status = ExecutionStatus::Failed);
            }
            TraceEvent::RunFinished { .. } | TraceEvent::StreamChunk { .. } | TraceEvent::VariableSet { .. } => {}
        }
    }

    fn set_status(&mut self, id: &NodeId, status: ExecutionStatus) {
        if let Some(card) = self.card_mut(id) {
            card.status = status;
        }
    }

    pub fn is_shelved(&self, id: &NodeId) -> bool {
        self.shelved.contains(id)
    }

    pub fn shelve(&mut self, id: &NodeId) {
        if !self.shelved.contains(id) {
            self.shelved.push(id.clone());
        }
    }

    pub fn unshelve(&mut self, id: &NodeId) {
        self.shelved.retain(|other| other != id);
    }

    pub fn set_aside(&mut self, id: &NodeId) -> usize {
        self.shelve(id);
        let before = self.wires.len();
        self.wires.retain(|wire| &wire.from_node != id && &wire.to_node != id);
        before - self.wires.len()
    }

    pub fn apply_toss(&mut self, toss: &crate::Toss) {
        if let crate::Toss::Aside(id) = toss {
            self.set_aside(id);
        }
    }

    pub fn active_camera(&self) -> &Camera {
        match self.mode {
            WorkbenchMode::Cauldron => &self.brew_camera,
            WorkbenchMode::Grimoire => &self.camera,
        }
    }

    pub fn zoom_active(&mut self, factor: f32, pivot: [f32; 2]) {
        match self.mode {
            WorkbenchMode::Cauldron => self.brew_camera.zoom_about_origin(factor),
            WorkbenchMode::Grimoire => self.camera.zoom_by(factor, pivot),
        }
    }

    pub fn clear_run_marks(&mut self) {
        self.wires.iter_mut().for_each(|wire| wire.active = false);
        for card in &mut self.nodes {
            card.status = ExecutionStatus::Idle;
            card.duration_ms = None;
            card.step_order = None;
        }
        self.brew_summary = None;
        self.last_brew = None;
        self.running_brew = false;
    }

    pub fn has_run(&self) -> bool {
        self.wires.iter().any(|wire| wire.active)
    }

    pub fn fit(&mut self, viewport: (f32, f32)) {
        if self.mode == WorkbenchMode::Cauldron {
            self.brew_camera.reset();
            return;
        }
        if self.nodes.is_empty() {
            self.camera.reset();
            return;
        }
        let min_x = self.nodes.iter().map(|card| card.position[0]).fold(f32::MAX, f32::min);
        let min_y = self.nodes.iter().map(|card| card.position[1]).fold(f32::MAX, f32::min);
        let max_x = self.nodes.iter().map(|card| card.position[0] + card.size[0]).fold(f32::MIN, f32::max);
        let max_y = self.nodes.iter().map(|card| card.position[1] + card.size[1]).fold(f32::MIN, f32::max);
        let margin = 16.0;
        let zoom = ((viewport.0 - margin * 2.0) / (max_x - min_x).max(1.0))
            .min((viewport.1 - margin * 2.0) / (max_y - min_y).max(1.0))
            .clamp(Camera::MIN_ZOOM, Camera::MAX_ZOOM);
        self.camera.zoom = zoom;
        self.camera.pan_x = (viewport.0 - (max_x - min_x) * zoom) / 2.0 - min_x * zoom;
        self.camera.pan_y = (viewport.1 - (max_y - min_y) * zoom) / 2.0 - min_y * zoom;
    }

    pub fn apply(&mut self, event: CanvasEvent) {
        match event {
            CanvasEvent::Pan { dx, dy } => self.camera.pan_by(dx, dy),
            CanvasEvent::Zoom { pointer, factor } => self.camera.zoom_at(pointer, factor),
            CanvasEvent::Select { node } => self.select(node),
            CanvasEvent::MoveNode { node, position } => {
                self.move_node(&node, position);
            }
            CanvasEvent::WireActivated { wire } => {
                self.wires.iter_mut().filter(|model| model.id == wire).for_each(|model| model.active = true);
            }
            CanvasEvent::ConnectWire { from, to } => {
                self.last_connect_error = self.connect_sockets(&SocketRef::new(from.0, from.1), &SocketRef::new(to.0, to.1)).err();
            }
        }
    }
}
