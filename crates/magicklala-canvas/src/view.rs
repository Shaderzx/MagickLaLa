use magicklala_graph::SocketType;
use rune::{Color, El, LineCap, Path, Phase, Shape, TextStyle, canvas, col, el, row, text};

use crate::model::{CARD_HEADER, SOCKET_ROW};
use crate::{CanvasState, DragState, ExecutionStatus, NodeCardModel, SocketModel, SocketRef};

const SOCKET_SIZE: f32 = 12.0;
const DOT_SPACING: f32 = 24.0;

fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color { r, g, b, a }
}

fn with_alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasPalette {
    pub surface: Color,
    pub dot: Color,
    pub card: Color,
    pub card_border: Color,
    pub ink: Color,
    pub muted: Color,
    pub ring: Color,
    pub header_ink: Color,
}

impl CanvasPalette {
    pub fn for_theme(light: bool) -> Self {
        if light {
            Self {
                surface: rgba(0.955, 0.957, 0.965, 1.0),
                dot: rgba(0.0, 0.0, 0.0, 0.14),
                card: rgba(1.0, 1.0, 1.0, 1.0),
                card_border: rgba(0.0, 0.0, 0.0, 0.12),
                ink: rgba(0.1, 0.11, 0.13, 1.0),
                muted: rgba(0.4, 0.42, 0.47, 1.0),
                ring: rgba(0.36, 0.42, 0.98, 1.0),
                header_ink: rgba(1.0, 1.0, 1.0, 1.0),
            }
        } else {
            Self {
                surface: rgba(0.058, 0.062, 0.075, 1.0),
                dot: rgba(1.0, 1.0, 1.0, 0.075),
                card: rgba(0.115, 0.122, 0.145, 1.0),
                card_border: rgba(1.0, 1.0, 1.0, 0.08),
                ink: rgba(0.92, 0.93, 0.95, 1.0),
                muted: rgba(0.55, 0.58, 0.64, 1.0),
                ring: rgba(0.52, 0.58, 1.0, 1.0),
                header_ink: rgba(1.0, 1.0, 1.0, 1.0),
            }
        }
    }
}

pub fn socket_color(ty: SocketType) -> Color {
    let (r, g, b) = match ty {
        SocketType::Flow => (0.95, 0.95, 0.97),
        SocketType::String => (0.35, 0.78, 0.5),
        SocketType::Int | SocketType::Float => (0.38, 0.62, 1.0),
        SocketType::Bool => (0.95, 0.42, 0.42),
        SocketType::Json | SocketType::Array => (0.96, 0.68, 0.3),
        SocketType::Embedding => (0.86, 0.48, 0.96),
        SocketType::Event => (1.0, 0.84, 0.34),
        SocketType::Any => (0.6, 0.62, 0.68),
    };
    Color { r, g, b, a: 1.0 }
}

fn flow_wire_color(ty: SocketType, light: bool) -> Color {
    if ty == SocketType::Flow && light { rgba(0.3, 0.32, 0.38, 1.0) } else { socket_color(ty) }
}

pub fn category_accent(category: &str) -> Color {
    match category.to_ascii_lowercase().as_str() {
        "events" | "event" => rgba(0.86, 0.55, 0.16, 1.0),
        "flow" => rgba(0.34, 0.38, 0.47, 1.0),
        "ai" => rgba(0.47, 0.36, 0.92, 1.0),
        "memory" => rgba(0.1, 0.6, 0.58, 1.0),
        "knowledge" => rgba(0.18, 0.47, 0.9, 1.0),
        "data" => rgba(0.2, 0.62, 0.36, 1.0),
        "script" => rgba(0.84, 0.3, 0.56, 1.0),
        _ => rgba(0.4, 0.43, 0.5, 1.0),
    }
}

