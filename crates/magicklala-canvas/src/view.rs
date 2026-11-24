use magicklala_graph::SocketType;
use rune::{Color, El, Phase, Shape, TextStyle, canvas, col, el, row, text};

use crate::hearth::{
    CauldronMood, HearthPalette, StreamStyle, WireStyle, brand_cauldron_mark, category_tone, distillation_point, distillation_ribbon_shape, fill_circle,
    fill_star4, ghost_potion_bottle_shape, grimoire_wire_shape, hero_cauldron_shape, hex, mini_vial_icon, potion_bottle_shape, pouring_stream_shape,
    sparkle_connector_h, stream_ink, tabletop_circle_shape, with_alpha,
};
use crate::model::{CARD_HEADER, SOCKET_ROW};
use crate::{CanvasState, DragState, ExecutionStatus, NodeCardModel, SocketModel, SocketRef, WorkbenchMode};

const FLOW_KNOT_SIZE: f32 = 11.5;
const DATA_PIN_SIZE: f32 = 8.5;
const DOT_SPACING: f32 = 26.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasPalette {
    pub surface: Color,
    pub dot: Color,
    pub card: Color,
    pub card_header: Color,
    pub card_border: Color,
    pub card_border_idle: Color,
    pub ink: Color,
    pub muted: Color,
    pub ring: Color,
    pub header_ink: Color,
    pub thread: Color,
    pub knot: Color,
    pub rule: Color,
    pub rule_2: Color,
    pub offset_shadow: Color,
    pub offset_card: Color,
}

impl CanvasPalette {
    pub fn for_theme(light: bool) -> Self {
        let p = HearthPalette::for_theme(light);
        Self {
            surface: p.paper,
            dot: p.canvas_dot,
            card: p.card,
            card_header: p.wash_2,
            card_border: p.ink,
            card_border_idle: p.rule_2,
            ink: p.ink,
            muted: p.mute,
            ring: p.broth_mid,
            header_ink: p.ink,
            thread: p.broth_mid,
            knot: p.broth_deep,
            rule: p.rule,
            rule_2: p.rule_2,
            offset_shadow: p.shadow_soft,
            offset_card: p.shadow_card,
        }
    }
}

pub fn socket_color(ty: SocketType) -> Color {
    let p = HearthPalette::for_theme(true);
    match ty {
        SocketType::Flow => p.broth_mid,
        _ => p.ink,
    }
}

pub fn category_accent(category: &str) -> Color {
    category_tone(category, true).liquid
}

pub fn status_color(status: ExecutionStatus) -> Color {
    let p = HearthPalette::for_theme(true);
    match status {
        ExecutionStatus::Idle => p.mute,
        ExecutionStatus::Running => p.gold,
        ExecutionStatus::Succeeded => p.broth_mid,
        ExecutionStatus::Failed => p.hex,
    }
}

pub fn canvas_view(state: &CanvasState, size: (f32, f32)) -> El {
    interactive_canvas_view::<CanvasState>(state, size, |state| state)
}

pub fn interactive_canvas_view<S: 'static>(state: &CanvasState, size: (f32, f32), lens: fn(&mut S) -> &mut CanvasState) -> El {
    match state.mode {
        WorkbenchMode::Cauldron => cauldron_workbench_view(state, size, lens),
        WorkbenchMode::Grimoire => grimoire_graph_view(state, size, lens),
    }
}

fn cauldron_mood(state: &CanvasState) -> CauldronMood {
    if state.nodes.is_empty() {
        return CauldronMood::Empty;
    }
    if state.invalid_brew || state.nodes.iter().any(|card| card.status == ExecutionStatus::Failed) {
        return CauldronMood::Hexed;
    }
    if state.running_brew || state.nodes.iter().any(|card| card.status == ExecutionStatus::Running) {
        return CauldronMood::Simmering;
    }
    if state.has_run() || state.nodes.iter().any(|card| card.status == ExecutionStatus::Succeeded) {
        return CauldronMood::Brewed;
    }
    CauldronMood::Ready
}

fn is_potion_output(card: &NodeCardModel) -> bool {
    card.kind == "io/send_message" || card.category.eq_ignore_ascii_case("io")
}

fn mode_switcher<S: 'static>(state: &CanvasState, p: &HearthPalette, lens: fn(&mut S) -> &mut CanvasState) -> El {
    let is_cauldron = state.mode == WorkbenchMode::Cauldron;
    let pill = |key: &str, caption: &str, active: bool, target: WorkbenchMode| {
        row()
            .key(key)
            .h(26.0)
            .px(10.0)
            .gap(5.0)
            .radius(9.0)
            .continuous_corners()
            .items_center()
            .fill(if active { p.primary_bg } else { Color::TRANSPARENT })
            .child(text(caption).font(TextStyle::new(11.5, if active { 700.0 } else { 550.0 })).color(if active { p.primary_ink } else { p.mute }))
            .on_press(move |s: &mut S| {
                let c = lens(s);
                c.mode = target;
                c.context_menu = None;
            })
    };
    row()
        .key("workbench-mode-switch")
        .padding(3.0)
        .gap(2.0)
        .radius(12.0)
        .continuous_corners()
        .fill(p.card)
        .border(1.0, p.rule_2)
        .shadow(3.0, 10.0, p.shadow_card)
        .absolute(14.0, 12.0)
        .child(pill("mode-cauldron", "Cauldron", is_cauldron, WorkbenchMode::Cauldron))
        .child(pill("mode-grimoire", "Grimoire", !is_cauldron, WorkbenchMode::Grimoire))
}

