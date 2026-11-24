use magicklala_graph::NodeId;
use rune::{Beat, Color, El, LineCap, LoopSpec, Path, Phase, Sequence, Shape, TextStyle, canvas, el, row, text};

use crate::brew::{BrewCard, BrewPlan, BrewRole, MULTI_BOTTLE_SIZE, SINGLE_BOTTLE_SIZE, brew_plan};
use crate::hearth::{
    CauldronMood, HearthPalette, StreamStyle, brew_tint, category_tone, distillation_ribbon_shape, fill_circle, ghost_potion_bottle_shape, hero_cauldron_shape,
    pour_point, pouring_stream_shape, stream_ink, with_alpha,
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
    let (cx, cy, u) = pot_frame(plan.pot);
    [cx + 94.0 * u, cy + 30.0 * u]
}

fn pot_frame(pot: [f32; 4]) -> (f32, f32, f32) {
    (pot[0] + pot[2] * 0.5, pot[1] + pot[3] * 0.56, (pot[2].min(pot[3]) / 240.0).clamp(0.55, 1.35))
}

fn tap_shape(size: [f32; 2], p: &HearthPalette) -> Shape {
    let (cx, cy, u) = pot_frame([0.0, 0.0, size[0], size[1]]);
    let mut shape = Shape::new();
    let pipe = Path::new().move_to([cx + 66.0 * u, cy + 30.0 * u]).line_to([cx + 91.0 * u, cy + 30.0 * u]);
    shape = shape.stroke_path(&pipe, 8.4 * u, LineCap::Round, p.iron_rim);
    shape = shape.stroke_path(&pipe, 6.2 * u, LineCap::Round, p.gold);
    shape = shape.stroke_path(&pipe, 2.0 * u, LineCap::Round, p.gold_hi);
    let stem = Path::new().move_to([cx + 83.0 * u, cy + 27.0 * u]).line_to([cx + 83.0 * u, cy + 20.5 * u]);
    shape = shape.stroke_path(&stem, 2.6 * u, LineCap::Round, p.gold);
    let key = Path::new().move_to([cx + 78.5 * u, cy + 20.0 * u]).line_to([cx + 87.5 * u, cy + 20.0 * u]);
    shape = shape.stroke_path(&key, 3.4 * u, LineCap::Round, p.iron_rim);
    shape = shape.stroke_path(&key, 1.6 * u, LineCap::Round, p.gold_hi);
    shape = fill_circle(shape, cx + 67.5 * u, cy + 30.0 * u, 6.0 * u, p.iron_rim);
    shape = fill_circle(shape, cx + 67.5 * u, cy + 30.0 * u, 4.6 * u, p.gold);
    shape = fill_circle(shape, cx + 66.4 * u, cy + 28.9 * u, 1.5 * u, p.gold_hi);
    shape = fill_circle(shape, cx + 92.5 * u, cy + 30.0 * u, 4.6 * u, p.iron_rim);
    fill_circle(shape, cx + 92.5 * u, cy + 30.0 * u, 3.4 * u, p.gold_hi)
}

fn cubic_samples(from: [f32; 2], c1: [f32; 2], c2: [f32; 2], to: [f32; 2], steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|step| {
            let t = step as f32 / steps as f32;
            let u = 1.0 - t;
            [
                u * u * u * from[0] + 3.0 * u * u * t * c1[0] + 3.0 * u * t * t * c2[0] + t * t * t * to[0],
                u * u * u * from[1] + 3.0 * u * u * t * c1[1] + 3.0 * u * t * t * c2[1] + t * t * t * to[1],
            ]
        })
        .collect()
}

fn lane_route(from: [f32; 2], to: [f32; 2], lane: f32, s: f32) -> Vec<[f32; 2]> {
    let rising = to[1] - 10.0 * s < from[1];
    let turn = [lane, if rising { to[1] - 6.0 * s } else { to[1] - 24.0 * s }];
    let bend = ((from[1] - turn[1]).abs() * 0.5).min(48.0 * s);
    let approach = [lane, if rising { turn[1] + bend } else { turn[1] - bend }];
    let mut points = cubic_samples(from, [from[0] + (lane - from[0]) * 0.7, from[1]], approach, turn, 28);
    points.pop();
    let heading = [turn[0] - approach[0], turn[1] - approach[1]];
    let length = heading[0].hypot(heading[1]).max(0.01);
    let carry = [turn[0] + heading[0] / length * 12.0 * s, turn[1] + heading[1] / length * 12.0 * s];
    points.extend(cubic_samples(turn, carry, [to[0] - 6.0 * s, to[1] - 16.0 * s], to, 16));
    points
}

