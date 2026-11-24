use magicklala_graph::NodeId;
use rune::{Beat, Color, El, LoopSpec, Phase, Sequence, Shape, TextStyle, canvas, el, row, text};

use crate::brew::{BrewCard, BrewPlan, BrewRole, MULTI_BOTTLE_SIZE, SINGLE_BOTTLE_SIZE, brew_plan};
use crate::hearth::{
    CauldronMood, HearthPalette, StreamStyle, brew_tint, category_tone, distillation_ribbon_shape, ghost_potion_bottle_shape, hero_cauldron_shape, pour_point,
    pouring_stream_shape, stream_ink, with_alpha,
};
use crate::view::{apothecary_dots, branch_bottle_view, cauldron_mood, ingredient_vial_card, mode_switcher, potion_stand_view};
use crate::{CanvasState, DragState, ExecutionStatus, NodeCardModel, WireModel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toss {
    IntoPot(NodeId),
    Aside(NodeId),
}

pub type TossHook<S> = fn(&mut S, Toss);

const STIRS: [f32; 5] = [0.0, -0.42, 0.3, -0.18, 0.5];

static SPLASH: [Beat; 3] = [Beat::new(0.0).scale(0.35).opacity(0.0), Beat::new(0.14).scale(0.95).opacity(1.0), Beat::new(0.8).scale(1.7).opacity(0.0)];

fn rim_point(plan: &BrewPlan, share: f32) -> [f32; 2] {
    let pot = plan.pot;
    [pot[0] + pot[2] * (0.30 + 0.40 * share), pot[1] + pot[3] * 0.44]
}

fn spout_point(plan: &BrewPlan) -> [f32; 2] {
    let pot = plan.pot;
    [pot[0] + pot[2] * 0.76, pot[1] + pot[3] * 0.42]
}

fn bottle_neck(card: &BrewCard, multi: bool) -> [f32; 2] {
    let rect = card.rect;
    let neck_share = if multi { 24.0 / MULTI_BOTTLE_SIZE[1] } else { 29.0 / SINGLE_BOTTLE_SIZE[1] };
    [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * neck_share]
}

fn stream_for(state: &CanvasState, card: &NodeCardModel, mood: CauldronMood, p: &HearthPalette, arch_lift: f32) -> StreamStyle {
    let tone = category_tone(&card.category, state.light);
    let hexed = card.status == ExecutionStatus::Failed || card.inputs.iter().any(|socket| socket.flagged);
    let blocked = mood == CauldronMood::Hexed && !hexed;
    let untaken = state.has_run() && card.status == ExecutionStatus::Idle && !hexed && !blocked;
    let active = matches!(card.status, ExecutionStatus::Succeeded | ExecutionStatus::Running);
    let idle = !active && !untaken && !blocked && !hexed;
    let color = if hexed {
        p.hex
    } else if blocked {
        p.mute
    } else if idle {
        stream_ink(&card.category, state.light)
    } else {
        tone.liquid
    };
    StreamStyle { color, glow: if hexed { p.hex } else { tone.glow }, active, idle, untaken, blocked, hexed, light: state.light, arch_lift }
}

fn pouring_cards<'a>(state: &'a CanvasState, plan: &'a BrewPlan) -> impl Iterator<Item = (&'a BrewCard, &'a NodeCardModel)> {
    plan.with_role(BrewRole::Ingredient)
        .filter_map(|placed| state.card(&placed.id).map(|card| (placed, card)))
        .filter(|(_, card)| !state.has_run() || matches!(card.status, ExecutionStatus::Succeeded | ExecutionStatus::Running))
}

pub fn potion_tint(state: &CanvasState, plan: &BrewPlan) -> Option<Color> {
    let liquids: Vec<Color> = pouring_cards(state, plan).map(|(_, card)| category_tone(&card.category, state.light).liquid).collect();
    brew_tint(&liquids)
}

fn bottle_tint(state: &CanvasState, plan: &BrewPlan, bottle: &NodeId) -> Option<Color> {
    let own: Vec<Color> = pouring_cards(state, plan)
        .filter(|(placed, _)| placed.feeds_only.as_ref() == Some(bottle))
        .map(|(_, card)| category_tone(&card.category, state.light).liquid)
        .collect();
    let all: Vec<Color> = pouring_cards(state, plan).map(|(_, card)| category_tone(&card.category, state.light).liquid).collect();
    brew_tint(&own).or_else(|| brew_tint(&all))
}