fn cauldron_workbench_view<S: 'static>(state: &CanvasState, size: (f32, f32), lens: fn(&mut S) -> &mut CanvasState) -> El {
    let p = HearthPalette::for_theme(state.light);
    let mood = cauldron_mood(state);
    let compact = size.0 < 690.0 || size.1 < 570.0;
    let zoom = state.camera.zoom.clamp(0.65, 1.5);
    let ox = size.0 * 0.5;
    let oy = size.1 * 0.5;
    let pan_x = state.camera.pan_x;
    let pan_y = state.camera.pan_y;
    let map_pt = |pt: [f32; 2]| -> [f32; 2] { [ox + (pt[0] - ox) * zoom + pan_x, oy + (pt[1] - oy) * zoom + pan_y] };
    let outputs: Vec<&NodeCardModel> = state.nodes.iter().filter(|card| is_potion_output(card)).collect();
    let multi_outputs = outputs.len() > 1;
    let base_cx = if state.nodes.is_empty() {
        size.0 * 0.50
    } else if multi_outputs {
        size.0 * 0.47
    } else {
        size.0 * 0.52
    };
    let base_cy = if state.nodes.is_empty() {
        size.1 * 0.39
    } else if compact {
        (size.1 * 0.49).clamp(228.0, 276.0)
    } else {
        320.0
    };
    let [cx, cy] = map_pt([base_cx, base_cy]);
    let cauldron_w: f32 = (if compact { 206.0 } else { 256.0 }) * zoom;
    let cauldron_h: f32 = (if compact { 200.0 } else { 248.0 }) * zoom;
    let u = (cauldron_w.min(cauldron_h) / 240.0).clamp(0.45, 1.55);
    let rim_y = cy - cauldron_h * 0.54 + cauldron_h * 0.56 - 30.0 * u;
    let circle_r = (if compact { 140.0 } else { 172.0 }) * zoom;

    let glow_color = match mood {
        CauldronMood::Hexed => with_alpha(p.hex, if state.light { 0.12 } else { 0.18 }),
        CauldronMood::Empty => Color::TRANSPARENT,
        CauldronMood::Ready => with_alpha(p.broth_mid, 0.08),
        CauldronMood::Simmering | CauldronMood::Brewed => with_alpha(p.broth_foam, if state.light { 0.24 } else { 0.20 }),
    };
    let hearth_glow = el()
        .size(size.0, size.1)
        .radial_gradient(
            [(cx / size.0).clamp(0.0, 1.0), (cy / size.1).clamp(0.0, 1.0)],
            [circle_r * 1.45, circle_r * 1.25],
            [(0.0, glow_color), (0.65, with_alpha(glow_color, 0.04)), (1.0, Color::TRANSPARENT)],
        )
        .absolute(0.0, 0.0);

    let backdrop = canvas(apothecary_dots(state, size, &p, [cx, cy], circle_r)).key("canvas-dots").size(size.0, size.1).absolute(0.0, 0.0);

    let all_non_outputs: Vec<&NodeCardModel> = state.nodes.iter().filter(|card| !is_potion_output(card)).collect();
    let mut branch_helpers: Vec<(&NodeCardModel, magicklala_graph::NodeId)> = Vec::new();
    let mut ingredients: Vec<&NodeCardModel> = Vec::new();
    for card in &all_non_outputs {
        let outgoing: Vec<&crate::WireModel> = state.wires.iter().filter(|w| w.from_node == card.id).collect();
        let single_out_target = if multi_outputs && !outgoing.is_empty() && outgoing.iter().all(|w| outputs.iter().any(|o| o.id == w.to_node)) {
            let first = &outgoing[0].to_node;
            outgoing.iter().all(|w| &w.to_node == first).then(|| first.clone())
        } else {
            None
        };
        if let Some(target_out) = single_out_target {
            branch_helpers.push((*card, target_out));
        } else {
            ingredients.push(*card);
        }
    }
    let has_run = state.has_run();

    let vial_w = (if compact { 180.0 } else { 196.0 }) * zoom;
    let vial_h = (if compact { 50.0 } else { 58.0 }) * zoom;
    let left_x = if compact { 20.0 } else { 24.0 };
    let left_count = ingredients.len().min(4);
    let gap_y = if compact { 16.0 } else { 22.0 };
    let base_vial_h = if compact { 50.0 } else { 58.0 };
    let stack_h = if left_count > 0 { left_count as f32 * base_vial_h + (left_count - 1) as f32 * gap_y } else { 0.0 };
    let start_y = if compact {
        ((size.1 - stack_h) * 0.5 - 2.0).max(52.0)
    } else if left_count <= 3 {
        ((base_cy - stack_h * 0.5) - 6.0).max(120.0)
    } else {
        150.0
    };

    let mut placed_ingredients: Vec<(&NodeCardModel, [f32; 2])> = Vec::with_capacity(ingredients.len());
    for (idx, card) in ingredients.iter().enumerate() {
        let base_pos = if idx < 4 {
            [left_x, start_y + idx as f32 * (base_vial_h + gap_y)]
        } else {
            let extra = (idx - 4) as f32;
            [(base_cx - 98.0 + (extra - 0.5) * 210.0).clamp(16.0, size.0 - 212.0), 54.0]
        };
        placed_ingredients.push((*card, map_pt(base_pos)));
    }

    let potion_w = (if compact { 126.0 } else { 148.0 }) * zoom;
    let base_potion_w = if compact { 126.0 } else { 148.0 };
    let base_potion_x = (size.0 - base_potion_w - if compact { 16.0 } else { 22.0 }).max(base_cx + (if compact { 103.0 } else { 128.0 }) + 16.0);
    let base_potion_y = (base_cy - 78.0).max(58.0);
    let [potion_x, potion_y] = map_pt([base_potion_x, base_potion_y]);
    let sealed_vintage =
        state.release_badge.is_some() && !matches!(mood, CauldronMood::Brewed | CauldronMood::Simmering | CauldronMood::Hexed | CauldronMood::Empty);
    let bottle_neck = [potion_x + potion_w * 0.5, potion_y + (if sealed_vintage { 14.0 } else { 29.0 }) * zoom];

    let mut streams = Shape::new();
    let count_f = placed_ingredients.len().max(1) as f32;
    for (idx, (card, pos)) in placed_ingredients.iter().enumerate() {
        let tone = category_tone(&card.category, state.light);
        let from = [pos[0] + vial_w, pos[1] + vial_h * 0.5];
        let spread = if count_f > 1.0 { idx as f32 / (count_f - 1.0) } else { 0.5 };
        let target_x = cx + 12.0 * u - spread * (56.0 * u);
        let target_y = rim_y + 4.0 * u;
        let hexed = card.status == ExecutionStatus::Failed || card.inputs.iter().any(|s| s.flagged);
        let blocked_by_hex = mood == CauldronMood::Hexed && !hexed;
        let untaken = has_run && card.status == ExecutionStatus::Idle && !hexed && !blocked_by_hex;
        let active = card.status == ExecutionStatus::Succeeded || card.status == ExecutionStatus::Running;
        let idle = !active && !untaken && !blocked_by_hex && !hexed;
        let stream_color = if hexed {
            p.hex
        } else if blocked_by_hex {
            p.mute
        } else if idle {
            stream_ink(&card.category, state.light)
        } else {
            tone.liquid
        };
        let stream_glow = if hexed { p.hex } else { tone.glow };
        let arch_lift = match idx {
            0 => 56.0,
            1 => 34.0,
            2 => 14.0,
            _ => 0.0,
        };
        streams = pouring_stream_shape(
            streams,
            from,
            [target_x, target_y],
            StreamStyle { color: stream_color, glow: stream_glow, active, idle, untaken, blocked: blocked_by_hex, hexed, light: state.light, arch_lift },
        );
    }

    let dist_active = matches!(mood, CauldronMood::Brewed | CauldronMood::Simmering);
    let mut visible_outputs: Vec<&NodeCardModel> = outputs.clone();
    if visible_outputs.len() > 3 {
        visible_outputs.sort_by_key(|card| match card.status {
            ExecutionStatus::Succeeded | ExecutionStatus::Running => 0,
            ExecutionStatus::Failed => 1,
            ExecutionStatus::Idle => 2,
        });
    }
    let overflow_bottles = visible_outputs.len().saturating_sub(3);
    let base_multi_w = if compact { 126.0 } else { 150.0 };
    let multi_bottle_w = base_multi_w * zoom;
    let base_multi_x = (size.0 - base_multi_w - if compact { 14.0 } else { 20.0 }).max(base_cx + (if compact { 103.0 } else { 128.0 }) + 16.0);
    let base_multi_step = if compact { 120.0 } else { 152.0 };
    let base_multi_start = if compact { 60.0 } else { 86.0 };
    let dist_from = [cx + 66.0 * u, rim_y - 3.0 * u];
    if !state.nodes.is_empty() {
        if multi_outputs {
            for (idx, out_card) in visible_outputs.iter().take(3).enumerate() {
                let [bx, by] = map_pt([base_multi_x, base_multi_start + idx as f32 * base_multi_step]);
                let taken = out_card.status == ExecutionStatus::Succeeded || out_card.status == ExecutionStatus::Running;
                let untaken = has_run && !taken;
                let out_mood = if taken { CauldronMood::Brewed } else { CauldronMood::Ready };
                streams = distillation_ribbon_shape(streams, dist_from, [bx + multi_bottle_w * 0.5, by + 24.0 * zoom], out_mood, state.light, taken, untaken);
            }
        } else {
            streams = distillation_ribbon_shape(streams, dist_from, bottle_neck, mood, state.light, dist_active, false);
        }
    } else {
        let ghost_x = (cx + cauldron_w * 0.5 + 32.0 * zoom).min(size.0 - 144.0);
        let ghost_y = (cy - 68.0 * zoom).max(56.0);
        streams = distillation_ribbon_shape(streams, dist_from, [ghost_x + 64.0, ghost_y + 29.0], CauldronMood::Empty, state.light, false, false);
    }

    let streams_el = canvas(streams).key("canvas-wires").size(size.0, size.1).absolute(0.0, 0.0);

    let cauldron_art = canvas(hero_cauldron_shape(mood, state.light, cauldron_w, cauldron_h)).key("hero-cauldron").size(cauldron_w, cauldron_h);
    let primary_target = state.nodes.iter().find(|card| card.kind == "ai/generate_text").or_else(|| ingredients.first().copied()).map(|card| card.id.clone());
    let cauldron_box =
        el().key("cauldron-center").size(cauldron_w, cauldron_h).absolute(cx - cauldron_w * 0.5, cy - cauldron_h * 0.54).child(cauldron_art).on_press(
            move |s: &mut S| {
                if let Some(id) = primary_target.clone() {
                    lens(s).select(Some(id));
                }
            },
        );

    let total_ingredients = all_non_outputs.len();
    let taken_ingredients = all_non_outputs.iter().filter(|card| card.status == ExecutionStatus::Succeeded).count();
    let taken_outputs = outputs.iter().filter(|card| card.status == ExecutionStatus::Succeeded).count();
    let sum_ms: u64 = state.nodes.iter().filter_map(|card| card.duration_ms).sum();
    let time_part = if sum_ms == 0 { "<1ms".to_owned() } else { format!("{sum_ms}ms") };
    let plaque_text = if let Some(summary) = &state.brew_summary {
        summary.clone()
    } else if state.nodes.is_empty() {
        "Empty cauldron".to_owned()
    } else if mood == CauldronMood::Hexed {
        "Hexed · check crimson vial".to_owned()
    } else if mood == CauldronMood::Brewed && multi_outputs {
        format!("Brewed · {time_part} · {taken_ingredients} of {total_ingredients} ingredients · {taken_outputs} of {} outputs", outputs.len())
    } else if mood == CauldronMood::Brewed {
        format!("Brewed · {time_part}")
    } else if let Some(badge) = &state.release_badge {
        let word = if total_ingredients == 1 { "ingredient" } else { "ingredients" };
        format!("Published {badge} · {total_ingredients} {word}")
    } else if state.validated {
        let word = if total_ingredients == 1 { "ingredient" } else { "ingredients" };
        format!("Checked · {total_ingredients} {word}")
    } else {
        let word = if total_ingredients == 1 { "ingredient" } else { "ingredients" };
        format!("Draft · {total_ingredients} {word}")
    };
    let plaque_dot = match mood {
        CauldronMood::Hexed => p.hex,
        CauldronMood::Simmering => p.gold,
        CauldronMood::Brewed => p.broth_mid,
        CauldronMood::Ready if state.validated || state.release_badge.is_some() => p.broth_mid,
        CauldronMood::Ready => p.gold,
        CauldronMood::Empty => p.mute,
    };
    let plaque_w = 344.0;
    let plaque = row()
        .key("cauldron-plaque")
        .h(28.0)
        .px(12.0)
        .gap(7.0)
        .radius(14.0)
        .continuous_corners()
        .items_center()
        .justify_center()
        .fill(if mood == CauldronMood::Hexed { p.hex_tint } else { p.card })
        .border(1.2, if mood == CauldronMood::Hexed { p.hex } else { p.rule_2 })
        .shadow(3.0, 10.0, p.shadow_card)
        .child(el().size(7.0, 7.0).radius(3.5).fill(plaque_dot))
        .child(text(plaque_text).font(TextStyle::new(11.5, 650.0)).color(if mood == CauldronMood::Hexed { p.hex } else { p.ink }));
    let plaque_wrap = row().w(plaque_w).justify_center().absolute(cx - plaque_w * 0.5, cy + cauldron_h * 0.42).child(plaque);

    let mut hit_rects: Vec<(magicklala_graph::NodeId, [f32; 4])> =
        placed_ingredients.iter().map(|(c, pos)| (c.id.clone(), [pos[0], pos[1], vial_w, vial_h])).collect();
    if !state.nodes.is_empty() {
        if multi_outputs {
            for (idx, out_card) in visible_outputs.iter().take(3).enumerate() {
                let [bx, by] = map_pt([base_multi_x, base_multi_start + idx as f32 * base_multi_step]);
                hit_rects.push((out_card.id.clone(), [bx, by, multi_bottle_w, 128.0 * zoom]));
            }
        } else if let Some(primary) = outputs.first() {
            hit_rects.push((primary.id.clone(), [potion_x, potion_y, potion_w, 178.0 * zoom]));
        }
    }

    let mut board = el()
        .key("canvas")
        .size(size.0, size.1)
        .fill(p.paper)
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
            let hit = hit_rects
                .iter()
                .rev()
                .find_map(|(id, r)| (press.x >= r[0] && press.x <= r[0] + r[2] && press.y >= r[1] && press.y <= r[1] + r[3]).then(|| id.clone()));
            if hit.is_some() {
                canvas.select(hit);
            }
            canvas.context_menu = Some([press.x, press.y]);
        })
        .child(hearth_glow)
        .child(backdrop)
        .child(streams_el)
        .child(cauldron_box);

    if !state.nodes.is_empty() {
        board = board.child(plaque_wrap);
    }

    for (card, pos) in placed_ingredients {
        let step_num = if has_run { card.step_order } else { None };
        board = board.child(ingredient_vial_card(state, card, step_num, pos, [vial_w, vial_h], &p, lens));
    }

    if !state.nodes.is_empty() {
        if multi_outputs {
            for (idx, out_card) in visible_outputs.iter().take(3).enumerate() {
                let [bx, by] = map_pt([base_multi_x, base_multi_start + idx as f32 * base_multi_step]);
                let bottle_target = [bx + multi_bottle_w * 0.5, by + 24.0 * zoom];
                let step = if has_run { out_card.step_order } else { None };
                let route_name = state
                    .wires
                    .iter()
                    .find(|w| w.to_node == out_card.id && w.ty.is_flow())
                    .map(|w| w.from_socket.clone())
                    .unwrap_or_else(|| out_card.id.0.strip_suffix("_reply").unwrap_or(&out_card.id.0).to_owned());
                let taken = out_card.status == ExecutionStatus::Succeeded || out_card.status == ExecutionStatus::Running;
                let pill_t = match idx {
                    0 => 0.44,
                    1 => 0.48,
                    _ => 0.36,
                };
                let curve_pt = distillation_point(dist_from, bottle_target, pill_t);
                let tag_x = match idx {
                    0 | 1 => (curve_pt[0] - 26.0).clamp(cx + 46.0 * u, bx - 48.0),
                    _ => (curve_pt[0] + 10.0).clamp(cx + 88.0 * u, bx - 54.0),
                };
                let tag_y = match idx {
                    0 => (curve_pt[1] - 24.0).max(58.0),
                    1 => curve_pt[1] - 24.0,
                    _ => curve_pt[1] - 18.0,
                };
                let route_pill = row()
                    .px(6.5)
                    .py(1.5)
                    .radius(7.0)
                    .fill(if taken { if state.light { hex(0xE6F6EF) } else { with_alpha(p.broth_mid, 0.24) } } else { p.card })
                    .border(1.0, if taken { p.broth_mid } else { p.rule_2 })
                    .absolute(tag_x, tag_y)
                    .child(text(route_name).font(TextStyle::new(10.0, if taken { 700.0 } else { 600.0 }).mono()).color(if taken {
                        if state.light { p.broth_deep } else { p.broth_foam }
                    } else {
                        p.ink_2
                    }));
                board = board.child(route_pill);
                if let Some((helper, _)) = branch_helpers.iter().find(|(_, target)| *target == out_card.id) {
                    let helper_pt = distillation_point(dist_from, bottle_target, 0.68);
                    let hw = 128.0 * zoom;
                    let hh = 42.0 * zoom;
                    let hx = (bx - hw - 14.0).max(cx + 96.0 * u);
                    let hy = (helper_pt[1] + 22.0).max(cy + cauldron_h * 0.42 + 44.0).min(size.1 - hh - 38.0);
                    let tone = category_tone(&helper.category, state.light);
                    let helper_untaken = has_run && helper.status == ExecutionStatus::Idle;
                    let h_bg = if helper_untaken { if state.light { hex(0xF1EFF6) } else { hex(0x1E192D) } } else { p.card };
                    let h_border = if helper.selected {
                        tone.liquid
                    } else if helper_untaken {
                        if state.light { hex(0x9890AA) } else { hex(0x5E547D) }
                    } else {
                        tone.border
                    };
                    let h_title = if helper_untaken { if state.light { hex(0x3A3248) } else { hex(0xDFD9F0) } } else { p.ink };
                    let h_sub = if helper_untaken { if state.light { hex(0x524963) } else { hex(0xC2B9D9) } } else { p.mute };
                    let hid = helper.id.clone();
                    let chip = row()
                        .key(format!("node-{}-header", helper.id.0))
                        .size(hw, hh)
                        .px(7.0)
                        .gap(6.0)
                        .radius(14.0)
                        .continuous_corners()
                        .items_center()
                        .fill(h_bg)
                        .border(if helper.selected { 1.8 } else { 1.1 }, h_border)
                        .child(
                            el().size(26.0 * zoom, 26.0 * zoom)
                                .radius(13.0 * zoom)
                                .fill(tone.tint)
                                .border(1.0, tone.border)
                                .center()
                                .child(canvas(mini_vial_icon(&helper.category, state.light, 20.0 * zoom)).size(20.0 * zoom, 20.0 * zoom)),
                        )
                        .child(
                            col()
                                .grow(1.0)
                                .min_w(0.0)
                                .gap(1.0)
                                .child(text(&helper.title).font(TextStyle::new(10.8, 750.0)).color(h_title).truncate())
                                .child(text(&helper.kind).font(TextStyle::new(8.8, 500.0).mono()).color(h_sub).truncate()),
                        )
                        .on_press(move |s: &mut S| lens(s).select(Some(hid.clone())));
                    let mut chip_shell = el().key(format!("node-{}", helper.id.0)).size(hw, hh).absolute(hx, hy).child(chip);
                    if helper_untaken {
                        let u_badge = row()
                            .px(5.0)
                            .py(1.0)
                            .radius(6.5)
                            .fill(if state.light { hex(0xF5F2FA) } else { hex(0x2B243F) })
                            .border(1.0, h_border)
                            .child(text("Untaken").font(TextStyle::new(8.8, 700.0)).color(h_title));
                        chip_shell = chip_shell.child(row().w(hw).px(6.0).justify_end().absolute(0.0, -10.0).child(u_badge));
                    }
                    board = board.child(chip_shell);
                }
                board = board.child(branch_bottle_view(state, out_card, step, [bx, by, multi_bottle_w], has_run, &p, lens));
            }
            if overflow_bottles > 0 {
                let [ox_over, over_y] = map_pt([base_multi_x, base_multi_start + 3.0 * base_multi_step - 6.0]);
                let over_label = if overflow_bottles == 1 { "+1 more bottle".to_owned() } else { format!("+{overflow_bottles} more bottles") };
                board = board.child(
                    row()
                        .w(multi_bottle_w)
                        .h(26.0)
                        .radius(9.0)
                        .items_center()
                        .justify_center()
                        .fill(p.card)
                        .border(1.0, p.rule_2)
                        .absolute(ox_over, over_y)
                        .child(text(over_label).font(TextStyle::new(10.5, 650.0)).color(p.ink_2))
                        .on_press(move |s: &mut S| {
                            lens(s).mode = WorkbenchMode::Grimoire;
                        }),
                );
            }
        } else {
            let bottle_step = outputs.iter().find_map(|c| c.step_order);
            board = board.child(potion_stand_view(state, &outputs, bottle_step, [potion_x, potion_y, potion_w], mood, &p, lens));
        }
    } else {
        let ghost_x = (cx + cauldron_w * 0.5 + 32.0 * zoom).min(size.0 - 144.0);
        let ghost_y = (cy - 68.0 * zoom).max(56.0);
        let ghost = col()
            .w(128.0)
            .gap(4.0)
            .items_center()
            .absolute(ghost_x, ghost_y)
            .child(canvas(ghost_potion_bottle_shape(state.light, 128.0, 96.0)).size(128.0, 96.0))
            .child(text("Distills here").font(TextStyle::new(10.5, 600.0)).color(p.mute));
        board = board.child(ghost);
    }

    board = board.child(mode_switcher(state, &p, lens));

    if !state.nodes.is_empty() && size.0 >= 420.0 && size.1 >= 260.0 {
        let mut dot_colors: Vec<Color> = Vec::new();
        for card in &ingredients {
            let col = stream_ink(&card.category, state.light);
            if !dot_colors.contains(&col) && dot_colors.len() < 4 {
                dot_colors.push(col);
            }
        }
        if dot_colors.is_empty() {
            dot_colors.push(p.broth_mid);
        }
        let mut legend = row()
            .key("canvas-legend")
            .gap(12.0)
            .px(11.0)
            .py(6.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .fill(p.card)
            .border(1.0, p.rule)
            .shadow(2.0, 8.0, p.shadow_card)
            .absolute(14.0, (size.1 - 38.0).max(12.0));
        if mood != CauldronMood::Hexed {
            let swatch = if dist_active {
                row()
                    .size(14.0, 14.0)
                    .radius(7.0)
                    .items_center()
                    .justify_center()
                    .fill(p.primary_bg)
                    .child(text("1").font(TextStyle::new(8.8, 750.0).features("'tnum' 1")).color(p.primary_ink))
            } else {
                dot_colors.into_iter().fold(row().gap(2.5).items_center(), |r, c| r.child(el().w(5.0).h(2.0).radius(1.0).fill(c)))
            };
            legend = legend.child(
                row().gap(6.0).items_center().child(swatch).child(
                    text(if dist_active { format!("Run order 1–{}", taken_ingredients + taken_outputs) } else { "Ingredient streams".to_owned() })
                        .font(TextStyle::new(10.5, 600.0))
                        .color(p.ink),
                ),
            );
        }
        legend = legend.child(if mood == CauldronMood::Simmering {
            row()
                .gap(6.0)
                .items_center()
                .child(canvas(sparkle_connector_h(16.0, 6.0, state.light)).size(16.0, 6.0))
                .child(text("Distilling").font(TextStyle::new(10.5, 600.0)).color(p.ink))
        } else if mood == CauldronMood::Brewed {
            row()
                .gap(6.0)
                .items_center()
                .child(canvas(sparkle_connector_h(16.0, 6.0, state.light)).size(16.0, 6.0))
                .child(text("Distilled").font(TextStyle::new(10.5, 600.0)).color(p.ink))
        } else if mood == CauldronMood::Hexed {
            let blocked_col = if state.light { hex(0x655B78) } else { hex(0x9A8FB8) };
            row()
                .gap(3.5)
                .items_center()
                .child(el().size(3.0, 3.0).radius(1.5).fill(blocked_col))
                .child(el().size(3.0, 3.0).radius(1.5).fill(blocked_col))
                .child(el().size(3.0, 3.0).radius(1.5).fill(blocked_col))
                .child(text("Blocked").font(TextStyle::new(10.5, 600.0)).color(p.ink_2))
        } else {
            let dist_col = if state.light { hex(0x137B60) } else { hex(0x3AD6A6) };
            row()
                .gap(5.0)
                .items_center()
                .child(el().w(5.0).h(2.0).fill(dist_col))
                .child(el().w(5.0).h(2.0).fill(dist_col))
                .child(text("Distillation path").font(TextStyle::new(10.5, 550.0)).color(p.ink_2))
        });
        if mood == CauldronMood::Hexed {
            legend = legend.child(
                row()
                    .gap(5.0)
                    .items_center()
                    .child(el().w(6.0).h(2.0).fill(p.hex))
                    .child(el().w(6.0).h(2.0).fill(p.hex))
                    .child(text("Hexed").font(TextStyle::new(10.5, 650.0)).color(p.hex)),
            );
        } else if multi_outputs && has_run {
            let untaken_col = if state.light { hex(0x6D637E) } else { hex(0x9489B0) };
            legend = legend.child(
                row()
                    .gap(5.0)
                    .items_center()
                    .child(el().w(5.0).h(2.0).fill(untaken_col))
                    .child(el().w(5.0).h(2.0).fill(untaken_col))
                    .child(text("Untaken").font(TextStyle::new(10.5, 600.0)).color(p.ink_2)),
            );
        }
        board = board.child(legend);
    }

    board
}