fn over_route(from: [f32; 2], to: [f32; 2], ceiling: f32, climb_x: f32, s: f32) -> Vec<[f32; 2]> {
    let corner = 22.0 * s;
    let rise = [climb_x.clamp(from[0] + corner, (to[0] - corner * 2.0).max(from[0] + corner)), ceiling];
    let fall = [to[0] - corner, ceiling];
    let mut points = cubic_samples(from, [from[0] + (rise[0] - from[0]) * 0.55, from[1]], [from[0] + (rise[0] - from[0]) * 0.45, ceiling], rise, 18);
    points.pop();
    let run = ((fall[0] - rise[0]).max(0.0) / (6.0 * s)).ceil().max(1.0) as usize;
    points.extend((0..run).map(|step| [rise[0] + (fall[0] - rise[0]) * step as f32 / run as f32, ceiling]));
    points.extend(cubic_samples(fall, [to[0] - corner * 0.35, ceiling], [to[0], ceiling + (to[1] - ceiling) * 0.45], to, 14));
    points
}

fn ribbon_route(plan: &BrewPlan, to: [f32; 2], own: Option<&NodeId>) -> Vec<[f32; 2]> {
    let tap = spout_point(plan);
    let s = plan.scale.max(0.4);
    let nest = own.and_then(|id| plan.card(id)).map_or(0, |mine| plan.with_role(BrewRole::Bottle).filter(|other| other.rect[0] > mine.rect[0]).count());
    let lip_steps = 4 + nest * 4;
    let lip: Vec<[f32; 2]> = (0..lip_steps).map(|step| [tap[0] + step as f32 * 4.0 * s, tap[1]]).collect();
    let mut route = ribbon_course(plan, [tap[0] + lip_steps as f32 * 4.0 * s, tap[1]], to, own, s);
    route.splice(0..0, lip);
    route
}