fn drips(state: &CanvasState, plan: &BrewPlan, mood: CauldronMood, p: &HearthPalette) -> Vec<El> {
    if matches!(mood, CauldronMood::Hexed | CauldronMood::Empty) {
        return Vec::new();
    }
    let paths = pour_paths(state, plan);
    let mut out = Vec::new();
    for (index, (id, points)) in paths.iter().enumerate() {
        let Some(card) = state.card(id) else { continue };
        let lift = (36.0 - index as f32 * 10.0).max(0.0) * plan.scale;
        let style = stream_for(state, card, mood, p, lift);
        if style.untaken || style.blocked || style.hexed || points.len() < 4 {
            continue;
        }
        let r = (if style.active { 3.4 } else { 2.6 }) * plan.scale.max(0.6);
        let period = if style.active { 0.9 } else { 1.8 };
        let third = points.len() / 3;
        for k in 0..3 {
            let start = points[k * third];
            let end = points[((k + 1) * third).min(points.len() - 1)];
            let delay = period * k as f64 / 3.0 + index as f64 * 0.17;
            let mut travel = LoopSpec::slide(period, [0.0, 0.0], [end[0] - start[0], end[1] - start[1]]);
            travel.delay = delay;
            let dot = el()
                .size(r * 2.0, r * 2.0)
                .radius(r)
                .fill(with_alpha(if style.active { style.glow } else { style.color }, 0.95))
                .border(1.0, with_alpha(p.card, 0.8))
                .looping(LoopSpec::opacity(period, delay, 0.2, 1.0));
            out.push(el().key(format!("drip-{}-{k}", id.0)).absolute(start[0] - r, start[1] - r).child(dot).looping(travel));
        }
    }
    out
}

fn vial_mouth(placed: &BrewCard, plan: &BrewPlan) -> [f32; 2] {
    let scale = placed.rect[3] / plan.vial[1];
    let badge = if plan.vial[1] < 50.0 { 30.0 } else { 38.0 };
    let center = [placed.rect[0] + placed.rect[2] - (9.0 + badge * 0.5) * scale, placed.rect[1] + placed.rect[3] * 0.5];
    [center[0] + badge * 0.22 * scale, center[1] - badge * 0.36 * scale]
}

pub fn pour_paths(state: &CanvasState, plan: &BrewPlan) -> Vec<(NodeId, Vec<[f32; 2]>)> {
    let ingredients: Vec<&BrewCard> = plan.with_role(BrewRole::Ingredient).filter(|placed| state.card(&placed.id).is_some()).collect();
    let spread = ingredients.len().max(2) as f32 - 1.0;
    let mut columns: Vec<f32> = ingredients.iter().map(|placed| placed.rect[0]).collect();
    columns.sort_by(f32::total_cmp);
    columns.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    ingredients
        .iter()
        .enumerate()
        .map(|(index, placed)| {
            let from = vial_mouth(placed, plan);
            let points = match plan.lanes {
                Some([shelf_bottom, gap]) => {
                    let column = columns.iter().position(|x| (x - placed.rect[0]).abs() < 1.0).unwrap_or(0);
                    let to = rim_point(plan, column as f32 / (columns.len().max(2) - 1) as f32);
                    let lane_x = placed.rect[0] + placed.rect[2] + gap * 0.5;
                    let turn = [lane_x, shelf_bottom + gap];
                    let mut points: Vec<[f32; 2]> = (0..=4).map(|step| [from[0] + (lane_x - from[0]) * step as f32 / 4.0, from[1]]).collect();
                    points.extend((1..=12).map(|step| [lane_x, from[1] + (turn[1] - from[1]) * step as f32 / 12.0]));
                    points.extend((1..=16).map(|step| {
                        let t = step as f32 / 16.0;
                        let c1 = [lane_x, turn[1] + 4.0 * plan.scale];
                        let c2 = [to[0] + (lane_x - to[0]) * 0.15, to[1] - 70.0 * plan.scale];
                        let u = 1.0 - t;
                        [
                            u * u * u * turn[0] + 3.0 * u * u * t * c1[0] + 3.0 * u * t * t * c2[0] + t * t * t * to[0],
                            u * u * u * turn[1] + 3.0 * u * u * t * c1[1] + 3.0 * u * t * t * c2[1] + t * t * t * to[1],
                        ]
                    }));
                    points
                }
                None => {
                    let to = rim_point(plan, index as f32 / spread.max(1.0));
                    let lift = (36.0 - index as f32 * 10.0).max(0.0) * plan.scale;
                    (0..=40).map(|step| pour_point(from, to, lift, step as f32 / 40.0)).collect()
                }
            };
            (placed.id.clone(), points)
        })
        .collect()
}