fn ingredient_vial_card<S: 'static>(
    state: &CanvasState,
    card: &NodeCardModel,
    step_num: Option<usize>,
    pos: [f32; 2],
    dims: [f32; 2],
    p: &HearthPalette,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let tone = category_tone(&card.category, state.light);
    let hexed = card.status == ExecutionStatus::Failed || card.inputs.iter().any(|s| s.flagged);
    let dimmed = state.has_run() && card.status == ExecutionStatus::Idle && !card.selected && !hexed;
    let untaken_bg = if state.light { hex(0xF1EFF6) } else { hex(0x1E192D) };
    let untaken_border = if state.light { hex(0x9890AA) } else { hex(0x5E547D) };
    let untaken_title = if state.light { hex(0x3A3248) } else { hex(0xDFD9F0) };
    let untaken_sub = if state.light { hex(0x524963) } else { hex(0xC2B9D9) };
    let border_col = if hexed {
        p.hex
    } else if card.selected {
        tone.liquid
    } else if dimmed {
        untaken_border
    } else {
        tone.border
    };
    let border_w = if hexed || card.selected { 2.0 } else { 1.3 };
    let fill_col = if hexed {
        p.hex_tint
    } else if card.selected {
        tone.tint
    } else if dimmed {
        untaken_bg
    } else {
        p.card
    };
    let node_id = card.id.clone();
    let select_id = card.id.clone();

    let vial_badge = el()
        .size(38.0, 38.0)
        .radius(19.0)
        .fill(if hexed { with_alpha(p.hex, 0.14) } else { tone.tint })
        .border(1.3, if hexed { p.hex } else { tone.border })
        .center()
        .child(canvas(mini_vial_icon(&card.category, state.light, 29.0)).size(29.0, 29.0));

    let step_badge = step_num.map(|num| {
        row()
            .size(17.0, 17.0)
            .radius(8.5)
            .items_center()
            .justify_center()
            .fill(if hexed {
                p.hex
            } else if card.status == ExecutionStatus::Succeeded {
                p.primary_bg
            } else {
                p.wash_2
            })
            .border(
                1.0,
                if hexed {
                    p.hex
                } else if card.status == ExecutionStatus::Succeeded {
                    p.primary_bg
                } else {
                    p.rule_2
                },
            )
            .child(text(num.to_string()).font(TextStyle::new(9.2, 750.0).features("'tnum' 1")).color(if hexed || card.status == ExecutionStatus::Succeeded {
                p.primary_ink
            } else {
                p.ink_2
            }))
    });

    let status_pill = if hexed {
        let missing_label = card.inputs.iter().find(|s| s.flagged).map_or_else(|| "Hexed".to_owned(), |s| format!("Missing \"{}\"", s.name));
        Some(
            row()
                .px(6.0)
                .py(1.5)
                .radius(7.0)
                .fill(p.card)
                .border(1.2, p.hex)
                .shadow(2.0, 6.0, p.shadow_card)
                .child(text(missing_label).font(TextStyle::new(9.2, 750.0)).color(p.hex)),
        )
    } else if card.status == ExecutionStatus::Succeeded {
        let ms_text = match card.duration_ms {
            Some(0) => "✓ <1ms".to_owned(),
            Some(ms) => format!("✓ {ms}ms"),
            None => "✓".to_owned(),
        };
        Some(
            row()
                .px(5.5)
                .py(1.5)
                .radius(7.0)
                .fill(p.card)
                .border(1.0, p.broth_mid)
                .shadow(2.0, 6.0, p.shadow_card)
                .child(text(ms_text).font(TextStyle::new(9.5, 700.0).features("'tnum' 1")).color(if state.light { p.broth_deep } else { p.broth_foam })),
        )
    } else if card.status == ExecutionStatus::Running {
        Some(row().px(6.0).py(1.5).radius(7.0).fill(p.card).border(1.0, p.gold).child(text("Simmering").font(TextStyle::new(9.2, 750.0)).color(p.gold)))
    } else if dimmed {
        let untaken_label = state
            .wires
            .iter()
            .find(|w| w.from_node == card.id)
            .and_then(|w| state.wires.iter().find(|fw| fw.to_node == w.to_node && fw.ty.is_flow()))
            .map_or_else(|| "Untaken".to_owned(), |fw| format!("Untaken · {}", fw.from_socket));
        Some(
            row()
                .px(5.5)
                .py(1.5)
                .radius(7.0)
                .fill(if state.light { hex(0xF5F2FA) } else { hex(0x2B243F) })
                .border(1.0, if state.light { hex(0x9890AA) } else { hex(0x5E547D) })
                .child(text(untaken_label).font(TextStyle::new(9.2, 700.0)).color(if state.light { hex(0x322A42) } else { hex(0xDFD8F2) })),
        )
    } else {
        None
    };

    let top_row =
        row().w_full().items_center().child(text(&card.title).font(TextStyle::new(12.4, 750.0)).color(if dimmed { untaken_title } else { p.ink }).truncate());

    let sub_color = if hexed {
        p.hex
    } else if dimmed {
        untaken_sub
    } else {
        p.mute
    };

    let content = row()
        .key(format!("node-{}-header", card.id.0))
        .size(dims[0], dims[1])
        .px(9.0)
        .gap(9.0)
        .radius(24.0)
        .continuous_corners()
        .items_center()
        .fill(fill_col)
        .border(border_w, border_col)
        .shadow(if card.selected { 5.0 } else { 3.0 }, if card.selected { 14.0 } else { 9.0 }, if dimmed { Color::TRANSPARENT } else { p.shadow_card })
        .child(vial_badge)
        .child(col().grow(1.0).min_w(0.0).gap(2.0).child(top_row).child(text(&card.kind).font(TextStyle::new(9.8, 500.0).mono()).color(sub_color).truncate()))
        .on_press(move |s: &mut S| lens(s).select(Some(select_id.clone())))
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

    let mut shell = el().key(format!("node-{}", card.id.0)).size(dims[0], dims[1]).absolute(pos[0], pos[1]).child(content);
    if let Some(badge) = step_badge {
        shell = shell.child(el().absolute(-5.0, -6.0).child(badge));
    }
    if let Some(pill) = status_pill {
        shell = shell.child(row().w(dims[0]).px(10.0).justify_end().absolute(0.0, -12.0).child(pill));
    }
    shell
}