pub fn status_color(status: ExecutionStatus) -> Color {
    match status {
        ExecutionStatus::Idle => rgba(0.55, 0.58, 0.64, 1.0),
        ExecutionStatus::Running => rgba(0.98, 0.78, 0.25, 1.0),
        ExecutionStatus::Succeeded => rgba(0.3, 0.82, 0.5, 1.0),
        ExecutionStatus::Failed => rgba(0.95, 0.36, 0.36, 1.0),
    }
}

pub fn canvas_view(state: &CanvasState, size: (f32, f32)) -> El {
    interactive_canvas_view::<CanvasState>(state, size, |state| state)
}

pub fn interactive_canvas_view<S: 'static>(state: &CanvasState, size: (f32, f32), lens: fn(&mut S) -> &mut CanvasState) -> El {
    let palette = CanvasPalette::for_theme(state.light);
    let backdrop = canvas(dot_matrix(state, size, palette)).key("canvas-dots").size(size.0, size.1).absolute(0.0, 0.0);
    let wires = canvas(wire_shape(state, palette)).key("canvas-wires").size(size.0, size.1).absolute(0.0, 0.0);
    let board = el()
        .key("canvas")
        .size(size.0, size.1)
        .fill(palette.surface)
        .clip_rounded(0.0)
        .on_drag(move |s: &mut S, drag, tx| {
            tx.instant();
            let canvas = lens(s);
            match drag.phase {
                Phase::Down => {
                    canvas.select(None);
                    canvas.drag = DragState::Panning { origin: [canvas.camera.pan_x, canvas.camera.pan_y] };
                }
                Phase::Move => {
                    if let DragState::Panning { origin } = canvas.drag {
                        canvas.camera.pan_x = origin[0] + drag.dx;
                        canvas.camera.pan_y = origin[1] + drag.dy;
                    }
                }
                _ => canvas.drag = DragState::Idle,
            }
        })
        .on_secondary_press_local(move |s: &mut S, press, tx| {
            tx.instant();
            let canvas = lens(s);
            let hit = canvas.node_at(canvas.camera.screen_to_world([press.x, press.y]));
            canvas.select(hit);
            canvas.context_menu = Some([press.x, press.y]);
        })
        .child(backdrop)
        .child(wires);
    state.nodes.iter().fold(board, |board, card| board.child(card_view(state, card, palette, lens)))
}

fn dot_matrix(state: &CanvasState, size: (f32, f32), palette: CanvasPalette) -> Shape {
    let spacing = DOT_SPACING * state.camera.zoom;
    let radius = (1.1 * state.camera.zoom).max(0.8);
    let start_x = state.camera.pan_x.rem_euclid(spacing);
    let start_y = state.camera.pan_y.rem_euclid(spacing);
    let columns = (size.0 / spacing).ceil() as usize + 1;
    let rows = (size.1 / spacing).ceil() as usize + 1;
    (0..rows).flat_map(|row| (0..columns).map(move |column| (row, column))).fold(Shape::new(), |shape, (row, column)| {
        let x = start_x + column as f32 * spacing;
        let y = start_y + row as f32 * spacing;
        shape.fill(vec![[x - radius, y - radius], [x + radius, y - radius], [x + radius, y + radius], [x - radius, y + radius]], palette.dot)
    })
}

fn bezier(from: [f32; 2], to: [f32; 2], zoom: f32) -> Path {
    let bend = ((to[0] - from[0]).abs() * 0.5).max(70.0 * zoom);
    Path::new().move_to(from).cubic_to([from[0] + bend, from[1]], [to[0] - bend, to[1]], to)
}