fn lane_stream(shape: Shape, points: &[[f32; 2]], style: StreamStyle, p: &HearthPalette) -> Shape {
    let Some((first, rest)) = points.split_first() else { return shape };
    let path = rest.iter().fold(rune::Path::new().move_to(*first), |path, point| path.line_to(*point));
    let ink = if style.untaken || style.blocked {
        p.untaken_ink()
    } else if style.hexed {
        p.hex
    } else {
        style.color
    };
    let core = if style.active { 3.2 } else { 2.4 };
    let shape = shape.stroke_path(
        &path,
        core + 4.0,
        rune::LineCap::Round,
        with_alpha(if style.active { style.glow } else { ink }, if style.active { 0.32 } else { 0.16 }),
    );
    shape.stroke_path(&path, core, rune::LineCap::Round, ink)
}

fn streams_shape(state: &CanvasState, plan: &BrewPlan, mood: CauldronMood, p: &HearthPalette) -> Shape {
    let mut shape = Shape::new();
    let ingredients: Vec<&BrewCard> = plan.with_role(BrewRole::Ingredient).collect();
    let bottles: Vec<&BrewCard> = plan.with_role(BrewRole::Bottle).collect();
    let multi = bottles.len() > 1;
    let spread = ingredients.len().max(2) as f32 - 1.0;
    let paths = pour_paths(state, plan);
    for (index, placed) in ingredients.iter().enumerate() {
        let Some(card) = state.card(&placed.id) else { continue };
        let lift = (36.0 - index as f32 * 10.0).max(0.0) * plan.scale;
        let style = stream_for(state, card, mood, p, lift);
        shape = match (plan.lanes, paths.iter().find(|(id, _)| id == &placed.id)) {
            (Some(_), Some((_, points))) => lane_stream(shape, points, style, p),
            _ => pouring_stream_shape(shape, vial_mouth(placed, plan), rim_point(plan, index as f32 / spread.max(1.0)), style),
        };
    }
    let spout = spout_point(plan);
    for placed in &bottles {
        let Some(card) = state.card(&placed.id) else { continue };
        let taken = matches!(card.status, ExecutionStatus::Succeeded | ExecutionStatus::Running);
        let untaken = state.has_run() && !taken;
        let ribbon_mood = if multi && taken { CauldronMood::Brewed } else { mood };
        shape =
            distillation_ribbon_shape(shape, spout, bottle_neck(placed, multi), ribbon_mood, state.light, taken, untaken, bottle_tint(state, plan, &placed.id));
    }
    if let Some(ghost) = plan.ghost_bottle {
        shape = distillation_ribbon_shape(
            shape,
            spout,
            [ghost[0] + ghost[2] * 0.5, ghost[1] + ghost[3] * 0.15],
            CauldronMood::Empty,
            state.light,
            false,
            false,
            None,
        );
    }
    shape
}

pub(crate) fn route_label(state: &CanvasState, bottle: &NodeId) -> Option<String> {
    state
        .wires
        .iter()
        .find(|wire: &&WireModel| &wire.to_node == bottle && wire.ty.is_flow())
        .map(|wire| wire.from_socket.clone())
        .filter(|socket| socket != "out")
}