fn branch_bottle_view<S: 'static>(
    state: &CanvasState,
    card: &NodeCardModel,
    step_num: Option<usize>,
    frame: [f32; 3],
    has_run: bool,
    p: &HearthPalette,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let [pos_x, pos_y, width] = frame;
    let taken = card.status == ExecutionStatus::Succeeded || card.status == ExecutionStatus::Running;
    let untaken = has_run && !taken;
    let bottle_mood = if taken {
        CauldronMood::Brewed
    } else if untaken {
        CauldronMood::Hexed
    } else {
        CauldronMood::Ready
    };
    let bottle_h = 66.0;
    let branch = card.id.0.strip_suffix("_reply").unwrap_or(&card.id.0);
    let short = card.title.split_whitespace().next().unwrap_or(&card.title);
    let title = format!("{short} · {branch}");
    let bottle_art = canvas(potion_bottle_shape(bottle_mood, state.light, width, bottle_h, false)).size(width, bottle_h);
    let pill = if taken {
        let ms_text = match card.duration_ms {
            Some(0) => "✓ <1ms".to_owned(),
            Some(ms) => format!("✓ {ms}ms"),
            None => "✓".to_owned(),
        };
        row()
            .px(5.5)
            .py(1.5)
            .radius(7.0)
            .fill(p.card)
            .border(1.0, p.broth_mid)
            .shadow(2.0, 6.0, p.shadow_card)
            .child(text(ms_text).font(TextStyle::new(9.5, 700.0).features("'tnum' 1")).color(if state.light { p.broth_deep } else { p.broth_foam }))
    } else if untaken {
        row()
            .px(5.5)
            .py(1.5)
            .radius(7.0)
            .fill(if state.light { hex(0xF6F4FA) } else { hex(0x2B243F) })
            .border(1.0, if state.light { hex(0x9890AA) } else { hex(0x5E547D) })
            .child(text("Untaken").font(TextStyle::new(9.2, 700.0)).color(if state.light { hex(0x3A3248) } else { hex(0xDFD9F0) }))
    } else {
        row().px(5.5).py(1.5).radius(7.0).fill(p.wash_2).border(1.0, p.rule_2).child(text("Branch").font(TextStyle::new(9.2, 650.0)).color(p.ink_2))
    };
    let card_bg = if untaken { if state.light { hex(0xF1EFF6) } else { hex(0x1E192D) } } else { p.card };
    let card_border = if card.selected || taken {
        p.broth_mid
    } else if untaken {
        if state.light { hex(0x9890AA) } else { hex(0x5E547D) }
    } else {
        p.rule_2
    };
    let title_col = if untaken { if state.light { hex(0x3A3248) } else { hex(0xDFD9F0) } } else { p.ink };
    let sub_col = if untaken { if state.light { hex(0x524963) } else { hex(0xC2B9D9) } } else { p.mute };
    let tag_inner = col()
        .w_full()
        .px(10.0)
        .py(8.0)
        .gap(2.0)
        .radius(12.0)
        .continuous_corners()
        .fill(card_bg)
        .border(if taken || card.selected { 1.6 } else { 1.1 }, card_border)
        .shadow(2.0, 8.0, if untaken { Color::TRANSPARENT } else { p.shadow_card })
        .child(text(title).font(TextStyle::new(11.4, 750.0)).color(title_col).truncate())
        .child(text(&card.kind).font(TextStyle::new(9.5, 500.0).mono()).color(sub_col).truncate());
    let mut tag_wrap = el().w_full().child(tag_inner).child(row().w_full().px(8.0).justify_end().absolute(0.0, -12.0).child(pill));
    if let Some(num) = step_num {
        let step_badge = row()
            .size(17.0, 17.0)
            .radius(8.5)
            .items_center()
            .justify_center()
            .fill(p.primary_bg)
            .child(text(num.to_string()).font(TextStyle::new(9.2, 750.0).features("'tnum' 1")).color(p.primary_ink));
        tag_wrap = tag_wrap.child(el().absolute(-6.0, -7.0).child(step_badge));
    }
    let tag = col().w_full().child(tag_wrap);
    let select_id = card.id.clone();
    col()
        .key(format!("node-{}-header", card.id.0))
        .w(width)
        .gap(12.0)
        .items_center()
        .absolute(pos_x, pos_y)
        .child(bottle_art)
        .child(tag)
        .on_press(move |s: &mut S| lens(s).select(Some(select_id.clone())))
}