fn wire_shape(state: &CanvasState, palette: CanvasPalette) -> Shape {
    let zoom = state.camera.zoom;
    let shape = state.wires.iter().fold(Shape::new(), |shape, wire| {
        let path = bezier(state.camera.world_to_screen(wire.from), state.camera.world_to_screen(wire.to), zoom);
        let color = flow_wire_color(wire.ty, state.light);
        if wire.active {
            shape
                .stroke_path(&path, 12.0 * zoom, LineCap::Round, with_alpha(color, 0.16))
                .stroke_path(&path, 6.0 * zoom, LineCap::Round, with_alpha(color, 0.32))
                .stroke_path(&path, 3.0 * zoom, LineCap::Round, color)
        } else {
            shape.stroke_path(&path, 2.0 * zoom, LineCap::Round, with_alpha(color, 0.78))
        }
    });
    match &state.drag {
        DragState::ConnectingWire { ty, anchor, cursor, .. } => {
            let path = bezier(state.camera.world_to_screen(*anchor), state.camera.world_to_screen(*cursor), zoom);
            shape.stroke_path(&path, 6.0 * zoom, LineCap::Round, with_alpha(palette.ring, 0.2)).stroke_path(
                &path,
                2.5 * zoom,
                LineCap::Round,
                socket_color(*ty),
            )
        }
        _ => shape,
    }
}

fn status_badge(status: ExecutionStatus, zoom: f32) -> El {
    let color = status_color(status);
    row()
        .gap(4.0 * zoom)
        .items_center()
        .px(7.0 * zoom)
        .py(2.0 * zoom)
        .radius(9.0 * zoom)
        .fill(rgba(0.0, 0.0, 0.0, 0.28))
        .child(el().size(6.0 * zoom, 6.0 * zoom).radius(3.0 * zoom).fill(color))
        .child(text(status.label()).font(TextStyle::new(10.0 * zoom, 600.0)).color(rgba(1.0, 1.0, 1.0, 0.92)))
}

fn socket_row(card: &NodeCardModel, row_index: usize, palette: CanvasPalette, zoom: f32) -> El {
    let label =
        |socket: Option<&SocketModel>| text(socket.map_or("", |socket| socket.name.as_str())).font(TextStyle::new(11.0 * zoom, 450.0)).color(palette.muted);
    row()
        .w_full()
        .h(SOCKET_ROW * zoom)
        .px(14.0 * zoom)
        .items_center()
        .justify_between()
        .child(label(card.inputs.get(row_index)))
        .child(label(card.outputs.get(row_index)))
}

fn card_view<S: 'static>(state: &CanvasState, card: &NodeCardModel, palette: CanvasPalette, lens: fn(&mut S) -> &mut CanvasState) -> El {
    let zoom = state.camera.zoom;
    let [x, y] = state.camera.world_to_screen(card.position);
    let accent = category_accent(&card.category);
    let node_id = card.id.clone();
    let header = row()
        .key(format!("node-{}-header", card.id.0))
        .w_full()
        .h(CARD_HEADER * zoom)
        .px(12.0 * zoom)
        .items_center()
        .justify_between()
        .fill(accent)
        .child(
            col()
                .gap(1.0 * zoom)
                .child(text(&card.title).font(TextStyle::new(13.0 * zoom, 650.0)).color(palette.header_ink))
                .child(text(&card.kind).font(TextStyle::new(10.0 * zoom, 450.0)).color(rgba(1.0, 1.0, 1.0, 0.72))),
        )
        .child(status_badge(card.status, zoom))
        .on_drag(move |s: &mut S, drag, tx| {
            tx.instant();
            let canvas = lens(s);
            match drag.phase {
                Phase::Down => {
                    canvas.select(Some(node_id.clone()));
                    let origin = canvas.card(&node_id).map_or([0.0, 0.0], |card| card.position);
                    canvas.drag = DragState::DraggingNode { node: node_id.clone(), origin };
                }
                Phase::Move => {
                    if let DragState::DraggingNode { node, origin } = canvas.drag.clone() {
                        let zoom = canvas.camera.zoom;
                        canvas.move_node(&node, [origin[0] + drag.dx / zoom, origin[1] + drag.dy / zoom]);
                    }
                }
                _ => {
                    if drag.cancelled()
                        && let DragState::DraggingNode { node, origin } = canvas.drag.clone()
                    {
                        canvas.move_node(&node, origin);
                    }
                    canvas.drag = DragState::Idle;
                }
            }
        });
    let rows = card.inputs.len().max(card.outputs.len());
    let body = col().w_full().py(6.0 * zoom).children((0..rows).map(|row_index| socket_row(card, row_index, palette, zoom)));
    let select_id = card.id.clone();
    let face = col()
        .key(format!("node-{}-face", card.id.0))
        .w_full()
        .h_full()
        .radius(10.0 * zoom)
        .clip_rounded(10.0 * zoom)
        .fill(palette.card)
        .child(header)
        .child(body)
        .on_press(move |s: &mut S| lens(s).select(Some(select_id.clone())));
    let ring = if card.selected {
        palette.ring
    } else if card.status == ExecutionStatus::Running {
        status_color(card.status)
    } else {
        palette.card_border
    };
    let ring_width = if card.selected || card.status == ExecutionStatus::Running { 2.0 } else { 1.0 };
    let shell = el()
        .key(format!("node-{}", card.id.0))
        .size(card.size[0] * zoom, card.size[1] * zoom)
        .radius(10.0 * zoom)
        .border(ring_width, ring)
        .shadow(6.0 * zoom, 18.0 * zoom, rgba(0.0, 0.0, 0.0, if state.light { 0.12 } else { 0.4 }))
        .absolute(x, y)
        .child(face);
    let inputs = card.inputs.iter().map(|socket| socket_port::<S>(card, socket, false, palette, zoom, lens));
    let outputs = card.outputs.iter().map(|socket| socket_port::<S>(card, socket, true, palette, zoom, lens));
    inputs.chain(outputs).fold(shell, |shell, port| shell.child(port))
}