fn bubbles(mood: CauldronMood, plan: &BrewPlan, p: &HearthPalette, tint: Option<Color>) -> Vec<El> {
    let froth = tint.map_or(p.broth_foam, |t| crate::hearth::mix(t, crate::hearth::hex(0xFFFFFF), 0.5));
    let edge = tint.unwrap_or(p.broth_mid);
    if matches!(mood, CauldronMood::Empty | CauldronMood::Hexed) {
        return Vec::new();
    }
    let lively = matches!(mood, CauldronMood::Simmering | CauldronMood::Brewed);
    let count = if lively { 7 } else { 3 };
    let rise = (if lively { 88.0 } else { 52.0 }) * plan.scale;
    let rim = rim_point(plan, 0.5);
    (0..count)
        .map(|index| {
            let share = (index as f32 + 0.5) / count as f32;
            let radius = (3.0 + (index % 3) as f32 * 1.8) * plan.scale.max(0.5);
            let x = plan.pot[0] + plan.pot[2] * (0.30 + 0.40 * share) - radius;
            let period = if lively { 1.6 + (index % 4) as f64 * 0.35 } else { 3.2 + (index % 3) as f64 * 0.6 };
            let mut spec = LoopSpec::slide(period, [0.0, 0.0], [((index % 2) as f32 - 0.5) * 6.0, -rise]);
            spec.delay = index as f64 * 0.27;
            let dot = el()
                .size(radius * 2.0, radius * 2.0)
                .radius(radius)
                .fill(with_alpha(froth, if lively { 0.88 } else { 0.55 }))
                .border(1.0, with_alpha(edge, 0.6))
                .looping(LoopSpec::opacity(period, spec.delay, 0.0, 1.0));
            el().key(format!("pot-bubble-{index}")).absolute(x, rim[1] - radius * 2.0).child(dot).looping(spec)
        })
        .collect()
}

fn splash(state: &CanvasState, plan: &BrewPlan, p: &HearthPalette) -> Option<El> {
    if state.toss_count == 0 {
        return None;
    }
    let rim = rim_point(plan, 0.5);
    let w = plan.pot[2] * 0.62;
    let ring = el().size(w, w * 0.32).radius(w * 0.16).border(2.4 * plan.scale.max(0.5), with_alpha(p.gold_hi, 0.9)).sequence(Sequence::new(&SPLASH));
    Some(el().key(format!("pot-splash-{}", state.toss_count)).absolute(rim[0] - w * 0.5, rim[1] - w * 0.16).child(ring))
}

fn plaque_text(state: &CanvasState, mood: CauldronMood, plan: &BrewPlan) -> String {
    if let Some(summary) = &state.brew_summary {
        return summary.clone();
    }
    let ingredients = plan.with_role(BrewRole::Ingredient).count();
    let word = if ingredients == 1 { "ingredient" } else { "ingredients" };
    let aside = plan.with_role(BrewRole::Aside).count();
    let aside_part = if aside == 0 { String::new() } else { format!(" · {aside} aside") };
    match mood {
        CauldronMood::Empty => "Empty cauldron".to_owned(),
        CauldronMood::Hexed if state.last_brew == Some(magicklala_graph::RunStatus::TimedOut) => "Timed out".to_owned(),
        CauldronMood::Hexed if state.last_brew == Some(magicklala_graph::RunStatus::Failed) => "Failed".to_owned(),
        CauldronMood::Hexed => "Hexed".to_owned(),
        CauldronMood::Ready if state.last_brew == Some(magicklala_graph::RunStatus::Cancelled) => format!("Cancelled · {ingredients} {word}{aside_part}"),
        CauldronMood::Simmering => format!("Brewing · {ingredients} {word}"),
        CauldronMood::Brewed => {
            let ms: u64 = state.nodes.iter().filter_map(|card| card.duration_ms).sum();
            let time = if ms == 0 { "<1ms".to_owned() } else { format!("{ms}ms") };
            let bottles: Vec<&BrewCard> = plan.with_role(BrewRole::Bottle).collect();
            let filled = bottles.iter().filter(|placed| state.card(&placed.id).is_some_and(|card| card.status == ExecutionStatus::Succeeded)).count();
            match (bottles.len(), filled) {
                (0 | 1, 0) => format!("Brewed · {time} · bottle empty"),
                (0 | 1, _) => format!("Brewed · {time}"),
                (count, _) => format!("Brewed · {time} · {filled} of {count} bottles"),
            }
        }
        CauldronMood::Ready => match &state.release_badge {
            Some(badge) => format!("Published {badge} · {ingredients} {word}{aside_part}"),
            None if state.validated => format!("Checked · {ingredients} {word}{aside_part}"),
            None => format!("Draft · {ingredients} {word}{aside_part}"),
        },
    }
}