fn potion_stand_view<S: 'static>(
    state: &CanvasState,
    outputs: &[&NodeCardModel],
    step_num: Option<usize>,
    frame: [f32; 3],
    mood: CauldronMood,
    p: &HearthPalette,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let [pos_x, pos_y, width] = frame;
    let pos = [pos_x, pos_y];
    let primary_output = outputs.iter().find(|c| c.status == ExecutionStatus::Succeeded).or_else(|| outputs.first()).copied();
    let bottled = state.release_badge.is_some();
    let sealed_vintage = bottled && !matches!(mood, CauldronMood::Brewed | CauldronMood::Simmering | CauldronMood::Hexed | CauldronMood::Empty);
    let bottle_h = 104.0;
    let bottle_art = canvas(potion_bottle_shape(mood, state.light, width, bottle_h, bottled)).key("hero-potion-bottle").size(width, bottle_h);
    let spell_label = if state.spell_name.is_empty() { "Potion" } else { state.spell_name.as_str() };
    let out_title = primary_output.map_or("Bottled Potion", |c| c.title.as_str());
    let out_kind = primary_output.map_or("io/send_message", |c| c.kind.as_str());
    let selected = primary_output.is_some_and(|c| c.selected);
    let select_id = primary_output.map(|c| c.id.clone());

    let out_brewed = primary_output.is_some_and(|c| c.status == ExecutionStatus::Succeeded);
    let mut header_row =
        row().w_full().gap(4.0).items_center().justify_between().child(text(spell_label).font(TextStyle::new(12.0, 750.0)).color(p.ink).truncate());
    if sealed_vintage && let Some(vintage) = &state.release_badge {
        header_row = header_row.child(
            row()
                .px(5.0)
                .py(1.0)
                .radius(5.5)
                .fill(p.primary_bg)
                .child(text(vintage).font(TextStyle::new(9.5, 700.0).features("'tnum' 1")).color(p.primary_ink)),
        );
    } else if out_brewed {
        header_row = header_row.child(
            row()
                .px(5.0)
                .py(1.0)
                .radius(5.5)
                .fill(if state.light { hex(0xE6F6EF) } else { with_alpha(p.broth_mid, 0.24) })
                .border(1.0, p.broth_mid)
                .child(text("Test").font(TextStyle::new(9.2, 700.0)).color(if state.light { p.broth_deep } else { p.broth_foam })),
        );
    }
    let mut tag_card = col()
        .w_full()
        .px(10.0)
        .py(9.0)
        .gap(2.0)
        .radius(14.0)
        .continuous_corners()
        .fill(p.card)
        .border(if selected { 2.0 } else { 1.2 }, if selected { p.broth_mid } else { p.rule_2 })
        .shadow(3.0, 10.0, p.shadow_card)
        .child(header_row)
        .child(text(out_title).font(TextStyle::new(11.0, 600.0)).color(p.ink_2).truncate())
        .child(text(out_kind).font(TextStyle::new(9.5, 500.0).mono()).color(p.mute).truncate());
    if out_brewed {
        let ms_text = match primary_output.and_then(|c| c.duration_ms) {
            Some(0) => "✓ <1ms".to_owned(),
            Some(ms) => format!("✓ {ms}ms"),
            None => "✓".to_owned(),
        };
        let timing_pill = row()
            .px(5.5)
            .py(1.5)
            .radius(7.0)
            .fill(p.card)
            .border(1.0, p.broth_mid)
            .shadow(2.0, 6.0, p.shadow_card)
            .child(text(ms_text).font(TextStyle::new(9.5, 700.0).features("'tnum' 1")).color(if state.light { p.broth_deep } else { p.broth_foam }));
        let mut wrapped = el().w_full().child(tag_card).child(row().w_full().px(8.0).justify_end().absolute(0.0, -12.0).child(timing_pill));
        if let Some(num) = step_num {
            let step_badge = row()
                .size(17.0, 17.0)
                .radius(8.5)
                .items_center()
                .justify_center()
                .fill(p.primary_bg)
                .child(text(num.to_string()).font(TextStyle::new(9.2, 750.0).features("'tnum' 1")).color(p.primary_ink));
            wrapped = wrapped.child(el().absolute(-5.0, -7.0).child(step_badge));
        }
        tag_card = col().w_full().child(wrapped);
    }

    let mut stand = col().key("potion-stand").w(width).gap(4.0).items_center().absolute(pos[0], pos[1]).child(bottle_art).child(tag_card);

    if let Some(id) = select_id {
        let header_key = format!("node-{}-header", id.0);
        let drag_id = id.clone();
        stand = stand.key(header_key).on_press(move |s: &mut S| lens(s).select(Some(id.clone()))).on_drag(move |s: &mut S, drag, tx| {
            tx.instant();
            let canvas = lens(s);
            match drag.phase {
                Phase::Down => {
                    canvas.select(Some(drag_id.clone()));
                    let origin = canvas.card(&drag_id).map_or([0.0, 0.0], |card| card.position);
                    canvas.drag = DragState::DraggingNode { node: drag_id.clone(), origin };
                }
                Phase::Move => {
                    if let DragState::DraggingNode { node, origin } = canvas.drag.clone() {
                        let zoom = canvas.camera.zoom;
                        canvas.move_node(&node, [origin[0] + drag.dx / zoom, origin[1] + drag.dy / zoom]);
                    }
                }
                _ => canvas.drag = DragState::Idle,
            }
        });
    }
    stand
}

