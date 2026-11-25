use magicklala_graph::{RunStatus, SocketType};
use rune::{Color, El, Phase, Shape, TextStyle, canvas, col, el, row, text};

use crate::brew::BrewPlan;
use crate::cauldron::{Toss, TossHook, cauldron_workbench_view};
use crate::hearth::{
    CauldronMood, HearthPalette, WireStyle, brand_cauldron_mark, category_tone, fill_circle, fill_star4, grimoire_wire_shape, hex, mini_vial_icon,
    potion_bottle_shape, with_alpha,
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
        ExecutionStatus::Running => p.running_mark(),
        ExecutionStatus::Succeeded => p.broth_mid,
        ExecutionStatus::Failed => p.hex,
    }
}

pub fn canvas_view(state: &CanvasState, size: (f32, f32)) -> El {
    interactive_canvas_view::<CanvasState>(state, size, |state| state, |state, toss| state.apply_toss(&toss))
}

pub fn interactive_canvas_view<S: 'static>(state: &CanvasState, size: (f32, f32), lens: fn(&mut S) -> &mut CanvasState, toss: TossHook<S>) -> El {
    match state.mode {
        WorkbenchMode::Cauldron => cauldron_workbench_view(state, size, lens, toss),
        WorkbenchMode::Grimoire => grimoire_graph_view(state, size, lens),
    }
}

pub fn cauldron_mood(state: &CanvasState) -> CauldronMood {
    if state.nodes.iter().all(|card| state.is_shelved(&card.id)) {
        return CauldronMood::Empty;
    }
    if state.running_brew {
        return CauldronMood::Simmering;
    }
    if state.invalid_brew || state.nodes.iter().any(|card| card.status == ExecutionStatus::Failed) {
        return CauldronMood::Hexed;
    }
    match state.last_brew {
        Some(RunStatus::Succeeded) => return CauldronMood::Brewed,
        Some(RunStatus::Failed | RunStatus::TimedOut) => return CauldronMood::Hexed,
        Some(RunStatus::Cancelled) => return CauldronMood::Ready,
        None => {}
    }
    if state.nodes.iter().any(|card| card.status == ExecutionStatus::Running) {
        return CauldronMood::Simmering;
    }
    if state.has_run() || state.nodes.iter().any(|card| card.status == ExecutionStatus::Succeeded) {
        return CauldronMood::Brewed;
    }
    CauldronMood::Ready
}

pub(crate) fn mode_switcher<S: 'static>(state: &CanvasState, p: &HearthPalette, lens: fn(&mut S) -> &mut CanvasState, size: (f32, f32)) -> El {
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
                if target == WorkbenchMode::Grimoire && !c.grimoire_fitted {
                    c.fit(size);
                    c.grimoire_fitted = true;
                }
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

#[allow(clippy::too_many_arguments)]
pub(crate) const LEGIBLE_PX: f32 = 10.5;