fn plaque(state: &CanvasState, mood: CauldronMood, plan: &BrewPlan, p: &HearthPalette) -> El {
    let dot = match mood {
        CauldronMood::Hexed => p.hex,
        CauldronMood::Simmering => p.gold,
        CauldronMood::Brewed => p.broth_mid,
        CauldronMood::Ready if state.validated || state.release_badge.is_some() => p.broth_mid,
        CauldronMood::Ready => p.gold,
        CauldronMood::Empty => p.mute,
    };
    let hexed = mood == CauldronMood::Hexed;
    let width = 360.0;
    let [cx, _] = plan.pot_center();
    row().w(width).justify_center().absolute(cx - width * 0.5, plan.plaque_top).child(
        row()
            .key("cauldron-plaque")
            .h(28.0)
            .px(12.0)
            .gap(7.0)
            .radius(14.0)
            .continuous_corners()
            .items_center()
            .fill(if hexed { p.hex_tint } else { p.card })
            .border(1.2, if hexed { p.hex } else { p.rule_2 })
            .shadow(3.0, 10.0, p.shadow_card)
            .child(el().size(7.0, 7.0).radius(3.5).fill(dot))
            .child(text(plaque_text(state, mood, plan)).font(TextStyle::new(12.0, 650.0)).color(if hexed { p.hex } else { p.ink })),
    )
}

fn legend(state: &CanvasState, mood: CauldronMood, plan: &BrewPlan, size: (f32, f32), p: &HearthPalette) -> Option<El> {
    let untaken = state.has_run() && state.nodes.iter().any(|card| card.status == ExecutionStatus::Idle);
    let hexed = mood == CauldronMood::Hexed;
    let aside = plan.with_role(BrewRole::Aside).next().is_some();
    if !untaken && !hexed && !aside {
        return None;
    }
    let dash = |color: Color| row().gap(3.0).items_center().child(el().w(6.0).h(2.0).fill(color)).child(el().w(6.0).h(2.0).fill(color));
    let mut keyed = row()
        .key("canvas-legend")
        .gap(12.0)
        .px(11.0)
        .py(6.0)
        .radius(10.0)
        .continuous_corners()
        .items_center()
        .fill(p.card)
        .border(1.0, p.rule)
        .absolute(14.0, (size.1 - 38.0).max(12.0));
    if hexed {
        keyed = keyed
            .child(row().gap(5.0).items_center().child(dash(p.hex)).child(text("Hexed").font(TextStyle::new(11.0, 650.0)).color(p.hex)))
            .child(row().gap(5.0).items_center().child(dash(p.mute)).child(text("Blocked").font(TextStyle::new(11.0, 600.0)).color(p.ink_2)));
    } else if untaken {
        keyed = keyed.child(row().gap(5.0).items_center().child(dash(p.mute)).child(text("Untaken").font(TextStyle::new(11.0, 600.0)).color(p.ink_2)));
    }
    if aside {
        keyed = keyed.child(
            row()
                .gap(5.0)
                .items_center()
                .child(el().size(12.0, 8.0).radius(3.0).border(1.2, p.rule_2))
                .child(text("Set aside").font(TextStyle::new(11.0, 600.0)).color(p.ink_2)),
        );
    }
    Some(keyed)
}

fn lifted_offset(state: &CanvasState, id: &NodeId) -> [f32; 2] {
    match &state.drag {
        DragState::LiftingVial { node, offset } if node == id => *offset,
        _ => [0.0, 0.0],
    }
}

fn card_frame(rect: [f32; 4], natural: [f32; 2], scale: f32, offset: [f32; 2]) -> [f32; 2] {
    [rect[0] + rect[2] * 0.5 - natural[0] * 0.5 + offset[0], rect[1] + rect[3] * 0.5 - natural[1] * 0.5 + offset[1]]
        .map(|v| if scale.is_finite() { v } else { 0.0 })
}