fn apothecary_dots(state: &CanvasState, size: (f32, f32), p: &HearthPalette, center: [f32; 2], circle_r: f32) -> Shape {
    let spacing = DOT_SPACING;
    let columns = (size.0 / spacing).ceil() as usize + 1;
    let rows = (size.1 / spacing).ceil() as usize + 1;
    let mut shape = Shape::new();
    for r in 0..rows {
        for c in 0..columns {
            let x = 13.0 + c as f32 * spacing;
            let y = 13.0 + r as f32 * spacing;
            if (r * 7 + c * 13) % 89 == 0 {
                shape = fill_star4(shape, x, y, 2.3, 0.75, with_alpha(p.gold, if state.light { 0.28 } else { 0.30 }));
            } else {
                shape = fill_circle(shape, x, y, 1.0, p.canvas_dot);
            }
        }
    }
    let circle = tabletop_circle_shape(center, circle_r, state.light);
    let _ = circle;
    let outer_segs = 56;
    for i in (0..outer_segs).step_by(2) {
        let a0 = (i as f32 / outer_segs as f32) * std::f32::consts::TAU;
        let a1 = ((i as f32 + 1.1) / outer_segs as f32) * std::f32::consts::TAU;
        shape = shape.line(
            [center[0] + a0.cos() * circle_r, center[1] + a0.sin() * circle_r],
            [center[0] + a1.cos() * circle_r, center[1] + a1.sin() * circle_r],
            1.1,
            rune::LineCap::Round,
            p.chalk,
        );
    }
    let inner = circle_r * 0.72;
    for i in 0..44 {
        let a = (i as f32 / 44.0) * std::f32::consts::TAU;
        shape = fill_circle(shape, center[0] + a.cos() * inner, center[1] + a.sin() * inner, 1.0, with_alpha(p.chalk, 0.72));
    }
    for k in 0..4 {
        let a = (k as f32 / 4.0) * std::f32::consts::TAU + std::f32::consts::FRAC_PI_4;
        let r = circle_r + 10.0;
        shape = fill_star4(shape, center[0] + a.cos() * r, center[1] + a.sin() * r, 3.8, 1.15, with_alpha(p.gold, if state.light { 0.42 } else { 0.50 }));
    }
    shape
}