fn ribbon_course(plan: &BrewPlan, from: [f32; 2], to: [f32; 2], own: Option<&NodeId>, s: f32) -> Vec<[f32; 2]> {
    let own_tag = own.and_then(|id| plan.card(id)).map(|card| {
        let r = card.rect;
        [r[0] - 20.0 * s, r[1] + r[3] * 0.46 - 8.0 * s, r[2] + 28.0 * s, r[3] * 0.54 + 8.0 * s]
    });
    let inside = |r: [f32; 4], points: &[[f32; 2]]| {
        points.iter().filter(|point| point[0] >= r[0] && point[0] <= r[0] + r[2] && point[1] >= r[1] && point[1] <= r[1] + r[3]).count()
    };
    let hits = |points: &[[f32; 2]]| -> usize {
        let body = &points[..points.len().saturating_sub(points.len() / 8)];
        plan.cards.iter().filter(|card| Some(&card.id) != own).map(|card| inside(card.rect, body)).sum::<usize>() + own_tag.map_or(0, |tag| inside(tag, points))
    };
    let overshoot = |points: &[[f32; 2]]| -> u32 {
        let top = points.iter().map(|point| point[1]).fold(f32::MAX, f32::min);
        (to[1].min(from[1]) - top - 20.0 * s).max(0.0).round() as u32
    };
    let dx = to[0] - from[0];
    let direct = cubic_samples(from, [from[0] + dx * 0.4, from[1]], [to[0], to[1] - 16.0 * s], to, 44);
    let blocking: Vec<[f32; 4]> = plan.cards.iter().filter(|card| Some(&card.id) != own).map(|card| card.rect).filter(|r| inside(*r, &direct) > 0).collect();
    let lanes_floor = plan.lanes.map_or(f32::MIN, |[shelf_bottom, band]| shelf_bottom + band + 6.0 * s);
    let nest = own.and_then(|id| plan.card(id)).map_or(0, |mine| plan.with_role(BrewRole::Bottle).filter(|other| other.rect[0] > mine.rect[0]).count()) as f32;
    let lowest_top = blocking.iter().map(|rect| rect[1]).fold(to[1] - 16.0 * s, f32::min);
    let ceiling = (lowest_top - 6.0 * s).max(lanes_floor);
    let shelf_left = plan.with_role(BrewRole::Bottle).map(|card| card.rect[0]).fold(to[0], f32::min);
    let climb_x = blocking.iter().map(|rect| rect[0]).fold(shelf_left, f32::min) - 18.0 * s + nest * 18.0 * s;
    let mut candidates = vec![direct];
    for (lead, tuck, lift) in [(0.3, 1.0, 10.0), (0.5, 1.0, 10.0), (0.2, 1.6, 14.0), (0.45, 0.0, 12.0), (0.15, 2.4, 6.0)] {
        candidates.push(cubic_samples(from, [from[0] + dx * lead, from[1]], [to[0] - 22.0 * s * tuck, to[1] - lift * s], to, 44));
    }
    candidates.push(over_route(from, to, ceiling, climb_x, s));
    let own_left = own.and_then(|id| plan.card(id)).map(|card| card.rect[0]);
    let column_left = plan.with_role(BrewRole::Bottle).map(|card| card.rect[0]).reduce(f32::min);
    let outer_rank = plan.with_role(BrewRole::Bottle).count().saturating_sub(1) as f32 - nest;
    for lane in [own_left, column_left].into_iter().flatten().map(|x| x - 14.0 * s - outer_rank.max(0.0) * 12.0 * s).filter(|x| *x > from[0] + 8.0 * s) {
        candidates.push(lane_route(from, to, lane, s));
    }
    let sharp = |points: &[[f32; 2]]| -> bool {
        let steps: Vec<[f32; 2]> = std::iter::once([1.0, 0.0])
            .chain(points.windows(2).map(|pair| [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]]))
            .filter(|step| step[0].hypot(step[1]) > 0.01)
            .collect();
        steps.windows(2).any(|pair| {
            let dot = pair[0][0] * pair[1][0] + pair[0][1] * pair[1][1];
            dot / (pair[0][0].hypot(pair[0][1]) * pair[1][0].hypot(pair[1][1])) < 0.34
        })
    };
    candidates
        .into_iter()
        .enumerate()
        .min_by_key(|(order, points)| (hits(points), sharp(points), overshoot(points), *order))
        .map(|(_, points)| points)
        .unwrap_or_default()
}
fn landing(plan: &BrewPlan, placed: &BrewCard, index: usize, spread: f32) -> ([f32; 2], f32) {
    if !plan.flank {
        return (rim_point(plan, index as f32 / spread.max(1.0)), (36.0 - index as f32 * 10.0).max(0.0) * plan.scale);
    }
    let side: Vec<&BrewCard> = plan.with_role(BrewRole::Ingredient).filter(|other| other.mirrored == placed.mirrored).collect();
    let rank = side.iter().filter(|other| other.rect[1] < placed.rect[1] - 0.5).count();
    let depth = rank as f32 / (side.len().max(2) - 1) as f32;
    let share = if placed.mirrored { 0.58 + 0.42 * depth } else { 0.42 * (1.0 - depth) };
    (rim_point(plan, share), (1.0 - depth) * 36.0 * plan.scale)
}

pub fn ribbon_paths(plan: &BrewPlan) -> Vec<Vec<[f32; 2]>> {
    let bottles: Vec<&BrewCard> = plan.with_role(BrewRole::Bottle).collect();
    let multi = bottles.len() > 1;
    let ghost = plan.ghost_bottle.map(|ghost| ribbon_route(plan, ghost_neck(ghost), None));
    bottles.iter().map(|placed| ribbon_route(plan, bottle_neck(placed, multi), Some(&placed.id))).chain(ghost).collect()
}

fn ghost_neck(ghost: [f32; 4]) -> [f32; 2] {
    [ghost[0] + ghost[2] * 0.5, ghost[1] + ghost[3] * 0.15]
}