pub fn cauldron_workbench_view<S: 'static>(state: &CanvasState, size: (f32, f32), lens: fn(&mut S) -> &mut CanvasState, toss: TossHook<S>) -> El {
    let p = HearthPalette::for_theme(state.light);
    let mood = cauldron_mood(state);
    let plan = brew_plan(state, size);
    let [pcx, pcy] = plan.pot_center();
    let has_run = state.has_run();

    let glow_color = match mood {
        CauldronMood::Hexed => with_alpha(p.hex, if state.light { 0.12 } else { 0.18 }),
        CauldronMood::Empty => Color::TRANSPARENT,
        CauldronMood::Ready => with_alpha(p.broth_mid, 0.10),
        CauldronMood::Simmering | CauldronMood::Brewed => with_alpha(p.broth_foam, if state.light { 0.24 } else { 0.20 }),
    };
    let radius = plan.pour_radius();
    let hearth_glow = el()
        .size(size.0, size.1)
        .radial_gradient(
            [(pcx / size.0).clamp(0.0, 1.0), (pcy / size.1).clamp(0.0, 1.0)],
            [radius * 1.45, radius * 1.25],
            [(0.0, glow_color), (0.65, with_alpha(glow_color, 0.04)), (1.0, Color::TRANSPARENT)],
        )
        .absolute(0.0, 0.0);
    let backdrop = canvas(apothecary_dots(state, size, &p, [pcx, pcy], radius)).key("canvas-dots").size(size.0, size.1).absolute(0.0, 0.0);
    let streams = canvas(streams_shape(state, &plan, mood, &p)).key("canvas-wires").size(size.0, size.1).absolute(0.0, 0.0);
    let pot = plan.pot;
    let cauldron = el()
        .key("cauldron-center")
        .size(pot[2], pot[3])
        .absolute(pot[0], pot[1])
        .child(
            canvas(hero_cauldron_shape(mood, state.light, pot[2], pot[3], potion_tint(state, &plan), STIRS[state.toss_count as usize % STIRS.len()]))
                .key("hero-cauldron")
                .size(pot[2], pot[3]),
        )
        .on_press(move |s: &mut S| lens(s).select(None));

    let hits: Vec<(NodeId, [f32; 4])> = plan.cards.iter().map(|card| (card.id.clone(), card.rect)).collect();
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
                    canvas.drag = DragState::Panning { origin: [canvas.brew_camera.pan_x, canvas.brew_camera.pan_y] };
                }
                Phase::Move => {
                    if let DragState::Panning { origin } = canvas.drag {
                        canvas.brew_camera.pan_x = origin[0] + drag.dx;
                        canvas.brew_camera.pan_y = origin[1] + drag.dy;
                    }
                }
                Phase::Up => canvas.drag = DragState::Idle,
            }
        })
        .on_secondary_press_local(move |s: &mut S, press, tx| {
            tx.instant();
            let canvas = lens(s);
            let hit = hits.iter().rev().find(|(_, r)| press.x >= r[0] && press.x <= r[0] + r[2] && press.y >= r[1] && press.y <= r[1] + r[3]);
            canvas.context_card = hit.map(|(_, r)| *r);
            canvas.select(hit.map(|(id, _)| id.clone()));
            canvas.context_menu = Some([press.x, press.y]);
        })
        .child(hearth_glow)
        .child(backdrop)
        .child(streams)
        .child(cauldron);
    for drip in drips(state, &plan, mood, &p) {
        board = board.child(drip);
    }
    for bubble in bubbles(mood, &plan, &p, potion_tint(state, &plan)) {
        board = board.child(bubble);
    }
    let lifted = match &state.drag {
        DragState::LiftingVial { node, offset } => {
            plan.card(node).map(|placed| (placed.role, [placed.rect[0] + placed.rect[2] * 0.5 + offset[0], placed.rect[1] + placed.rect[3] * 0.5 + offset[1]]))
        }
        _ => None,
    };
    if let Some((BrewRole::Ingredient, point)) = lifted {
        let zone = plan.aside_zone;
        let inside = plan.is_in_aside_zone(point);
        board = board.child(
            row()
                .key("aside-tray")
                .size(zone[2] - 28.0, zone[3] - 14.0)
                .absolute(zone[0] + 14.0, zone[1])
                .radius(18.0)
                .continuous_corners()
                .justify_center()
                .items_center()
                .fill(if inside { with_alpha(p.gold_hi, 0.14) } else { with_alpha(p.card, 0.55) })
                .border(if inside { 2.0 } else { 1.4 }, if inside { p.simmer_ink() } else { p.rule_2 })
                .child(text("Set aside").font(TextStyle::new(12.0, 700.0)).color(if inside { p.simmer_ink() } else { p.ink_2 })),
        );
    }
    let lifted_over_pot = matches!(lifted, Some((BrewRole::Aside, point)) if plan.is_over_pot(point));
    if state.pot_hover || lifted_over_pot {
        let w = plan.pot[2] * 0.86;
        let [hx, hy] = rim_point(&plan, 0.5);
        board = board.child(
            el().key("pot-hover")
                .size(w, w * 0.36)
                .radius(w * 0.18)
                .border(3.0 * plan.scale.max(0.5), with_alpha(p.gold_hi, 0.95))
                .shadow(0.0, 18.0, with_alpha(p.gold_hi, 0.55))
                .absolute(hx - w * 0.5, hy - w * 0.18)
                .looping(LoopSpec::opacity(0.9, 0.0, 0.55, 1.0)),
        );
    }
    if let Some(ring) = splash(state, &plan, &p) {
        board = board.child(ring);
    }
    if !state.nodes.is_empty() {
        board = board.child(plaque(state, mood, &plan, &p));
    }

    let bottles: Vec<&BrewCard> = plan.with_role(BrewRole::Bottle).collect();
    let multi = bottles.len() > 1;
    let aside_rects: Vec<[f32; 4]> = plan.with_role(BrewRole::Aside).map(|card| card.rect).collect();
    if let Some(first) = aside_rects.first() {
        let pad = 12.0 * plan.scale;
        let head = 24.0 * plan.scale.max(0.8);
        let left = aside_rects.iter().map(|r| r[0]).fold(first[0], f32::min) - pad;
        let top = aside_rects.iter().map(|r| r[1]).fold(first[1], f32::min) - pad - head;
        let right = aside_rects.iter().map(|r| r[0] + r[2]).fold(first[0] + first[2], f32::max) + pad;
        let bottom = aside_rects.iter().map(|r| r[1] + r[3]).fold(first[1] + first[3], f32::max) + pad;
        board = board.child(
            el().key("aside-shelf")
                .size(right - left, bottom - top)
                .absolute(left, top)
                .radius(16.0)
                .continuous_corners()
                .fill(with_alpha(p.wash_2, 0.75))
                .border(1.2, p.rule_2)
                .child(row().absolute(12.0, 5.0).child(text("Set aside").font(TextStyle::new(11.0, 700.0)).color(p.ink_2))),
        );
    }
    for placed in plan.cards.iter().filter(|card| card.role != BrewRole::Bottle) {
        let Some(card) = state.card(&placed.id) else { continue };
        let step = if placed.role == BrewRole::Aside {
            None
        } else if has_run {
            card.step_order
        } else {
            placed.recipe_step
        };
        let origin = card_frame(placed.rect, plan.vial, plan.scale, lifted_offset(state, &placed.id));
        let vial = ingredient_vial_card(state, card, step, origin, plan.vial, placed.role == BrewRole::Aside, &p, lens, toss, plan.clone());
        board = board.child(vial.scale(plan.scale));
    }
    for placed in &bottles {
        let Some(card) = state.card(&placed.id) else { continue };
        let step = if has_run { card.step_order } else { placed.recipe_step };
        let natural = if multi { MULTI_BOTTLE_SIZE } else { SINGLE_BOTTLE_SIZE };
        let origin = card_frame(placed.rect, natural, plan.scale, [0.0, 0.0]);
        let view = if multi {
            branch_bottle_view(state, card, step, [0.0, 0.0, natural[0]], bottle_tint(state, &plan, &placed.id), has_run, &p, lens)
        } else {
            potion_stand_view(state, card, step, [0.0, 0.0, natural[0]], potion_tint(state, &plan), mood, &p, lens)
        };
        board =
            board.child(el().key(format!("node-{}", placed.id.0)).size(natural[0], natural[1]).absolute(origin[0], origin[1]).child(view).scale(plan.scale));
    }
    if let Some(ghost) = plan.ghost_bottle {
        board = board.child(
            el().key("ghost-bottle")
                .size(ghost[2], ghost[3])
                .absolute(ghost[0], ghost[1])
                .child(canvas(ghost_potion_bottle_shape(state.light, ghost[2], ghost[3] * 0.6)).size(ghost[2], ghost[3] * 0.6)),
        );
    }
    board = board.child(mode_switcher(state, &p, lens, size));
    if let Some(keyed) = legend(state, mood, &plan, size, &p) {
        board = board.child(keyed);
    }
    board
}