fn grimoire_graph_view<S: 'static>(state: &CanvasState, size: (f32, f32), lens: fn(&mut S) -> &mut CanvasState) -> El {
    let p = HearthPalette::for_theme(state.light);
    let palette = CanvasPalette::for_theme(state.light);
    let backdrop = canvas(dot_matrix(state, size, &p)).key("canvas-dots").size(size.0, size.1).absolute(0.0, 0.0);
    let wires = canvas(wire_shape(state)).key("canvas-wires").size(size.0, size.1).absolute(0.0, 0.0);
    let board = el()
        .key("canvas")
        .size(size.0, size.1)
        .fill(p.paper)
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

    let mut board = state.nodes.iter().fold(board, |board, card| board.child(grimoire_card_view(state, card, &p, palette, lens)));
    board = board.child(mode_switcher(state, &p, lens));

    if !state.nodes.is_empty() && size.0 >= 420.0 && size.1 >= 260.0 {
        let has_run = state.has_run();
        let mut legend = row()
            .key("canvas-legend")
            .gap(12.0)
            .px(11.0)
            .py(6.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .fill(p.card)
            .border(1.0, p.rule)
            .shadow(2.0, 8.0, p.shadow_card)
            .absolute(14.0, (size.1 - 38.0).max(12.0))
            .child(
                row()
                    .gap(6.0)
                    .items_center()
                    .child(canvas(sparkle_connector_h(18.0, 6.0, state.light)).size(18.0, 6.0))
                    .child(text(if has_run { "Flow used" } else { "Flow" }).font(TextStyle::new(10.5, 600.0)).color(p.ink)),
            )
            .child(
                row()
                    .gap(6.0)
                    .items_center()
                    .child(el().w(14.0).h(if has_run { 2.3 } else { 1.5 }).fill(if has_run {
                        if state.light { hex(0x462A52) } else { hex(0xE8E2F5) }
                    } else {
                        p.data_wire
                    }))
                    .child(text(if has_run { "Data used" } else { "Data" }).font(TextStyle::new(10.5, 550.0)).color(p.ink_2)),
            );
        if has_run {
            let dash_col = if state.light { hex(0x625574) } else { hex(0xBDB3DC) };
            let untaken_ink = if state.light { hex(0x362E45) } else { hex(0xDDD6F0) };
            legend = legend
                .child(
                    row()
                        .gap(4.5)
                        .items_center()
                        .child(el().w(5.5).h(1.8).fill(dash_col))
                        .child(el().w(5.5).h(1.8).fill(dash_col))
                        .child(text("Untaken flow").font(TextStyle::new(10.5, 600.0)).color(untaken_ink)),
                )
                .child(
                    row()
                        .gap(3.0)
                        .items_center()
                        .child(el().size(2.6, 2.6).radius(1.3).fill(dash_col))
                        .child(el().size(2.6, 2.6).radius(1.3).fill(dash_col))
                        .child(el().size(2.6, 2.6).radius(1.3).fill(dash_col))
                        .child(text("Untaken data").font(TextStyle::new(10.5, 600.0)).color(untaken_ink)),
                );
        }
        board = board.child(legend);
    }
    board
}

fn dot_matrix(state: &CanvasState, size: (f32, f32), p: &HearthPalette) -> Shape {
    let spacing = (DOT_SPACING * state.camera.zoom).max(14.0);
    let radius = (1.05 * state.camera.zoom).clamp(0.75, 1.45);
    let start_x = state.camera.pan_x.rem_euclid(spacing);
    let start_y = state.camera.pan_y.rem_euclid(spacing);
    let columns = (size.0 / spacing).ceil() as usize + 1;
    let rows = (size.1 / spacing).ceil() as usize + 1;
    (0..rows).flat_map(|row| (0..columns).map(move |column| (row, column))).fold(Shape::new(), |shape, (row, column)| {
        let x = start_x + column as f32 * spacing;
        let y = start_y + row as f32 * spacing;
        fill_circle(shape, x, y, radius, p.canvas_dot)
    })
}

fn wire_shape(state: &CanvasState) -> Shape {
    let zoom = state.camera.zoom;
    let has_run = state.has_run();
    let mut ordered: Vec<(usize, &crate::WireModel)> = state.wires.iter().enumerate().collect();
    ordered.sort_by_key(|(_, wire)| {
        let untaken = has_run && !wire.active;
        match (untaken, wire.ty.is_flow()) {
            (true, false) => 0,
            (true, true) => 1,
            (false, false) => 2,
            (false, true) => 3,
        }
    });
    let mut untaken_flow_count = 0_i8;
    let shape = ordered.into_iter().fold(Shape::new(), |shape, (_, wire)| {
        let from = state.camera.world_to_screen(wire.from);
        let to = state.camera.world_to_screen(wire.to);
        let untaken = has_run && !wire.active;
        let lane = if untaken && wire.ty.is_flow() {
            let idx = untaken_flow_count;
            untaken_flow_count += 1;
            if idx % 2 == 0 { 1 } else { -1 }
        } else {
            0
        };
        grimoire_wire_shape(shape, from, to, WireStyle { zoom, is_flow: wire.ty.is_flow(), active: wire.active, untaken, light: state.light, lane })
    });
    match &state.drag {
        DragState::ConnectingWire { ty, anchor, cursor, .. } => {
            let from = state.camera.world_to_screen(*anchor);
            let to = state.camera.world_to_screen(*cursor);
            grimoire_wire_shape(shape, from, to, WireStyle { zoom, is_flow: ty.is_flow(), active: true, untaken: false, light: state.light, lane: 0 })
        }
        _ => shape,
    }
}

fn status_box(status: ExecutionStatus, duration_ms: Option<u64>, p: &HearthPalette, zoom: f32) -> Option<El> {
    if status == ExecutionStatus::Idle {
        return None;
    }
    let (border, ink) = match status {
        ExecutionStatus::Failed => (p.hex, p.hex),
        ExecutionStatus::Running => (p.gold, p.gold),
        ExecutionStatus::Succeeded => (p.broth_mid, if p.light { p.broth_deep } else { p.broth_foam }),
        ExecutionStatus::Idle => return None,
    };
    let text_label = match (status, duration_ms) {
        (ExecutionStatus::Succeeded, Some(0)) => "✓ <1ms".to_owned(),
        (ExecutionStatus::Succeeded, Some(ms)) => format!("✓ {ms}ms"),
        _ => status.display_label().to_owned(),
    };
    Some(
        row()
            .gap(4.0 * zoom)
            .items_center()
            .px((6.0 * zoom).max(5.5))
            .py((1.8 * zoom).max(1.5))
            .radius((6.5 * zoom).max(6.0))
            .fill(p.card)
            .border((1.0 * zoom).max(1.0), border)
            .shadow(2.0, 5.0, p.shadow_card)
            .child(text(text_label).font(TextStyle::new((11.2 * zoom).clamp(11.0, 12.2), 750.0).features("'tnum' 1")).color(ink)),
    )
}

fn socket_row(card: &NodeCardModel, row_index: usize, dimmed: bool, p: &HearthPalette, zoom: f32) -> El {
    let label = |socket: Option<&SocketModel>| {
        let (content, weight, color) = match socket {
            Some(socket) if socket.flagged => (socket.name.as_str(), 700.0, p.hex),
            Some(socket) if socket.ty.is_flow() => {
                (socket.name.as_str(), 650.0, if dimmed { if p.light { hex(0x362E45) } else { hex(0xDED7F2) } } else { p.ink })
            }
            Some(socket) => (socket.name.as_str(), 500.0, if dimmed { if p.light { hex(0x463D57) } else { hex(0xCFC7E6) } } else { p.ink_2 }),
            None => ("", 450.0, p.mute),
        };
        text(content).font(TextStyle::new((13.0 * zoom).clamp(11.4, 13.5), weight)).color(color)
    };
    let mut r = row()
        .w_full()
        .h(SOCKET_ROW * zoom)
        .px((11.0 * zoom).max(9.0))
        .items_center()
        .justify_between()
        .child(label(card.inputs.get(row_index)))
        .child(label(card.outputs.get(row_index)));
    if row_index > 0 {
        r = r.border_sides([1.0, 0.0, 0.0, 0.0], with_alpha(p.rule, 0.72));
    }
    r
}