pub(crate) fn legible_size(size: f32, scale: f32) -> f32 {
    size.max(LEGIBLE_PX / scale.max(0.05))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ingredient_vial_card<S: 'static>(
    state: &CanvasState,
    card: &NodeCardModel,
    step_num: Option<usize>,
    pos: [f32; 2],
    dims: [f32; 2],
    aside: bool,
    mirrored: bool,
    p: &HearthPalette,
    lens: fn(&mut S) -> &mut CanvasState,
    toss: TossHook<S>,
    plan: BrewPlan,
) -> El {
    let tone = category_tone(&card.category, state.light);
    let hexed = card.status == ExecutionStatus::Failed || card.inputs.iter().any(|s| s.flagged);
    let dimmed = state.has_run() && card.status == ExecutionStatus::Idle && !card.selected && !hexed;
    let untaken_bg = p.untaken_fill();
    let untaken_border = p.untaken_border();
    let untaken_title = p.untaken_title();
    let untaken_sub = p.untaken_sub();
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

    let legible = |size: f32| legible_size(size, plan.scale);
    let dense = dims[1] < 50.0;
    let narrow = dims[0] < 170.0;
    let crammed = legible(13.0) > 13.0;
    let badge = if crammed {
        24.0
    } else if dense || narrow {
        30.0
    } else {
        38.0
    };
    let vial_badge = el()
        .key(format!("vial-{}", card.id.0))
        .size(badge, badge)
        .radius(badge * 0.5)
        .fill(if hexed { with_alpha(p.hex, 0.14) } else { tone.tint })
        .border(1.3, if hexed { p.hex } else { tone.border })
        .center()
        .child(canvas(mini_vial_icon(&card.category, state.light, badge * 0.76)).size(badge * 0.76, badge * 0.76).rotate(if aside {
            0.0
        } else if mirrored {
            -0.5
        } else {
            0.5
        }));

    let badge_side = legible(11.0) * 1.8;
    let step_badge = step_num.map(|num| {
        let side = badge_side;
        row()
            .size(side, side)
            .radius(side * 0.5)
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
            .child(
                text(num.to_string())
                    .font(TextStyle::new(legible(11.0), 750.0).features("'tnum' 1"))
                    .color(if hexed || card.status == ExecutionStatus::Succeeded { p.primary_ink } else { p.ink_2 }),
            )
    });

    let status_pill = if hexed {
        let missing_label = card
            .inputs
            .iter()
            .find(|s| s.flagged)
            .map_or_else(|| "Hexed".to_owned(), |s| if s.mistyped { format!("Wrong type \"{}\"", s.name) } else { format!("Missing \"{}\"", s.name) });
        let pill_room = dims[0] - if step_num.is_some() { legible(11.0) * 1.8 + 2.0 } else { 10.0 } - 10.0 - 12.0;
        let missing_label = if missing_label.chars().count() as f32 * 7.2 * legible(11.0) / 11.0 > pill_room {
            missing_label.split(' ').next().unwrap_or("Hexed").to_owned()
        } else {
            missing_label
        };
        Some(
            row()
                .px(6.0)
                .py(1.5)
                .radius(7.0)
                .fill(p.card)
                .border(1.2, p.hex)
                .shadow(2.0, 6.0, p.shadow_card)
                .min_w(0.0)
                .child(text(missing_label).font(TextStyle::new(legible(11.0), 750.0)).color(p.hex).truncate()),
        )
    } else if card.status == ExecutionStatus::Succeeded {
        let ms_text = match card.duration_ms {
            Some(0) => "<1ms".to_owned(),
            Some(ms) => format!("{ms}ms"),
            None => String::new(),
        };
        Some(row().px(5.5).py(1.5).radius(7.0).fill(p.card).border(1.0, p.broth_mid).shadow(2.0, 6.0, p.shadow_card).child(
            row().gap(3.0).items_center().child(crate::hearth::check_mark(if state.light { p.broth_deep } else { p.broth_foam }, 10.0)).child(
                text(ms_text).font(TextStyle::new(legible(11.0), 700.0).features("'tnum' 1")).color(if state.light { p.broth_deep } else { p.broth_foam }),
            ),
        ))
    } else if card.status == ExecutionStatus::Running {
        Some(
            row()
                .px(6.0)
                .py(1.5)
                .radius(7.0)
                .fill(p.card)
                .border(1.0, p.gold)
                .child(text("Simmering").font(TextStyle::new(legible(11.0), 750.0)).color(p.simmer_ink())),
        )
    } else if dimmed && state.brewed_clean() {
        let branch = |socket: &str| !(socket == "out" || socket.starts_with("out_") || socket == "in");
        let untaken_label = state
            .wires
            .iter()
            .find(|fw| fw.to_node == card.id && fw.ty.is_flow())
            .or_else(|| {
                state.wires.iter().find(|w| w.from_node == card.id).and_then(|w| state.wires.iter().find(|fw| fw.to_node == w.to_node && fw.ty.is_flow()))
            })
            .filter(|fw| branch(&fw.from_socket))
            .map_or_else(|| "Untaken".to_owned(), |fw| format!("Untaken · {}", fw.from_socket));
        Some(
            row()
                .px(5.5)
                .py(1.5)
                .radius(7.0)
                .fill(p.untaken_pill_fill())
                .border(1.0, p.untaken_border())
                .min_w(0.0)
                .child(text(untaken_label).font(TextStyle::new(legible(11.0), 700.0)).color(p.untaken_pill_ink()).truncate()),
        )
    } else {
        None
    };

    let title_size = legible(13.0);
    let kind_size = legible(11.5);
    let step_pad = if step_num.is_some() { (badge_side - 4.0).max(15.0) } else { 9.0 };
    let title_room_est = dims[0] - step_pad - 9.0 - badge - if narrow { 6.0 } else { 9.0 };
    let one_line = plan
        .cards
        .iter()
        .filter(|placed| placed.role != crate::brew::BrewRole::Bottle)
        .all(|placed| state.display_title(&placed.id).chars().count() as f32 * 7.6 * title_size / 13.0 <= title_room_est);
    let title_lines = if one_line || crammed { 1.0 } else { 2.0 };
    let kind_fits = !dense && (title_lines * title_size + kind_size) * 1.25 + 8.0 <= dims[1];
    let title_room = dims[0] - 24.0 - 6.0 - 30.0;
    let wraps = narrow && state.display_title(&card.id).chars().count() as f32 * 7.4 * title_size / 13.0 > title_room;
    let top_row = col().w_full().min_w(0.0).child({
        let title = text(state.display_title(&card.id).replace(" · ", "\u{a0}·\u{a0}")).font(TextStyle::new(title_size, 750.0)).color(if dimmed {
            untaken_title
        } else {
            p.ink
        });
        if crammed { title.truncate() } else { title.wrap() }
    });

    let sub_color = if hexed {
        p.hex
    } else if dimmed {
        untaken_sub
    } else {
        p.mute
    };

    let kind_line = kind_fits.then(|| text(&card.kind).font(TextStyle::new(kind_size, 500.0).mono()).color(sub_color).truncate());
    let side_by_side = |words: El, vial: El, gap: f32| {
        let pair = row().grow(1.0).min_w(0.0).gap(gap).items_center();
        if mirrored { pair.child(vial).child(words) } else { pair.child(words).child(vial) }
    };
    let (card_body, hung_vial) = if crammed && plan.lanes.is_some() {
        (side_by_side(col().grow(1.0).min_w(0.0).gap(1.0).child(top_row).children(kind_line), vial_badge, 9.0), None)
    } else if crammed {
        let hang = [if mirrored { -badge * 0.5 } else { dims[0] - badge * 0.5 }, (dims[1] - badge) * 0.5];
        (col().grow(1.0).min_w(0.0).gap(1.0).child(top_row).children(kind_line), Some(el().absolute(hang[0], hang[1]).child(vial_badge)))
    } else if dense {
        (side_by_side(col().grow(1.0).min_w(0.0).child(top_row), vial_badge, 9.0), None)
    } else if narrow {
        let words = col().grow(1.0).min_w(0.0).child(top_row);
        let pair = row().w_full().gap(6.0).items_center();
        let pair = if mirrored { pair.child(vial_badge).child(words) } else { pair.child(words).child(vial_badge) };
        (col().grow(1.0).min_w(0.0).gap(if wraps { 0.0 } else { 2.0 }).child(pair).children(kind_line), None)
    } else {
        (side_by_side(col().grow(1.0).min_w(0.0).gap(2.0).child(top_row).children(kind_line), vial_badge, 9.0), None)
    };

    let content = row()
        .key(format!("node-{}-header", card.id.0))
        .size(dims[0], dims[1])
        .pl(if step_num.is_some() && !mirrored {
            (badge_side - 4.0).max(15.0)
        } else if hung_vial.is_some() && mirrored {
            badge * 0.5 + 5.0
        } else {
            9.0
        })
        .pr(if step_num.is_some() && mirrored {
            (badge_side - 4.0).max(15.0)
        } else if hung_vial.is_some() && !mirrored {
            badge * 0.5 + 5.0
        } else {
            9.0
        })
        .gap(9.0)
        .radius(24.0)
        .continuous_corners()
        .items_center()
        .fill(fill_col)
        .border(border_w, border_col)
        .shadow(if card.selected { 5.0 } else { 3.0 }, if card.selected { 14.0 } else { 9.0 }, if dimmed { Color::TRANSPARENT } else { p.shadow_card })
        .child(card_body)
        .children(hung_vial)
        .on_press(move |s: &mut S| lens(s).select(Some(select_id.clone())))
        .on_drag(move |s: &mut S, drag, tx| {
            tx.instant();
            if lens(s).running_brew {
                return;
            }
            match drag.phase {
                Phase::Down => {
                    let canvas = lens(s);
                    canvas.select(Some(node_id.clone()));
                    canvas.drag = DragState::LiftingVial { node: node_id.clone(), offset: [0.0, 0.0] };
                }
                Phase::Move => {
                    let canvas = lens(s);
                    if matches!(&canvas.drag, DragState::LiftingVial { node, .. } if node == &node_id) {
                        canvas.drag = DragState::LiftingVial { node: node_id.clone(), offset: [drag.dx, drag.dy] };
                    }
                }
                Phase::Up => {
                    let canvas = lens(s);
                    let lifted = matches!(&canvas.drag, DragState::LiftingVial { node, .. } if node == &node_id);
                    canvas.drag = DragState::Idle;
                    if !lifted || drag.cancelled() || drag.dx.hypot(drag.dy) < 6.0 {
                        return;
                    }
                    let Some(placed) = plan.card(&node_id) else { return };
                    let dropped_at = [placed.rect[0] + placed.rect[2] * 0.5 + drag.dx, placed.rect[1] + placed.rect[3] * 0.5 + drag.dy];
                    if aside && plan.is_over_pot(dropped_at) {
                        toss(s, Toss::IntoPot(node_id.clone()));
                    } else if !aside && plan.is_in_aside_zone(dropped_at) {
                        toss(s, Toss::Aside(node_id.clone()));
                    }
                }
            }
        });

    let mut shell = el().key(format!("node-{}", card.id.0)).size(dims[0], dims[1]).absolute(pos[0], pos[1]).child(content);
    if let Some(badge) = step_badge {
        shell = shell.child(el().key(format!("step-badge-{}", card.id.0)).absolute(if mirrored { dims[0] - 11.0 } else { -9.0 }, -9.0).child(badge));
    }
    if let Some(pill) = status_pill {
        let clear = if step_num.is_some() { badge_side + 2.0 } else { 10.0 };
        let lane = row().w(dims[0]).pl(if mirrored { 10.0 } else { clear }).pr(if mirrored { clear } else { 10.0 }).absolute(0.0, -12.0);
        let lane = if mirrored { lane.justify_start() } else { lane.justify_end() };
        let room = dims[0] - clear - 10.0;
        shell = shell.child(lane.child(el().key(format!("status-pill-{}", card.id.0)).max_w(room).min_w(0.0).child(pill.max_w(room))));
    }
    shell
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn branch_bottle_view<S: 'static>(
    state: &CanvasState,
    card: &NodeCardModel,
    step_num: Option<usize>,
    frame: [f32; 3],
    tint: Option<Color>,
    has_run: bool,
    terse: bool,
    scale: f32,
    p: &HearthPalette,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let legible = |size: f32| legible_size(size, scale);
    let [pos_x, pos_y, width] = frame;
    let taken = card.status == ExecutionStatus::Succeeded;
    let simmering = card.status == ExecutionStatus::Running;
    let untaken = has_run && !taken && !simmering && state.brewed_clean();
    let sealed = state.release_badge.is_some() && !has_run;
    let bottle_mood = branch_bottle_mood(state, card);
    let bottle_h = 66.0;
    let route = crate::cauldron::route_label(state, &card.id);
    let title = state.display_title(&card.id);
    let route_key = route.as_ref().map(|_| format!("route-{}", card.id.0));
    let hex_pill = bottle_hex_pill(card, scale, p);
    let hexed = hex_pill.is_some();
    let bottle_art = canvas(potion_bottle_shape(bottle_mood, state.light, width, bottle_h, sealed && !hexed, tint)).size(width, bottle_h);
    let pill = if let Some(hex_pill) = hex_pill {
        hex_pill
    } else if taken {
        let ms_text = match card.duration_ms {
            Some(0) => "<1ms".to_owned(),
            Some(ms) => format!("{ms}ms"),
            None => String::new(),
        };
        row().px(5.5).py(1.5).radius(7.0).fill(p.card).border(1.0, p.broth_mid).shadow(2.0, 6.0, p.shadow_card).child(
            row().gap(3.0).items_center().child(crate::hearth::check_mark(if state.light { p.broth_deep } else { p.broth_foam }, 10.0)).child(
                text(ms_text).font(TextStyle::new(legible(11.0), 700.0).features("'tnum' 1")).color(if state.light { p.broth_deep } else { p.broth_foam }),
            ),
        )
    } else if simmering {
        row()
            .px(6.0)
            .py(1.5)
            .radius(7.0)
            .fill(p.card)
            .border(1.0, p.gold)
            .child(text("Simmering").font(TextStyle::new(legible(11.0), 750.0)).color(p.simmer_ink()))
    } else if untaken {
        row()
            .px(5.5)
            .py(1.5)
            .radius(7.0)
            .fill(p.untaken_pill_fill())
            .border(1.0, p.untaken_border())
            .child(text("Untaken").font(TextStyle::new(legible(11.0), 700.0)).color(p.untaken_title()))
    } else if let Some(vintage) = state.release_badge.as_ref().filter(|_| sealed) {
        row()
            .px(5.0)
            .py(1.0)
            .radius(5.5)
            .fill(p.primary_bg)
            .child(text(vintage).font(TextStyle::new(legible(11.0), 700.0).features("'tnum' 1")).color(p.primary_ink))
    } else {
        el()
    };
    let card_bg = if hexed {
        p.hex_tint
    } else if untaken {
        p.untaken_fill()
    } else {
        p.card
    };
    let card_border = if hexed {
        p.hex
    } else if card.selected || taken {
        p.broth_mid
    } else if untaken {
        p.untaken_border()
    } else {
        p.rule_2
    };
    let title_col = if untaken { p.untaken_title() } else { p.ink };
    let sub_col = if untaken { p.untaken_sub() } else { p.mute };
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
        .child(match route_key {
            Some(key) => {
                col().key(key).w_full().min_w(0.0).child(text(title).font(TextStyle::new(legible(12.5), 750.0)).color(title_col).wrap_anywhere().max_lines(2))
            }
            None => col().w_full().min_w(0.0).child(text(title).font(TextStyle::new(legible(12.5), 750.0)).color(title_col).wrap_anywhere().max_lines(2)),
        })
        .children((!terse).then(|| text(&card.kind).font(TextStyle::new(legible(11.0), 500.0).mono()).color(sub_col).truncate()));
    let mut tag_wrap = el().w_full().child(tag_inner).child(row().w_full().px(8.0).justify_end().absolute(0.0, -12.0).child(pill));
    if let Some(num) = step_num {
        let side = legible(11.0) * 1.55;
        let step_badge = row()
            .size(side, side)
            .radius(side * 0.5)
            .items_center()
            .justify_center()
            .fill(if hexed { p.hex } else { p.primary_bg })
            .child(text(num.to_string()).font(TextStyle::new(legible(11.0), 750.0).features("'tnum' 1")).color(p.primary_ink));
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

pub fn branch_bottle_mood(state: &CanvasState, card: &NodeCardModel) -> CauldronMood {
    let taken = card.status == ExecutionStatus::Succeeded;
    let simmering = card.status == ExecutionStatus::Running;
    let hexed = card.status == ExecutionStatus::Failed || card.inputs.iter().any(|socket| socket.flagged);
    if taken {
        CauldronMood::Brewed
    } else if (state.has_run() && !simmering && state.brewed_clean()) || hexed || cauldron_mood(state) == CauldronMood::Hexed {
        CauldronMood::Hexed
    } else {
        CauldronMood::Ready
    }
}

pub fn bottle_art_mood(state: &CanvasState, output: &NodeCardModel) -> CauldronMood {
    match cauldron_mood(state) {
        CauldronMood::Brewed | CauldronMood::Simmering if !crate::cauldron::bottle_filled(state, output) => CauldronMood::Ready,
        mood => mood,
    }
}

pub fn grimoire_lanes(state: &CanvasState) -> Vec<(magicklala_graph::WireId, i8)> {
    let mut untaken: Vec<&crate::WireModel> = state.wires.iter().filter(|wire| state.has_run() && !wire.active && wire.ty.is_flow()).collect();
    untaken.sort_by(|a, b| a.from_node.0.cmp(&b.from_node.0).then(b.from[1].total_cmp(&a.from[1])));
    let mut lanes = Vec::with_capacity(untaken.len());
    for (index, wire) in untaken.iter().enumerate() {
        let rank = untaken[..index].iter().filter(|other| other.from_node == wire.from_node).count();
        lanes.push((wire.id.clone(), if rank % 2 == 0 { 1 } else { -1 }));
    }
    lanes
}

const PULL_SLOP: f32 = 4.0;
pub(crate) const DETAIL_ZOOM: f32 = 0.6;
pub(crate) const LABEL_ZOOM: f32 = 0.42;

fn bottle_hex_pill(card: &NodeCardModel, scale: f32, p: &HearthPalette) -> Option<El> {
    let caption = match card.inputs.iter().find(|socket| socket.flagged) {
        Some(socket) if socket.mistyped => format!("Wrong type \"{}\"", socket.name),
        Some(socket) => format!("Missing \"{}\"", socket.name),
        None if card.status == ExecutionStatus::Failed => "Hexed".to_owned(),
        None => return None,
    };
    Some(
        row()
            .px(6.0)
            .py(1.5)
            .radius(7.0)
            .fill(p.card)
            .border(1.2, p.hex)
            .shadow(2.0, 6.0, p.shadow_card)
            .child(text(caption).font(TextStyle::new(legible_size(11.0, scale), 750.0)).color(p.hex)),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn potion_stand_view<S: 'static>(
    state: &CanvasState,
    output: &NodeCardModel,
    step_num: Option<usize>,
    frame: [f32; 3],
    tint: Option<Color>,
    mood: CauldronMood,
    terse: bool,
    scale: f32,
    p: &HearthPalette,
    lens: fn(&mut S) -> &mut CanvasState,
) -> El {
    let legible = |size: f32| legible_size(size, scale);
    let [pos_x, pos_y, width] = frame;
    let pos = [pos_x, pos_y];
    let primary_output = Some(output);
    let bottled = state.release_badge.is_some();
    let sealed_vintage = bottled && !matches!(mood, CauldronMood::Simmering | CauldronMood::Hexed | CauldronMood::Empty);
    let bottle_h = 104.0;
    let art_mood = bottle_art_mood(state, output);
    let bottle_art = canvas(potion_bottle_shape(art_mood, state.light, width, bottle_h, bottled, tint)).key("hero-potion-bottle").size(width, bottle_h);
    let spell_label = if state.spell_name.is_empty() { "Potion" } else { state.spell_name.as_str() };
    let out_title = primary_output.map_or_else(|| "Bottled Potion".to_owned(), |c| state.display_title(&c.id));
    let out_kind = primary_output.map_or("io/send_message", |c| c.kind.as_str());
    let selected = primary_output.is_some_and(|c| c.selected);
    let select_id = primary_output.map(|c| c.id.clone());
    let hex_pill = bottle_hex_pill(output, scale, p);
    let hexed = hex_pill.is_some();

    let out_brewed = primary_output.is_some_and(|c| c.status == ExecutionStatus::Succeeded);
    let mut header_row =
        row().w_full().gap(4.0).items_center().justify_between().child(text(spell_label).font(TextStyle::new(legible(13.0), 750.0)).color(p.ink).truncate());
    if sealed_vintage && let Some(vintage) = &state.release_badge {
        header_row = header_row.child(
            row()
                .px(5.0)
                .py(1.0)
                .radius(5.5)
                .fill(p.primary_bg)
                .child(text(vintage).font(TextStyle::new(legible(11.0), 700.0).features("'tnum' 1")).color(p.primary_ink)),
        );
    }

    let mut tag_card = col()
        .w_full()
        .px(10.0)
        .py(9.0)
        .gap(2.0)
        .radius(14.0)
        .continuous_corners()
        .fill(if hexed { p.hex_tint } else { p.card })
        .border(
            if selected || hexed { 2.0 } else { 1.2 },
            if hexed {
                p.hex
            } else if selected {
                p.broth_mid
            } else {
                p.rule_2
            },
        )
        .shadow(3.0, 10.0, p.shadow_card)
        .child(header_row)
        .child(text(&out_title).font(TextStyle::new(legible(12.0), 600.0)).color(p.ink_2).truncate())
        .children((!terse).then(|| text(out_kind).font(TextStyle::new(legible(11.0), 500.0).mono()).color(p.mute).truncate()));
    if out_brewed {
        let ms_text = match primary_output.and_then(|c| c.duration_ms) {
            Some(0) => "<1ms".to_owned(),
            Some(ms) => format!("{ms}ms"),
            None => String::new(),
        };
        let timing_pill = row().px(5.5).py(1.5).radius(7.0).fill(p.card).border(1.0, p.broth_mid).shadow(2.0, 6.0, p.shadow_card).child(
            row().gap(3.0).items_center().child(crate::hearth::check_mark(if state.light { p.broth_deep } else { p.broth_foam }, 10.0)).child(
                text(ms_text).font(TextStyle::new(legible(11.0), 700.0).features("'tnum' 1")).color(if state.light { p.broth_deep } else { p.broth_foam }),
            ),
        );
        let mut wrapped = el().w_full().child(tag_card).child(row().w_full().px(8.0).justify_end().absolute(0.0, -12.0).child(timing_pill));
        if let Some(num) = step_num {
            let side = legible(11.0) * 1.6;
            let step_badge = row()
                .size(side, side)
                .radius(side * 0.5)
                .items_center()
                .justify_center()
                .fill(p.primary_bg)
                .child(text(num.to_string()).font(TextStyle::new(legible(11.0), 750.0).features("'tnum' 1")).color(p.primary_ink));
            wrapped = wrapped.child(el().absolute(-5.0, -7.0).child(step_badge));
        }
        tag_card = col().w_full().child(wrapped);
    } else if let Some(pill) = hex_pill {
        tag_card = col().w_full().child(el().w_full().child(tag_card).child(row().w_full().px(8.0).justify_end().absolute(0.0, -12.0).child(pill)));
    }

    let stand = col().key("potion-stand").w(width).gap(4.0).items_center().absolute(pos[0], pos[1]);
    let mut stand = stand.child(bottle_art).child(tag_card);

    if let Some(id) = select_id {
        stand = stand.key(format!("node-{}-header", id.0)).on_press(move |s: &mut S| lens(s).select(Some(id.clone())));
    }
    stand
}

pub(crate) fn apothecary_dots(state: &CanvasState, size: (f32, f32), p: &HearthPalette, center: [f32; 2], circle_r: f32) -> Shape {
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
            canvas.context_card = hit.as_ref().and_then(|id| canvas.card(id)).map(|card| {
                let top_left = canvas.camera.world_to_screen(card.position);
                let bottom_right = canvas.camera.world_to_screen([card.position[0] + card.size[0], card.position[1] + card.size[1]]);
                [top_left[0], top_left[1], bottom_right[0] - top_left[0], bottom_right[1] - top_left[1]]
            });
            canvas.select(hit);
            canvas.context_menu = Some([press.x, press.y]);
        })
        .child(backdrop)
        .child(wires);

    let mut board = state.nodes.iter().fold(board, |board, card| board.child(grimoire_card_view(state, card, &p, palette, lens)));
    if let Some((id, name, output)) = state.hover_socket.clone().filter(|_| state.camera.zoom < LABEL_ZOOM)
        && let Some((world, _)) = state.card(&id).and_then(|card| card.socket_world(&name, output))
    {
        let [sx, sy] = state.camera.world_to_screen(world);
        let tip = row()
            .key("socket-tip")
            .px(6.0)
            .py(2.0)
            .radius(6.0)
            .fill(p.card)
            .border(1.0, p.rule_2)
            .shadow(2.0, 6.0, p.shadow_card)
            .child(text(&name).font(TextStyle::new(11.5, 650.0)).color(p.ink));
        let room = 220.0;
        board = board.child(if output || sx < room * 0.5 {
            el().absolute(sx + 9.0, sy - 10.0).child(tip)
        } else {
            row().w(room).justify_end().absolute(sx - 9.0 - room, sy - 10.0).child(tip)
        });
    }
    board = board.child(mode_switcher(state, &p, lens, size));

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
    let lanes = grimoire_lanes(state);
    let lifted = match &state.drag {
        DragState::PullingWire { held, detached: true, .. } => Some(held.id.clone()),
        _ => None,
    };
    let shape = ordered.into_iter().filter(|(_, wire)| Some(&wire.id) != lifted.as_ref()).fold(Shape::new(), |shape, (_, wire)| {
        let from = state.camera.world_to_screen(wire.from);
        let to = state.camera.world_to_screen(wire.to);
        let untaken = has_run && !wire.active;
        let lane = lanes.iter().find(|(id, _)| id == &wire.id).map_or(0, |(_, lane)| *lane);
        grimoire_wire_shape(shape, from, to, WireStyle { zoom, is_flow: wire.ty.is_flow(), active: wire.active, untaken, light: state.light, lane })
    });
    match &state.drag {
        DragState::ConnectingWire { ty, anchor, cursor, .. } => {
            let from = state.camera.world_to_screen(*anchor);
            let to = state.camera.world_to_screen(*cursor);
            grimoire_wire_shape(shape, from, to, WireStyle { zoom, is_flow: ty.is_flow(), active: true, untaken: false, light: state.light, lane: 0 })
        }
        DragState::PullingWire { held, cursor, detached: true, .. } => {
            let anchor = state.card(&held.from_node).and_then(|card| card.socket_world(&held.from_socket, true)).map_or(held.from, |(point, _)| point);
            let from = state.camera.world_to_screen(anchor);
            let to = state.camera.world_to_screen(*cursor);
            grimoire_wire_shape(shape, from, to, WireStyle { zoom, is_flow: held.ty.is_flow(), active: true, untaken: false, light: state.light, lane: 0 })
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
        ExecutionStatus::Running => (p.gold, p.simmer_ink()),
        ExecutionStatus::Succeeded => (p.broth_mid, if p.light { p.broth_deep } else { p.broth_foam }),
        ExecutionStatus::Idle => return None,
    };
    let text_label = match (status, duration_ms) {
        (ExecutionStatus::Succeeded, Some(0)) => "<1ms".to_owned(),
        (ExecutionStatus::Succeeded, Some(ms)) => format!("{ms}ms"),
        _ => status.display_label().to_owned(),
    };
    let tick = (status == ExecutionStatus::Succeeded).then(|| crate::hearth::check_mark(ink, (10.0 * zoom).clamp(9.0, 11.0)));
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
            .children(tick)
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
        let floor = if zoom >= DETAIL_ZOOM { 11.4 } else { 10.0 };
        text(if zoom >= LABEL_ZOOM { content } else { "" }).font(TextStyle::new((13.0 * zoom).clamp(floor, 13.5), weight)).color(color)
    };
    let mut r = row()
        .w_full()
        .h(SOCKET_ROW * zoom)
        .px((11.0 * zoom).max(9.0))
        .gap((6.0 * zoom).max(4.0))
        .items_center()
        .justify_between()
        .child(label(card.inputs.get(row_index)).truncate())
        .child(label(card.outputs.get(row_index)));
    if row_index > 0 && zoom >= DETAIL_ZOOM {
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
    let shelved = state.is_shelved(&card.id);
    let dimmed = (state.has_run() && card.status == ExecutionStatus::Idle && !card.selected) || shelved;
    let tone = category_tone(&card.category, state.light);
    let [x, y] = state.camera.world_to_screen(card.position);
    let card_w = card.size[0] * zoom;
    let card_h = card.size[1] * zoom;
    let node_id = card.id.clone();
    let dimmed_bg = if state.light { hex(0xEBE8F0) } else { hex(0x1D182B) };
    let dimmed_header = if state.light { hex(0xE0DCE8) } else { hex(0x262038) };
    let dimmed_border = p.untaken_border();
    let header_bg = if dimmed { dimmed_header } else { tone.tint };
    let header_h = CARD_HEADER * zoom;
    let title_size = (13.6 * zoom).clamp(11.8, 15.5);
    let kind_size = (10.8 * zoom).clamp(11.0, 11.6);
    let title_color = if dimmed { if state.light { hex(0x362E45) } else { hex(0xDFD8F2) } } else { p.ink };
    let kind_color = if dimmed { if state.light { hex(0x3F3652) } else { hex(0xD4CCE8) } } else { p.mute };
    let vial_size = (22.0 * zoom).clamp(17.0, 26.0);
    let display_title = state.display_title(&card.id);
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
                .children(
                    ((title_size + kind_size) * 1.25 + 4.0 <= header_h)
                        .then(|| text(&card.kind).font(TextStyle::new(kind_size, 500.0).mono()).color(kind_color).truncate()),
                ),
        );
    let perched_badge = status_box(card.status, card.duration_ms, p, zoom).or_else(|| {
        (shelved || (dimmed && state.brewed_clean())).then(|| {
            let badge_bg = p.untaken_pill_fill();
            let badge_ink = p.untaken_pill_ink();
            row()
                .px((6.0 * zoom).max(5.5))
                .py((1.8 * zoom).max(1.5))
                .radius((6.5 * zoom).max(6.0))
                .fill(badge_bg)
                .border(1.0, dimmed_border)
                .child(text(if shelved { "Set aside" } else { "Untaken" }).font(TextStyle::new((11.0 * zoom).clamp(11.0, 12.0), 650.0)).color(badge_ink))
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
            if canvas.running_brew {
                return;
            }
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
            (p.running_mark(), p.card, r)
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
    let hovered = (card.id.clone(), socket.name.clone(), output);
    let mut port = el()
        .key(format!("socket-{}-{side}-{}", card.id.0, socket.name))
        .size(size * zoom, size * zoom)
        .radius(radius)
        .fill(fill)
        .border((1.6 * zoom).max(1.0), border_color)
        .absolute((socket.offset[0] - size / 2.0) * zoom, (socket.offset[1] - size / 2.0) * zoom);
    if zoom < LABEL_ZOOM {
        let mine = hovered.clone();
        port = port.on_hover_change(move |s: &mut S, on, _| {
            let canvas = lens(s);
            if on {
                canvas.hover_socket = Some(mine.clone());
            } else if canvas.hover_socket.as_ref() == Some(&mine) {
                canvas.hover_socket = None;
            }
        });
    }
    if !output {
        if !wired {
            return port;
        }
        let into = SocketRef::new(card.id.clone(), socket.name.clone());
        return port.on_drag(move |s: &mut S, drag, tx| {
            tx.instant();
            let canvas = lens(s);
            if canvas.running_brew {
                return;
            }
            match drag.phase {
                Phase::Down => {
                    let Some(held) = canvas.wires.iter().find(|wire| wire.to_node == into.node && wire.to_socket == into.socket).cloned() else { return };
                    let base = canvas.card(&into.node).and_then(|card| card.socket_world(&into.socket, false)).map_or(held.to, |(point, _)| point);
                    canvas.drag = DragState::PullingWire { held, base, cursor: base, detached: false };
                }
                Phase::Move => {
                    let zoom = canvas.camera.zoom;
                    if let DragState::PullingWire { base, cursor, detached, .. } = &mut canvas.drag {
                        *cursor = [base[0] + drag.dx / zoom, base[1] + drag.dy / zoom];
                        *detached |= drag.dx.hypot(drag.dy) >= PULL_SLOP;
                    }
                }
                _ => {
                    if let DragState::PullingWire { held, cursor, detached: true, .. } = canvas.drag.clone()
                        && !drag.cancelled()
                    {
                        let from = SocketRef::new(held.from_node.clone(), held.from_socket.clone());
                        match canvas.input_socket_at(cursor) {
                            Some(target) if target == into => {}
                            Some(target) => {
                                let kept = canvas.wires.clone();
                                canvas.wires.retain(|wire| wire.id != held.id);
                                if let Err(refused) = canvas.connect_sockets(&from, &target) {
                                    canvas.wires = kept;
                                    canvas.last_connect_error = Some(refused);
                                }
                            }
                            None => canvas.wires.retain(|wire| wire.id != held.id),
                        }
                    }
                    canvas.drag = DragState::Idle;
                }
            }
        });
    }
    let from = SocketRef::new(card.id.clone(), socket.name.clone());
    let ty = socket.ty;
    let anchor = [card.position[0] + socket.offset[0], card.position[1] + socket.offset[1]];
    port.on_drag(move |s: &mut S, drag, tx| {
        tx.instant();
        let canvas = lens(s);
        if canvas.running_brew {
            return;
        }
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