fn bottle_neck(card: &BrewCard, multi: bool) -> [f32; 2] {
    let rect = card.rect;
    let neck_share = if multi { 17.8 / MULTI_BOTTLE_SIZE[1] } else { 28.0 / SINGLE_BOTTLE_SIZE[1] };
    [rect[0] + rect[2] * 0.474, rect[1] + rect[3] * neck_share]
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

pub fn bottle_filled(state: &CanvasState, bottle: &NodeCardModel) -> bool {
    bottle.status == ExecutionStatus::Succeeded || (!state.has_run() && state.last_brew.is_none() && cauldron_mood(state) == CauldronMood::Brewed)
}

pub fn potion_tint(state: &CanvasState, plan: &BrewPlan) -> Option<Color> {
    if state.has_run()
        && let Some(last) = pouring_cards(state, plan).filter(|(_, card)| card.step_order.is_some()).max_by_key(|(_, card)| card.step_order)
    {
        return Some(category_tone(&last.1.category, state.light).liquid);
    }
    let liquids: Vec<Color> = pouring_cards(state, plan).map(|(_, card)| category_tone(&card.category, state.light).liquid).collect();
    brew_tint(&liquids)
}

pub fn bottle_tint(state: &CanvasState, plan: &BrewPlan, bottle: &NodeId) -> Option<Color> {
    if state.has_run() {
        let ran = || pouring_cards(state, plan).filter(|(_, card)| card.step_order.is_some());
        if let Some((_, last)) = ran()
            .filter(|(placed, _)| placed.feeds_only.as_ref() == Some(bottle))
            .max_by_key(|(_, card)| card.step_order)
            .or_else(|| ran().max_by_key(|(_, card)| card.step_order))
        {
            return Some(category_tone(&last.category, state.light).liquid);
        }
    }
    let own: Vec<Color> = pouring_cards(state, plan)
        .filter(|(placed, _)| placed.feeds_only.as_ref() == Some(bottle))
        .map(|(_, card)| category_tone(&card.category, state.light).liquid)
        .collect();
    let all: Vec<Color> = pouring_cards(state, plan).map(|(_, card)| category_tone(&card.category, state.light).liquid).collect();
    brew_tint(&own).or_else(|| brew_tint(&all))
}

pub fn drip_hops(points: &[[f32; 2]]) -> Vec<([f32; 2], [f32; 2])> {
    let third = points.len() / 3;
    let hop = 2.min(points.len().saturating_sub(1));
    (0..3).map(|k| (points[k * third], points[(k * third + hop).min(points.len() - 1)])).collect()
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
        for (k, (start, end)) in drip_hops(points).into_iter().enumerate() {
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
    let dense = plan.vial[1] < 50.0;
    let narrow = plan.vial[0] < 170.0;
    let badge = if dense || narrow { 30.0 } else { 38.0 };
    let centre_y = if narrow && !dense { placed.rect[1] + ((plan.vial[1] - 46.0) * 0.5 + 15.0) * scale } else { placed.rect[1] + placed.rect[3] * 0.5 };
    if placed.mirrored {
        let center = [placed.rect[0] + (9.0 + badge * 0.5) * scale, centre_y];
        return [center[0] - badge * 0.22 * scale, center[1] - badge * 0.36 * scale];
    }
    let center = [placed.rect[0] + placed.rect[2] - (9.0 + badge * 0.5) * scale, centre_y];
    [center[0] + badge * 0.22 * scale, center[1] - badge * 0.36 * scale]
}

pub fn pour_paths(state: &CanvasState, plan: &BrewPlan) -> Vec<(NodeId, Vec<[f32; 2]>)> {
    let ingredients: Vec<&BrewCard> = plan.with_role(BrewRole::Ingredient).filter(|placed| state.card(&placed.id).is_some()).collect();
    let spread = ingredients.len().max(2) as f32 - 1.0;
    let mut columns: Vec<f32> = ingredients.iter().map(|placed| placed.rect[0]).collect();
    columns.sort_by(f32::total_cmp);
    columns.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    let ribbon_top = ribbon_paths(plan).iter().flatten().map(|point| point[1]).fold(f32::MAX, f32::min);
    let lane_order: Vec<NodeId> = {
        let mut lanes: Vec<(f32, f32, NodeId)> = ingredients.iter().map(|placed| (placed.rect[0], -placed.rect[1], placed.id.clone())).collect();
        lanes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        lanes.into_iter().map(|(_, _, id)| id).collect()
    };
    ingredients
        .iter()
        .enumerate()
        .map(|(index, placed)| {
            let from = vial_mouth(placed, plan);
            let points = match plan.lanes {
                Some([shelf_bottom, gap]) => {
                    let lane_rank = lane_order.iter().position(|id| id == &placed.id).unwrap_or(0) as f32;
                    let column = columns.iter().position(|x| (x - placed.rect[0]).abs() < 1.0).unwrap_or(0);
                    let mates = ingredients.iter().filter(|other| (other.rect[0] - placed.rect[0]).abs() < 1.0).count().max(1);
                    let rank = ingredients.iter().filter(|other| (other.rect[0] - placed.rect[0]).abs() < 1.0 && other.rect[1] < placed.rect[1] - 0.5).count();
                    let offset = (mates as f32 - 1.0) * 0.5 - rank as f32;
                    let strand = (gap * 0.8 / mates as f32).min(6.0 * plan.scale);
                    let lane_x = placed.rect[0] + placed.rect[2] + gap * 0.5 + offset * strand;
                    let share = column as f32 / (columns.len().max(2) - 1) as f32;
                    let to = rim_point(plan, (share + offset * 0.035).clamp(0.0, 1.0));
                    let floor = (shelf_bottom + gap).min(ribbon_top - 8.0 * plan.scale).max(shelf_bottom + 2.0 * plan.scale);
                    let step = ((floor - shelf_bottom - 4.0 * plan.scale) / ingredients.len().max(1) as f32).clamp(0.0, 6.0 * plan.scale);
                    let turn = [lane_x, floor - (ingredients.len() as f32 - 1.0 - lane_rank) * step];
                    let clear_x = lane_x.min(spout_point(plan)[0] - 10.0 * plan.scale).max(to[0]);
                    let run = lane_x - clear_x;
                    let elbow = if run > 1.0 { (8.0 * plan.scale).min(run * 0.5).min((turn[1] - from[1]).abs() * 0.3) } else { 0.0 };
                    let bend = (8.0 * plan.scale).min((lane_x - from[0]).abs() * 0.8).min((turn[1] - from[1]).abs() * 0.5);
                    let side = if lane_x >= from[0] { 1.0 } else { -1.0 };
                    let corner = [lane_x - side * bend, from[1]];
                    let mut points: Vec<[f32; 2]> = (0..=4).map(|step| [from[0] + (corner[0] - from[0]) * step as f32 / 4.0, from[1]]).collect();
                    points.extend((1..=6).map(|step| {
                        let t = step as f32 / 6.0;
                        let u = 1.0 - t;
                        [u * u * corner[0] + 2.0 * u * t * lane_x + t * t * lane_x, u * u * from[1] + 2.0 * u * t * from[1] + t * t * (from[1] + bend)]
                    }));
                    let drop_end = turn[1] - elbow;
                    points.extend((1..=12).map(|step| [lane_x, from[1] + bend + (drop_end - from[1] - bend) * step as f32 / 12.0]));
                    let start = if run > 1.0 {
                        points.extend((1..=6).map(|step| {
                            let t = step as f32 / 6.0;
                            let u = 1.0 - t;
                            [u * u * lane_x + 2.0 * u * t * lane_x + t * t * (lane_x - elbow), u * u * drop_end + 2.0 * u * t * turn[1] + t * t * turn[1]]
                        }));
                        points.extend((1..=6).map(|step| [lane_x - elbow + (clear_x - lane_x + elbow) * step as f32 / 6.0, turn[1]]));
                        [clear_x, turn[1]]
                    } else {
                        turn
                    };
                    points.extend((1..=16).map(|step| {
                        let t = step as f32 / 16.0;
                        let c1 = [to[0] + (start[0] - to[0]) * 0.35, start[1]];
                        let c2 = [to[0], start[1] + (to[1] - start[1]) * 0.4];
                        let u = 1.0 - t;
                        [
                            u * u * u * start[0] + 3.0 * u * u * t * c1[0] + 3.0 * u * t * t * c2[0] + t * t * t * to[0],
                            u * u * u * start[1] + 3.0 * u * u * t * c1[1] + 3.0 * u * t * t * c2[1] + t * t * t * to[1],
                        ]
                    }));
                    points
                }
                None => {
                    let (to, lift) = landing(plan, placed, index, spread);
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
    if style.hexed {
        let cut = &points[..(points.len() * 46 / 100).max(2).min(points.len())];
        let end = cut[cut.len() - 1];
        let shape = crate::hearth::dashed_polyline(shape, cut, 7.0, 5.0, 2.4, ink, None);
        let shape = shape.line([end[0] - 5.0, end[1] - 5.0], [end[0] + 5.0, end[1] + 5.0], 2.2, rune::LineCap::Round, ink);
        return shape.line([end[0] - 5.0, end[1] + 5.0], [end[0] + 5.0, end[1] - 5.0], 2.2, rune::LineCap::Round, ink);
    }
    if style.untaken || style.blocked {
        return crate::hearth::dashed_polyline(shape, points, 6.5, 5.2, 2.0, ink, None);
    }
    let core = if style.active { 2.8 } else { 2.2 };
    let shape = shape.stroke_path(
        &path,
        core + if style.active { 4.0 } else { 2.6 },
        rune::LineCap::Round,
        with_alpha(style.glow, if style.active { 0.34 } else { 0.2 }),
    );
    let shape = shape.stroke_path(&path, core, rune::LineCap::Round, ink);
    let shape = shape.stroke_path(&path, 0.8, rune::LineCap::Round, with_alpha(p.broth_spark, if style.active { 0.85 } else { 0.45 }));
    let rim = points[points.len() - 1];
    crate::hearth::fill_ellipse(shape, rim[0], rim[1], 4.2 * core * 0.5, 1.6 * core * 0.5, with_alpha(style.glow, 0.7))
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
        let (to, lift) = landing(plan, placed, index, spread);
        let style = stream_for(state, card, mood, p, lift);
        shape = match (plan.lanes, paths.iter().find(|(id, _)| id == &placed.id)) {
            (Some(_), Some((_, points))) => lane_stream(shape, points, style, p),
            _ => pouring_stream_shape(shape, vial_mouth(placed, plan), to, style),
        };
    }
    for placed in &bottles {
        let Some(card) = state.card(&placed.id) else { continue };
        let taken = matches!(card.status, ExecutionStatus::Succeeded | ExecutionStatus::Running);
        let untaken = state.has_run() && !taken;
        let ribbon_mood = if multi && taken { CauldronMood::Brewed } else { mood };
        shape = distillation_ribbon_shape(
            shape,
            &ribbon_route(plan, bottle_neck(placed, multi), Some(&placed.id)),
            ribbon_mood,
            state.light,
            taken,
            untaken,
            if multi { bottle_tint(state, plan, &placed.id) } else { potion_tint(state, plan) },
        );
    }
    if let Some(ghost) = plan.ghost_bottle {
        shape = distillation_ribbon_shape(shape, &ribbon_route(plan, ghost_neck(ghost), None), CauldronMood::Empty, state.light, false, false, None);
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
            let ms = state.brew_total_ms.unwrap_or_else(|| state.nodes.iter().filter_map(|card| card.duration_ms).sum());
            let time = if ms == 0 { "<1ms".to_owned() } else { format!("{ms}ms") };
            let bottles: Vec<&BrewCard> = plan.with_role(BrewRole::Bottle).collect();
            let filled = bottles.iter().filter(|placed| state.card(&placed.id).is_some_and(|card| card.status == ExecutionStatus::Succeeded)).count();
            match (bottles.len(), filled) {
                (_, 0) => format!("Brewed · {time} · bottle empty"),
                (0 | 1, _) => format!("Brewed · {time}"),
                (_, 1) => format!("Brewed · {time} · 1 bottle filled"),
                (_, count) => format!("Brewed · {time} · {count} bottles filled"),
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
    let shrink = plan.scale.min(1.0);
    row().w(width).h(28.0).justify_center().absolute(cx - width * 0.5, plan.plaque_top - 14.0 * (1.0 - shrink)).scale(shrink).child(
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

fn shelf_plank_shape(size: [f32; 2], ledges: &[f32], s: f32, p: &HearthPalette) -> Shape {
    let mut shape = Shape::new();
    let thick = 9.0 * s;
    for &ledge in ledges {
        let y = ledge + 2.0 * s;
        let bracket = |shape: Shape, x: f32| {
            let arm = Path::new().move_to([x, y + thick]).line_to([x, y + thick + 12.0 * s]).line_to([x + 12.0 * s, y + thick]);
            shape.stroke_path(&arm, 2.6 * s, LineCap::Round, p.iron_rim)
        };
        shape = bracket(shape, size[0] * 0.14);
        shape = bracket(shape, size[0] * 0.86 - 12.0 * s);
        let board =
            Path::new().move_to([2.0 * s, y]).line_to([size[0] - 2.0 * s, y]).line_to([size[0] - 2.0 * s, y + thick]).line_to([2.0 * s, y + thick]).close();
        shape = shape.fill_path(&board, p.wood);
        shape = shape.line([3.0 * s, y + 1.2 * s], [size[0] - 3.0 * s, y + 1.2 * s], 1.6 * s, LineCap::Round, p.wood_hi);
        shape = shape.line([3.0 * s, y + thick], [size[0] - 3.0 * s, y + thick], 1.2 * s, LineCap::Round, with_alpha(p.iron_rim, 0.55));
        for grain in [0.22_f32, 0.47, 0.71] {
            let x = size[0] * grain;
            shape = shape.line([x, y + thick * 0.45], [x + 22.0 * s, y + thick * 0.55], 0.9 * s, LineCap::Round, with_alpha(p.iron_rim, 0.28));
        }
    }
    shape
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
        .child(canvas(tap_shape([pot[2], pot[3]], &p)).key("pot-tap").size(pot[2], pot[3]).absolute(0.0, 0.0))
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
                .fill(if inside { with_alpha(p.gold_hi, 0.16) } else { with_alpha(p.card, 0.35) })
                .child(
                    canvas(shelf_plank_shape([zone[2] - 28.0, zone[3] - 14.0], &[(zone[3] - 14.0) * 0.62], 1.0, &p))
                        .key("aside-tray-plank")
                        .size(zone[2] - 28.0, zone[3] - 14.0)
                        .absolute(0.0, 0.0)
                        .opacity(1.0),
                ),
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
        let s = plan.scale.max(0.4);
        let pad = 14.0 * s;
        let left = aside_rects.iter().map(|r| r[0]).fold(first[0], f32::min) - pad;
        let top = aside_rects.iter().map(|r| r[1]).fold(first[1], f32::min);
        let right = aside_rects.iter().map(|r| r[0] + r[2]).fold(first[0] + first[2], f32::max) + pad;
        let bottom = aside_rects.iter().map(|r| r[1] + r[3]).fold(first[1] + first[3], f32::max) + 26.0 * s;
        let mut ledges: Vec<f32> = aside_rects.iter().map(|r| r[1] + r[3] - top).collect();
        ledges.sort_by(f32::total_cmp);
        ledges.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        board = board.child(el().key("aside-shelf").size(right - left, bottom - top).absolute(left, top).child(
            canvas(shelf_plank_shape([right - left, bottom - top], &ledges, s, &p)).key("aside-plank").size(right - left, bottom - top).absolute(0.0, 0.0),
        ));
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
        let vial = ingredient_vial_card(state, card, step, origin, plan.vial, placed.role == BrewRole::Aside, placed.mirrored, &p, lens, toss, plan.clone());
        board = board.child(vial.scale(plan.scale));
    }
    for placed in &bottles {
        let Some(card) = state.card(&placed.id) else { continue };
        let step = if has_run {
            card.step_order
        } else if multi {
            None
        } else {
            placed.recipe_step
        };
        let natural = if multi { MULTI_BOTTLE_SIZE } else { SINGLE_BOTTLE_SIZE };
        let origin = card_frame(placed.rect, natural, plan.scale, [0.0, 0.0]);
        let view = if multi {
            branch_bottle_view(state, card, step, [0.0, 0.0, natural[0]], bottle_tint(state, &plan, &placed.id), has_run, plan.vial[1] < 50.0, &p, lens)
        } else {
            potion_stand_view(state, card, step, [0.0, 0.0, natural[0]], potion_tint(state, &plan), mood, plan.vial[1] < 50.0, &p, lens)
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
    board
}