fn grimoire_card_view<S: 'static>(
    state: &CanvasState,
    card: &NodeCardModel,
    p: &HearthPalette,
    palette: CanvasPalette,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let zoom = state.camera.zoom;
    let dimmed = state.has_run() && card.status == ExecutionStatus::Idle && !card.selected;
    let tone = category_tone(&card.category, state.light);
    let [x, y] = state.camera.world_to_screen(card.position);
    let card_w = card.size[0] * zoom;
    let card_h = card.size[1] * zoom;
    let node_id = card.id.clone();
    let dimmed_bg = if state.light { hex(0xEBE8F0) } else { hex(0x1D182B) };
    let dimmed_header = if state.light { hex(0xE0DCE8) } else { hex(0x262038) };
    let dimmed_border = if state.light { hex(0xB5AEC2) } else { hex(0x4F446B) };
    let header_bg = if dimmed { dimmed_header } else { tone.tint };
    let header_h = CARD_HEADER * zoom;
    let title_size = (13.6 * zoom).clamp(11.8, 15.5);
    let kind_size = (10.8 * zoom).clamp(10.2, 11.6);
    let title_color = if dimmed { if state.light { hex(0x362E45) } else { hex(0xDFD8F2) } } else { p.ink };
    let kind_color = if dimmed { if state.light { hex(0x3F3652) } else { hex(0xD4CCE8) } } else { p.mute };
    let vial_size = (22.0 * zoom).clamp(17.0, 26.0);
    let duplicate_title = state.nodes.iter().filter(|other| other.title == card.title).count() > 1;
    let display_title = if duplicate_title {
        let branch = card.id.0.strip_suffix("_reply").unwrap_or(&card.id.0);
        let short = card.title.split_whitespace().next().unwrap_or(&card.title);
        format!("{short} · {branch}")
    } else {
        card.title.clone()
    };
    let header_inner = row()
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .px((8.0 * zoom).max(6.0))
        .gap((6.0 * zoom).max(5.0))
        .items_center()
        .child(canvas(mini_vial_icon(&card.category, state.light, vial_size)).size(vial_size, vial_size))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .justify_center()
                .gap((1.0 * zoom).max(0.5))
                .child(text(display_title).font(TextStyle::new(title_size, 750.0)).color(title_color).truncate())
                .child(text(&card.kind).font(TextStyle::new(kind_size, 500.0).mono()).color(kind_color).truncate()),
        );
    let perched_badge = status_box(card.status, card.duration_ms, p, zoom).or_else(|| {
        dimmed.then(|| {
            let badge_bg = if state.light { hex(0xF5F2FA) } else { hex(0x2B243F) };
            let badge_ink = if state.light { hex(0x322A42) } else { hex(0xDFD8F2) };
            row()
                .px((6.0 * zoom).max(5.5))
                .py((1.8 * zoom).max(1.5))
                .radius((6.5 * zoom).max(6.0))
                .fill(badge_bg)
                .border(1.0, dimmed_border)
                .child(text("Untaken").font(TextStyle::new((11.0 * zoom).clamp(11.0, 12.0), 650.0)).color(badge_ink))
        })
    });
    let header = row()
        .key(format!("node-{}-header", card.id.0))
        .w_full()
        .h(header_h)
        .items_center()
        .fill(header_bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], if dimmed { dimmed_border } else { tone.border })
        .child(header_inner)
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
    let body = col().w_full().py(6.0 * zoom).children((0..rows).map(|row_index| socket_row(card, row_index, dimmed, p, zoom)));
    let select_id = card.id.clone();
    let face = col()
        .key(format!("node-{}-face", card.id.0))
        .w_full()
        .h_full()
        .fill(if dimmed { dimmed_bg } else { p.card })
        .child(header)
        .child(body)
        .on_press(move |s: &mut S| lens(s).select(Some(select_id.clone())));
    let (border_color, border_width) = if card.status == ExecutionStatus::Failed {
        (p.hex, 2.0)
    } else if card.selected {
        (tone.liquid, 2.2)
    } else if card.status == ExecutionStatus::Succeeded {
        (p.broth_mid, 1.8)
    } else if dimmed {
        (dimmed_border, 1.2)
    } else {
        (tone.border, 1.4)
    };
    let corner = (12.0 * zoom).clamp(7.0, 14.0);
    let card_frame = el()
        .size(card_w, card_h)
        .radius(corner)
        .continuous_corners()
        .fill(if dimmed { dimmed_bg } else { p.card })
        .clip_rounded(corner)
        .shadow(if card.selected { 5.0 } else { 3.0 }, if card.selected { 14.0 } else { 8.0 }, if dimmed { Color::TRANSPARENT } else { p.shadow_card })
        .absolute(0.0, 0.0)
        .child(face);
    let border_overlay = el().size(card_w, card_h).radius(corner).continuous_corners().border(border_width, border_color).absolute(0.0, 0.0);
    let mut shell = el().key(format!("node-{}", card.id.0)).size(card_w, card_h).absolute(x, y).child(card_frame).child(border_overlay);
    let _ = palette;
    let inputs = card.inputs.iter().map(|socket| socket_port::<S>(state, card, socket, false, dimmed, p, zoom, lens));
    let outputs = card.outputs.iter().map(|socket| socket_port::<S>(state, card, socket, true, dimmed, p, zoom, lens));
    shell = inputs.chain(outputs).fold(shell, |shell, port| shell.child(port));
    if let Some(badge) = perched_badge {
        shell = shell.child(row().w(card_w).px((7.0 * zoom).max(5.0)).justify_end().absolute(0.0, -(19.5 * zoom).clamp(17.5, 21.0)).child(badge));
    }
    if let Some(step) = card.step_order {
        let step_sz = (18.0 * zoom).clamp(16.0, 20.0);
        let step_badge = row()
            .size(step_sz, step_sz)
            .radius(step_sz * 0.5)
            .items_center()
            .justify_center()
            .fill(p.primary_bg)
            .child(text(step.to_string()).font(TextStyle::new((10.0 * zoom).clamp(9.4, 11.0), 750.0).features("'tnum' 1")).color(p.primary_ink));
        shell = shell.child(el().absolute(-6.0 * zoom, -(18.5 * zoom).clamp(16.5, 20.0)).child(step_badge));
    }
    shell
}

#[allow(clippy::too_many_arguments)]
fn socket_port<S: 'static>(
    state: &CanvasState,
    card: &NodeCardModel,
    socket: &SocketModel,
    output: bool,
    dimmed: bool,
    p: &HearthPalette,
    zoom: f32,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let side = if output { "out" } else { "in" };
    let is_flow = socket.ty.is_flow();
    let size = if socket.flagged {
        11.0
    } else if is_flow {
        FLOW_KNOT_SIZE
    } else {
        DATA_PIN_SIZE
    };
    let wired = state
        .wires
        .iter()
        .any(|w| if output { w.from_node == card.id && w.from_socket == socket.name } else { w.to_node == card.id && w.to_socket == socket.name });
    let wired_active = state
        .wires
        .iter()
        .any(|w| w.active && if output { w.from_node == card.id && w.from_socket == socket.name } else { w.to_node == card.id && w.to_socket == socket.name });
    let muted_dot = if p.light { hex(0x766D8A) } else { hex(0x948AB8) };
    let flow_col = if p.light { hex(0x137B60) } else { p.broth_mid };
    let has_run = state.has_run();
    let (fill, border_color, radius) = if socket.flagged {
        (p.hex, p.card, if is_flow { size * zoom * 0.5 } else { 2.0 * zoom })
    } else if is_flow {
        let r = size * zoom * 0.5;
        if !wired {
            (p.card, if dimmed { muted_dot } else { flow_col }, r)
        } else if has_run && !wired_active {
            (muted_dot, p.card, r)
        } else if card.status == ExecutionStatus::Running {
            (p.gold, p.card, r)
        } else {
            (flow_col, p.card, r)
        }
    } else if !wired {
        (p.card, if dimmed { muted_dot } else { p.ink }, 2.0 * zoom)
    } else if wired_active {
        (if p.light { hex(0x3B2148) } else { hex(0xEEE8FA) }, p.card, 2.0 * zoom)
    } else if has_run {
        (muted_dot, p.card, 2.0 * zoom)
    } else {
        (p.data_wire, p.card, 2.0 * zoom)
    };
    let port = el()
        .key(format!("socket-{}-{side}-{}", card.id.0, socket.name))
        .size(size * zoom, size * zoom)
        .radius(radius)
        .fill(fill)
        .border((1.6 * zoom).max(1.0), border_color)
        .absolute((socket.offset[0] - size / 2.0) * zoom, (socket.offset[1] - size / 2.0) * zoom);
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

pub fn brand_mark(light: bool) -> Shape {
    brand_cauldron_mark(light)
}
