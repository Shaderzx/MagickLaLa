use magicklala_graph::SocketType;
use rune::{Color, El, LineCap, Path, Shape, canvas, col, el, text};

use crate::{CanvasState, NodeCardModel};

const SURFACE: Color = Color { r: 0.07, g: 0.075, b: 0.09, a: 1.0 };
const CARD: Color = Color { r: 0.13, g: 0.14, b: 0.17, a: 1.0 };
const CARD_SELECTED: Color = Color { r: 0.18, g: 0.2, b: 0.26, a: 1.0 };
const INK: Color = Color { r: 0.92, g: 0.93, b: 0.95, a: 1.0 };
const MUTED: Color = Color { r: 0.55, g: 0.58, b: 0.64, a: 1.0 };
const SOCKET_SIZE: f32 = 10.0;

pub fn socket_color(ty: SocketType) -> Color {
    let (r, g, b) = match ty {
        SocketType::Flow => (0.95, 0.95, 0.97),
        SocketType::String => (0.35, 0.75, 0.45),
        SocketType::Int => (0.35, 0.55, 1.0),
        SocketType::Float => (0.45, 0.8, 0.95),
        SocketType::Bool => (0.95, 0.4, 0.4),
        SocketType::Json => (0.95, 0.7, 0.3),
        SocketType::Array => (0.75, 0.5, 0.95),
        SocketType::Embedding => (0.95, 0.45, 0.8),
        SocketType::Event => (1.0, 0.85, 0.35),
        SocketType::Any => (0.6, 0.62, 0.66),
    };
    Color { r, g, b, a: 1.0 }
}

pub fn canvas_view(state: &CanvasState, size: (f32, f32)) -> El {
    let wires = state.wires.iter().fold(Shape::new(), |shape, wire| {
        let from = state.camera.world_to_screen(wire.from);
        let to = state.camera.world_to_screen(wire.to);
        let bend = ((to[0] - from[0]).abs() * 0.5).max(60.0 * state.camera.zoom);
        let path = Path::new().move_to(from).cubic_to([from[0] + bend, from[1]], [to[0] - bend, to[1]], to);
        let width = if wire.active { 3.5 } else { 2.0 } * state.camera.zoom;
        shape.stroke_path(&path, width, LineCap::Round, socket_color(wire.ty))
    });
    let layer = canvas(wires).key("canvas-wires").size(size.0, size.1).absolute(0.0, 0.0);
    state.nodes.iter().fold(el().key("canvas").size(size.0, size.1).fill(SURFACE).child(layer), |root, card| root.child(card_view(state, card)))
}

fn card_view(state: &CanvasState, card: &NodeCardModel) -> El {
    let zoom = state.camera.zoom;
    let [x, y] = state.camera.world_to_screen(card.position);
    let selected = state.selection.as_ref() == Some(&card.id);
    let header = col().gap(2.0).padding(10.0).child(text(&card.title).color(INK)).child(text(&card.kind).color(MUTED));
    let sockets = card.inputs.iter().chain(card.outputs.iter()).map(|socket| {
        el().key(format!("socket-{}-{}-{}", card.id.0, socket.name, socket.offset[0]))
            .size(SOCKET_SIZE * zoom, SOCKET_SIZE * zoom)
            .radius(SOCKET_SIZE * zoom / 2.0)
            .fill(socket_color(socket.ty))
            .absolute((socket.offset[0] - SOCKET_SIZE / 2.0) * zoom, (socket.offset[1] - SOCKET_SIZE / 2.0) * zoom)
    });
    sockets.fold(
        el().key(format!("node-{}", card.id.0))
            .size(card.size[0] * zoom, card.size[1] * zoom)
            .radius(10.0 * zoom)
            .fill(if selected { CARD_SELECTED } else { CARD })
            .absolute(x, y)
            .child(header),
        |card_el, socket| card_el.child(socket),
    )
}