fn socket_port<S: 'static>(
    card: &NodeCardModel,
    socket: &SocketModel,
    output: bool,
    palette: CanvasPalette,
    zoom: f32,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let side = if output { "out" } else { "in" };
    let color = socket_color(socket.ty);
    let port = el()
        .key(format!("socket-{}-{side}-{}", card.id.0, socket.name))
        .size(SOCKET_SIZE * zoom, SOCKET_SIZE * zoom)
        .radius(if socket.ty.is_flow() { 3.0 * zoom } else { SOCKET_SIZE * zoom / 2.0 })
        .fill(color)
        .border(2.0 * zoom, palette.card)
        .absolute((socket.offset[0] - SOCKET_SIZE / 2.0) * zoom, (socket.offset[1] - SOCKET_SIZE / 2.0) * zoom);
    if !output {
        return port;
    }
    let from = SocketRef::new(card.id.clone(), socket.name.clone());
    let ty = socket.ty;
    let anchor = [card.position[0] + socket.offset[0], card.position[1] + socket.offset[1]];
    port.on_drag(move |s: &mut S, drag, tx| {
        tx.instant();
        let canvas = lens(s);
        match drag.phase {
            Phase::Down => {
                let anchor = canvas.card(&from.node).and_then(|card| card.socket_world(&from.socket, true)).map_or(anchor, |(point, _)| point);
                canvas.drag = DragState::ConnectingWire { from: from.clone(), ty, anchor, cursor: anchor };
            }
            Phase::Move => {
                let zoom = canvas.camera.zoom;
                if let DragState::ConnectingWire { anchor, cursor, .. } = &mut canvas.drag {
                    *cursor = [anchor[0] + drag.dx / zoom, anchor[1] + drag.dy / zoom];
                }
            }
            _ => {
                if let DragState::ConnectingWire { from, cursor, .. } = canvas.drag.clone()
                    && !drag.cancelled()
                    && let Some(target) = canvas.input_socket_at(cursor)
                {
                    canvas.last_connect_error = canvas.connect_sockets(&from, &target).err();
                }
                canvas.drag = DragState::Idle;
            }
        }
    })
}
