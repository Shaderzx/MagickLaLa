use std::collections::{HashSet, VecDeque};

use magicklala_graph::NodeId;

use crate::{CanvasState, NodeCardModel, WireModel};

pub const VIAL_SIZE: [f32; 2] = [196.0, 58.0];
pub const POT_SIZE: [f32; 2] = [300.0, 290.0];
pub const SINGLE_BOTTLE_SIZE: [f32; 2] = [148.0, 196.0];
pub const MULTI_BOTTLE_SIZE: [f32; 2] = [150.0, 148.0];
const ROW_GAP: f32 = 22.0;
const COL_GAP: f32 = 28.0;
const SHELF_TO_POT: f32 = 52.0;
const POT_TO_BOTTLES: f32 = 60.0;
const PLAQUE_ROOM: f32 = 46.0;
const EMPTY_POT_ROOM: f32 = 132.0;
const BOTTLE_GAP: f32 = 30.0;
const ASIDE_GAP: f32 = 40.0;
const TOP_ROOM: f32 = 64.0;
const BOTTOM_ROOM: f32 = 52.0;
const SIDE_ROOM: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrewRole {
    Ingredient,
    Aside,
    Bottle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrewCard {
    pub id: NodeId,
    pub role: BrewRole,
    pub recipe_step: Option<usize>,
    pub feeds_only: Option<NodeId>,
    pub mirrored: bool,
    pub rect: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrewPlan {
    pub scale: f32,
    pub pot: [f32; 4],
    pub plaque_top: f32,
    pub ghost_bottle: Option<[f32; 4]>,
    pub aside_zone: [f32; 4],
    pub vial: [f32; 2],
    pub lanes: Option<[f32; 2]>,
    pub flank: bool,
    pub cards: Vec<BrewCard>,
}

impl BrewPlan {
    pub fn card(&self, id: &NodeId) -> Option<&BrewCard> {
        self.cards.iter().find(|card| &card.id == id)
    }

    pub fn with_role(&self, role: BrewRole) -> impl Iterator<Item = &BrewCard> {
        self.cards.iter().filter(move |card| card.role == role)
    }

    pub fn pot_center(&self) -> [f32; 2] {
        [self.pot[0] + self.pot[2] * 0.5, self.pot[1] + self.pot[3] * 0.5]
    }

    pub fn pour_radius(&self) -> f32 {
        self.pot[2].max(self.pot[3]) * 0.72
    }

    pub fn is_in_aside_zone(&self, point: [f32; 2]) -> bool {
        let zone = self.aside_zone;
        point[0] >= zone[0] && point[0] <= zone[0] + zone[2] && point[1] >= zone[1] && point[1] <= zone[1] + zone[3] && !self.is_over_pot(point)
    }

    pub fn is_over_pot(&self, point: [f32; 2]) -> bool {
        let center = self.pot_center();
        (point[0] - center[0]).hypot(point[1] - center[1]) <= self.pour_radius()
    }
}

pub fn is_bottle(card: &NodeCardModel) -> bool {
    card.category.eq_ignore_ascii_case("io")
}

fn upstream_of(state: &CanvasState, targets: &[NodeId]) -> HashSet<NodeId> {
    let mut seen: HashSet<NodeId> = targets.iter().cloned().collect();
    let mut queue: VecDeque<NodeId> = targets.iter().cloned().collect();
    while let Some(node) = queue.pop_front() {
        for wire in state.wires.iter().filter(|wire| wire.to_node == node) {
            if seen.insert(wire.from_node.clone()) {
                queue.push_back(wire.from_node.clone());
            }
        }
    }
    seen
}

pub fn recipe_order(state: &CanvasState) -> Vec<NodeId> {
    let mut waiting: Vec<usize> =
        state.nodes.iter().map(|card| state.wires.iter().filter(|wire| wire.to_node == card.id && wire.from_node != card.id).count()).collect();
    let mut placed = vec![false; state.nodes.len()];
    let mut order: Vec<NodeId> = Vec::with_capacity(state.nodes.len());
    loop {
        let ready = state
            .nodes
            .iter()
            .enumerate()
            .filter(|(index, _)| !placed[*index] && waiting[*index] == 0)
            .min_by_key(|(index, card)| (!card.category.eq_ignore_ascii_case("events"), *index))
            .map(|(index, _)| index);
        let next = match ready {
            Some(index) => index,
            None => match placed.iter().position(|done| !done) {
                Some(index) => index,
                None => break,
            },
        };
        placed[next] = true;
        let id = state.nodes[next].id.clone();
        for wire in state.wires.iter().filter(|wire| wire.from_node == id) {
            if let Some(target) = state.nodes.iter().position(|card| card.id == wire.to_node) {
                waiting[target] = waiting[target].saturating_sub(1);
            }
        }
        order.push(id);
    }
    order
}

fn trigger_reach(state: &CanvasState) -> Vec<NodeId> {
    let triggers: Vec<&NodeCardModel> = state.nodes.iter().filter(|card| !state.is_shelved(&card.id) && card.category.eq_ignore_ascii_case("events")).collect();
    let pouring: Vec<NodeId> =
        triggers.iter().filter(|card| state.wires.iter().any(|wire| wire.from_node == card.id && wire.ty.is_flow())).map(|card| card.id.clone()).collect();
    let lone = |card: &&&NodeCardModel| !state.wires.iter().any(|wire| wire.from_node == card.id || wire.to_node == card.id);
    let mut ran: Vec<NodeId> = if pouring.is_empty() { triggers.iter().filter(lone).map(|card| card.id.clone()).collect() } else { pouring };
    let mut index = 0;
    while index < ran.len() {
        let node = ran[index].clone();
        for wire in state.wires.iter().filter(|wire| wire.from_node == node && wire.ty.is_flow()) {
            if !ran.contains(&wire.to_node) {
                ran.push(wire.to_node.clone());
            }
        }
        index += 1;
    }
    ran
}

pub fn potion_ends(state: &CanvasState) -> Vec<NodeId> {
    let reach = trigger_reach(state);
    let fed = |card: &NodeCardModel| reach.contains(&card.id);
    let outputs: Vec<NodeId> = state
        .nodes
        .iter()
        .filter(|card| !state.is_shelved(&card.id) && is_bottle(card) && fed(card) && !state.wires.iter().any(|wire| wire.from_node == card.id))
        .map(|card| card.id.clone())
        .collect();
    if !outputs.is_empty() {
        return outputs;
    }
    state
        .nodes
        .iter()
        .filter(|card| !state.is_shelved(&card.id))
        .filter(|card| state.wires.iter().any(|wire| wire.to_node == card.id && wire.ty.is_flow()))
        .filter(|card| !state.wires.iter().any(|wire| wire.from_node == card.id))
        .filter(|card| reach.contains(&card.id))
        .map(|card| card.id.clone())
        .collect()
}

pub fn brewing_nodes(state: &CanvasState) -> HashSet<NodeId> {
    let bottles = potion_ends(state);
    let open = |id: &NodeId| !state.is_shelved(id);
    if bottles.is_empty() && state.wires.is_empty() {
        return trigger_reach(state).into_iter().collect();
    }
    if bottles.is_empty() {
        let reach = trigger_reach(state);
        return state
            .nodes
            .iter()
            .filter(|card| state.wires.iter().any(|wire| wire.from_node == card.id || wire.to_node == card.id))
            .filter(|card| !is_bottle(card) || reach.contains(&card.id))
            .map(|card| card.id.clone())
            .filter(open)
            .collect();
    }
    let mut ran = trigger_reach(state);
    ran.extend(bottles);
    upstream_of(state, &ran).into_iter().filter(open).collect()
}

pub fn recipe_nodes(state: &CanvasState) -> HashSet<NodeId> {
    let mut recipe = brewing_nodes(state);
    recipe.extend(potion_ends(state));
    recipe
}

fn feeds_only_one_bottle(state: &CanvasState, id: &NodeId, bottle_count: usize) -> Option<NodeId> {
    if bottle_count < 2 {
        return None;
    }
    let outgoing: Vec<&WireModel> = state.wires.iter().filter(|wire| &wire.from_node == id).collect();
    let first = outgoing.first()?.to_node.clone();
    let one_target = outgoing.iter().all(|wire| wire.to_node == first);
    let target_is_bottle = state.card(&first).is_some_and(is_bottle);
    (one_target && target_is_bottle && outgoing.iter().all(|wire| !wire.ty.is_flow())).then_some(first)
}

fn shelf_grid(count: usize, rows_fit: usize) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    let cols = count.div_ceil(rows_fit.clamp(3, 6));
    (cols, count.div_ceil(cols))
}

fn stack_height(count: usize, item: f32, gap: f32) -> f32 {
    if count == 0 { 0.0 } else { count as f32 * item + (count - 1) as f32 * gap }
}

#[derive(Debug, Clone, Copy)]
struct Metrics {
    vial: [f32; 2],
    pot: [f32; 2],
    row_gap: f32,
    col_gap: f32,
    shelf_to_pot: f32,
    pot_to_bottles: f32,
    bottle_gap: f32,
}

const ROOMY: Metrics = Metrics {
    vial: VIAL_SIZE,
    pot: POT_SIZE,
    row_gap: ROW_GAP,
    col_gap: COL_GAP,
    shelf_to_pot: SHELF_TO_POT,
    pot_to_bottles: POT_TO_BOTTLES,
    bottle_gap: BOTTLE_GAP,
};
const CROWDED: Metrics =
    Metrics { vial: [160.0, 58.0], pot: [250.0, 242.0], row_gap: 12.0, col_gap: 14.0, shelf_to_pot: 40.0, pot_to_bottles: 44.0, bottle_gap: 10.0 };
const TIGHT: Metrics = Metrics { vial: [160.0, 42.0], ..CROWDED };
const READABLE: f32 = 0.92;
const BOTTLE_STAIR: f32 = 30.0;
const LEGIBLE: f32 = 0.8;
const SHELVED: f32 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    CrowdedRing,
    CrowdedFlank,
    TightFlank,
    Stacked,
    TightRing,
    TightStacked,
}

impl Arrangement {
    fn build(self, state: &CanvasState, size: (f32, f32)) -> (f32, BrewPlan) {
        let small_pot = [204.0, 198.0];
        match self {
            Arrangement::CrowdedRing => plan_with(state, size, CROWDED),
            Arrangement::CrowdedFlank => plan_flanked(state, size, CROWDED),
            Arrangement::TightFlank => plan_flanked(state, size, TIGHT),
            Arrangement::Stacked => plan_stacked(state, size, Metrics { pot: small_pot, col_gap: 24.0, ..CROWDED }),
            Arrangement::TightRing => plan_with(state, size, TIGHT),
            Arrangement::TightStacked => plan_stacked(state, size, Metrics { pot: small_pot, ..TIGHT }),
        }
    }
}

const FALLBACKS: [(Arrangement, f32); 8] = [
    (Arrangement::CrowdedRing, READABLE),
    (Arrangement::CrowdedFlank, LEGIBLE),
    (Arrangement::TightFlank, LEGIBLE),
    (Arrangement::Stacked, READABLE),
    (Arrangement::TightRing, READABLE),
    (Arrangement::Stacked, LEGIBLE),
    (Arrangement::TightRing, LEGIBLE),
    (Arrangement::TightStacked, LEGIBLE),
];

pub fn brew_plan(state: &CanvasState, size: (f32, f32)) -> BrewPlan {
    let roomy = plan_with(state, size, ROOMY);
    if roomy.0.min(1.0) >= READABLE {
        return roomy.1;
    }
    let mut built: Vec<(Arrangement, f32, BrewPlan)> = Vec::new();
    for (arrangement, floor) in FALLBACKS {
        let fit = match built.iter().find(|(made, _, _)| *made == arrangement) {
            Some((_, fit, _)) => *fit,
            None => {
                let (fit, plan) = arrangement.build(state, size);
                built.push((arrangement, fit, plan));
                fit
            }
        };
        if fit >= floor {
            return built.iter().find(|(made, _, _)| *made == arrangement).map(|(_, _, plan)| plan.clone()).unwrap_or_else(|| roomy.1.clone());
        }
    }
    let best_fit = built.iter().map(|(_, fit, _)| *fit).fold(0.0, f32::max);
    if let Some((_, fit, plan)) = built.iter().find(|(made, _, _)| *made == Arrangement::TightFlank)
        && *fit >= SHELVED
        && *fit >= best_fit * 0.9
    {
        return plan.clone();
    }
    built
        .into_iter()
        .fold(None, |best: Option<(f32, BrewPlan)>, (_, fit, plan)| match best {
            Some((top, _)) if top >= fit => best,
            _ => Some((fit, plan)),
        })
        .map_or_else(|| plan_with(state, size, TIGHT).1, |(_, plan)| plan)
}
fn plan_flanked(state: &CanvasState, size: (f32, f32), m: Metrics) -> (f32, BrewPlan) {
    let order = recipe_order(state);
    let brewing = brewing_nodes(state);
    let ends = potion_ends(state);
    let bottles: Vec<NodeId> = order.iter().filter(|id| ends.contains(id)).cloned().collect();
    let ingredients: Vec<NodeId> = order.iter().filter(|id| brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();
    let aside: Vec<NodeId> = order.iter().filter(|id| !brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();
    let usable_w = (size.0 - 2.0 * SIDE_ROOM).max(120.0);
    let usable_h = (size.1 - TOP_ROOM - BOTTOM_ROOM).max(120.0);
    let left_count = ingredients.len().div_ceil(2);
    let side_h = stack_height(left_count, m.vial[1], m.row_gap);
    let bottle_size = if bottles.len() > 1 { MULTI_BOTTLE_SIZE } else { SINGLE_BOTTLE_SIZE };
    let bottle_count = bottles.len().max(1);
    let bottles_w = bottle_count as f32 * bottle_size[0] + (bottle_count - 1) as f32 * m.col_gap;
    let rim_drop = m.pot[1] * 0.44;
    let tap_drop = m.pot[1] * 0.56 + 30.0;
    let neck_drop = if bottles.len() > 1 { 24.0 } else { 29.0 };
    let pot_x = m.vial[0] + m.shelf_to_pot;
    let pot_y = (side_h - 20.0 - rim_drop).max(0.0);
    let side_top = pot_y + rim_drop + 20.0 - side_h;
    let shelf_x = pot_x + m.pot[0] + m.shelf_to_pot;
    let bottles_x = shelf_x + ((m.vial[0] - bottles_w) * 0.5).max(0.0);
    let bottles_top = (side_top + side_h + 16.0).max(pot_y + tap_drop - neck_drop);
    let block_w = (shelf_x + m.vial[0]).max(bottles_x + bottles_w);
    let block_h = (pot_y + m.pot[1] + PLAQUE_ROOM).max(bottles_top + bottle_size[1]).max(side_top + side_h);
    let aside_per_row = (((block_w + m.col_gap) / (m.vial[0] + m.col_gap)).floor() as usize).max(1);
    let aside_rows = aside.len().div_ceil(aside_per_row);
    let aside_h = if aside.is_empty() { 0.0 } else { ASIDE_GAP + stack_height(aside_rows, m.vial[1], m.row_gap) };
    let design_h = block_h + aside_h;
    let fit = (usable_w / block_w).min(usable_h / design_h).clamp(0.05, 1.25);
    let scale = fit * state.brew_camera.zoom;
    let anchor = [size.0 * 0.5 + state.brew_camera.pan_x, TOP_ROOM + usable_h * 0.5 + state.brew_camera.pan_y];
    let to_screen = |x: f32, y: f32, w: f32, h: f32| -> [f32; 4] {
        [anchor[0] + (x - block_w * 0.5) * scale, anchor[1] + (y - design_h * 0.5) * scale, w * scale, h * scale]
    };
    let mut cards = Vec::with_capacity(state.nodes.len());
    for (index, id) in ingredients.iter().enumerate() {
        let mirrored = index >= left_count;
        let row = if mirrored { index - left_count } else { index };
        let x = if mirrored { shelf_x } else { 0.0 };
        let y = side_top + row as f32 * (m.vial[1] + m.row_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Ingredient,
            recipe_step: Some(index + 1),
            feeds_only: feeds_only_one_bottle(state, id, bottles.len()),
            mirrored,
            rect: to_screen(x, y, m.vial[0], m.vial[1]),
        });
    }
    for (index, id) in bottles.iter().enumerate() {
        let x = bottles_x + index as f32 * (bottle_size[0] + m.col_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Bottle,
            recipe_step: Some(ingredients.len() + index + 1),
            feeds_only: None,
            mirrored: false,
            rect: to_screen(x, bottles_top, bottle_size[0], bottle_size[1]),
        });
    }
    let aside_top = block_h + ASIDE_GAP;
    for (index, id) in aside.iter().enumerate() {
        let x = (index % aside_per_row) as f32 * (m.vial[0] + m.col_gap);
        let y = aside_top + (index / aside_per_row) as f32 * (m.vial[1] + m.row_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Aside,
            recipe_step: None,
            feeds_only: None,
            mirrored: false,
            rect: to_screen(x, y, m.vial[0], m.vial[1]),
        });
    }
    let pot = to_screen(pot_x, pot_y, m.pot[0], m.pot[1]);
    let ghost_bottle = bottles.is_empty().then(|| to_screen(bottles_x, bottles_top, SINGLE_BOTTLE_SIZE[0], SINGLE_BOTTLE_SIZE[1]));
    let plaque_top = pot[1] + pot[3] + 8.0 * scale;
    let block_bottom = to_screen(0.0, block_h, 0.0, 0.0)[1] + 12.0 * scale;
    let zone_top = if aside.is_empty() {
        block_bottom.max(plaque_top + 40.0 * scale)
    } else {
        (to_screen(0.0, aside_top, 0.0, 0.0)[1] - ASIDE_GAP * 0.5 * scale).max(plaque_top + 40.0 * scale)
    };
    let zone_top = zone_top.min(size.1 - 84.0).max(0.0);
    let aside_zone = [0.0, zone_top, size.0, (size.1 - zone_top).max(84.0)];
    (fit, BrewPlan { scale, pot, plaque_top, ghost_bottle, aside_zone, vial: m.vial, lanes: None, flank: true, cards })
}

fn plan_stacked(state: &CanvasState, size: (f32, f32), m: Metrics) -> (f32, BrewPlan) {
    let order = recipe_order(state);
    let brewing = brewing_nodes(state);
    let ends = potion_ends(state);
    let bottles: Vec<NodeId> = order.iter().filter(|id| ends.contains(id)).cloned().collect();
    let ingredients: Vec<NodeId> = order.iter().filter(|id| brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();
    let aside: Vec<NodeId> = order.iter().filter(|id| !brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();
    let usable_w = (size.0 - 2.0 * SIDE_ROOM).max(120.0);
    let usable_h = (size.1 - TOP_ROOM - BOTTOM_ROOM).max(120.0);
    let readable_row = (((usable_w / READABLE + m.col_gap) / (m.vial[0] + m.col_gap)).floor() as usize).max(1);
    let bottle_size = if bottles.len() > 1 { MULTI_BOTTLE_SIZE } else { SINGLE_BOTTLE_SIZE };
    let pot_block_h = m.pot[1] + PLAQUE_ROOM;
    let lane_band = 14.0 + 6.0 * ingredients.len().min(4) as f32;
    let hop_room = if bottles.len() >= 4 { 18.0 } else { 14.0 * bottles.len().saturating_sub(1) as f32 };
    let shelf_gap = if ingredients.is_empty() { 0.0 } else { 22.0 + lane_band + hop_room };
    let bottle_count = bottles.len().max(1);
    let shelf_for = |per_row: usize| {
        let cols = ingredients.len().min(per_row);
        let w = if cols == 0 { 0.0 } else { cols as f32 * m.vial[0] + (cols - 1) as f32 * m.col_gap };
        (per_row, w, stack_height(ingredients.len().div_ceil(per_row), m.vial[1], m.row_gap))
    };
    let shape_for = |(per_row, shelf_w, shelf_h): (usize, f32, f32), rows: usize| {
        let cols = bottle_count.div_ceil(rows);
        let rows = bottle_count.div_ceil(cols);
        let w = cols as f32 * bottle_size[0] + (cols - 1) as f32 * m.col_gap;
        let h = stack_height(rows, bottle_size[1], m.bottle_gap) + (cols - 1) as f32 * BOTTLE_STAIR;
        let lower_w = m.pot[0] + m.pot_to_bottles + w;
        let lower_h = pot_block_h.max(h);
        let fit = (usable_w / shelf_w.max(lower_w)).min(usable_h / (shelf_h + shelf_gap + lower_h));
        (fit, per_row, shelf_w, shelf_h, rows, h, lower_w, lower_h)
    };
    let widest = if bottles.len() >= 4 { ingredients.len().max(readable_row) } else { readable_row };
    let (_, per_row, shelf_w, shelf_h, bottle_rows, bottles_h, lower_w, lower_h) = (readable_row..=widest)
        .flat_map(|per_row| (1..=bottle_count).map(move |rows| (per_row, rows)))
        .map(|(per_row, rows)| shape_for(shelf_for(per_row), rows))
        .fold(shape_for(shelf_for(readable_row), 1), |best, next| if next.0 > best.0 + 0.001 { next } else { best });
    let block_w = shelf_w.max(lower_w);
    let block_h = shelf_h + shelf_gap + lower_h;
    let aside_per_row = (((block_w + m.col_gap) / (m.vial[0] + m.col_gap)).floor() as usize).max(1);
    let aside_rows = aside.len().div_ceil(aside_per_row);
    let aside_h = if aside.is_empty() { 0.0 } else { ASIDE_GAP + stack_height(aside_rows, m.vial[1], m.row_gap) };
    let design_h = block_h + aside_h;
    let fit = (usable_w / block_w).min(usable_h / design_h).clamp(0.05, 1.25);
    let scale = fit * state.brew_camera.zoom;
    let anchor = [size.0 * 0.5 + state.brew_camera.pan_x, TOP_ROOM + usable_h * 0.5 + state.brew_camera.pan_y];
    let to_screen = |x: f32, y: f32, w: f32, h: f32| -> [f32; 4] {
        [anchor[0] + (x - block_w * 0.5) * scale, anchor[1] + (y - design_h * 0.5) * scale, w * scale, h * scale]
    };
    let mut cards = Vec::with_capacity(state.nodes.len());
    let shelf_left = (block_w - shelf_w) * 0.5;
    for (index, id) in ingredients.iter().enumerate() {
        let x = shelf_left + (index % per_row) as f32 * (m.vial[0] + m.col_gap);
        let y = (index / per_row) as f32 * (m.vial[1] + m.row_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Ingredient,
            recipe_step: Some(index + 1),
            feeds_only: feeds_only_one_bottle(state, id, bottles.len()),
            mirrored: false,
            rect: to_screen(x, y, m.vial[0], m.vial[1]),
        });
    }
    let lower_top = shelf_h + shelf_gap;
    let pot_x = ((block_w - m.pot[0]) * 0.5).min(block_w - lower_w).max(0.0);
    let bottles_x = pot_x + m.pot[0] + m.pot_to_bottles;
    let bottles_top = lower_top + (lower_h - bottles_h) * 0.5;
    let bottle_cols = bottles.len().div_ceil(bottle_rows.max(1)).max(1);
    for (index, id) in bottles.iter().enumerate() {
        let col = index / bottle_rows;
        let x = bottles_x + col as f32 * (bottle_size[0] + m.col_gap);
        let y = bottles_top + (bottle_cols - 1 - col) as f32 * BOTTLE_STAIR + (index % bottle_rows) as f32 * (bottle_size[1] + m.bottle_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Bottle,
            recipe_step: Some(ingredients.len() + index + 1),
            feeds_only: None,
            mirrored: false,
            rect: to_screen(x, y, bottle_size[0], bottle_size[1]),
        });
    }
    let aside_top = block_h + ASIDE_GAP;
    for (index, id) in aside.iter().enumerate() {
        let x = (index % aside_per_row) as f32 * (m.vial[0] + m.col_gap);
        let y = aside_top + (index / aside_per_row) as f32 * (m.vial[1] + m.row_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Aside,
            recipe_step: None,
            feeds_only: None,
            mirrored: false,
            rect: to_screen(x, y, m.vial[0], m.vial[1]),
        });
    }
    let pot = to_screen(pot_x, lower_top + (lower_h - pot_block_h) * 0.5, m.pot[0], m.pot[1]);
    let ghost_bottle =
        bottles.is_empty().then(|| to_screen(bottles_x, lower_top + (lower_h - SINGLE_BOTTLE_SIZE[1]) * 0.5, SINGLE_BOTTLE_SIZE[0], SINGLE_BOTTLE_SIZE[1]));
    let plaque_top = pot[1] + pot[3] + 8.0 * scale;
    let block_bottom = to_screen(0.0, block_h, 0.0, 0.0)[1] + 12.0 * scale;
    let zone_top = if aside.is_empty() {
        block_bottom.max(plaque_top + 40.0 * scale)
    } else {
        (to_screen(0.0, aside_top, 0.0, 0.0)[1] - ASIDE_GAP * 0.5 * scale).max(plaque_top + 40.0 * scale)
    };
    let zone_top = zone_top.min(size.1 - 84.0).max(0.0);
    let aside_zone = [0.0, zone_top, size.0, (size.1 - zone_top).max(84.0)];
    let shelf_bottom = to_screen(0.0, shelf_h, 0.0, 0.0)[1];
    (fit, BrewPlan { scale, pot, plaque_top, ghost_bottle, aside_zone, vial: m.vial, lanes: Some([shelf_bottom, lane_band * scale]), flank: false, cards })
}

fn plan_with(state: &CanvasState, size: (f32, f32), m: Metrics) -> (f32, BrewPlan) {
    let order = recipe_order(state);
    let brewing = brewing_nodes(state);
    let ends = potion_ends(state);
    let bottles: Vec<NodeId> = order.iter().filter(|id| ends.contains(id)).cloned().collect();
    let ingredients: Vec<NodeId> = order.iter().filter(|id| brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();
    let aside: Vec<NodeId> = order.iter().filter(|id| !brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();

    let usable_h = (size.1 - TOP_ROOM - BOTTOM_ROOM).max(120.0);
    let rows_fit = ((usable_h / READABLE + m.row_gap) / (m.vial[1] + m.row_gap)).floor() as usize;
    let (cols, rows) = shelf_grid(ingredients.len(), rows_fit);
    let shelf_w = if cols == 0 { 0.0 } else { cols as f32 * m.vial[0] + (cols - 1) as f32 * m.col_gap };
    let shelf_h = stack_height(rows, m.vial[1], m.row_gap);
    let bottle_size = if bottles.len() > 1 { MULTI_BOTTLE_SIZE } else { SINGLE_BOTTLE_SIZE };
    let bottle_rows_fit = (((usable_h / READABLE + m.bottle_gap) / (bottle_size[1] + m.bottle_gap)).floor() as usize).max(1);
    let bottle_cols = bottles.len().max(1).div_ceil(bottle_rows_fit);
    let bottle_rows = bottles.len().max(1).div_ceil(bottle_cols);
    let bottles_h = stack_height(bottle_rows, bottle_size[1], m.bottle_gap);
    let headroom = if state.nodes.is_empty() { (m.pot[0].max(m.pot[1]) * 0.72 - m.pot[1] * 0.5 + 12.0).max(0.0) } else { 0.0 };
    let pot_block_h = headroom + m.pot[1] + if state.nodes.is_empty() { EMPTY_POT_ROOM } else { PLAQUE_ROOM };
    let block_h = shelf_h.max(pot_block_h).max(bottles_h);
    let pot_x = if cols == 0 { 0.0 } else { shelf_w + m.shelf_to_pot };
    let bottles_x = pot_x + m.pot[0] + m.pot_to_bottles;
    let block_w = bottles_x + bottle_cols as f32 * bottle_size[0] + (bottle_cols - 1) as f32 * m.col_gap;
    let aside_per_row = ((block_w + m.col_gap) / (m.vial[0] + m.col_gap)).floor().max(1.0) as usize;
    let aside_rows = aside.len().div_ceil(aside_per_row);
    let aside_h = if aside.is_empty() { 0.0 } else { ASIDE_GAP + stack_height(aside_rows, m.vial[1], m.row_gap) };
    let design_w = block_w;
    let design_h = block_h + aside_h;

    let fit = ((size.0 - 2.0 * SIDE_ROOM) / design_w).min(usable_h / design_h).clamp(0.05, 1.25);
    let scale = fit * state.brew_camera.zoom;
    let readable_fit = fit;
    let anchor = [size.0 * 0.5 + state.brew_camera.pan_x, TOP_ROOM + usable_h * 0.5 + state.brew_camera.pan_y];
    let to_screen = |x: f32, y: f32, w: f32, h: f32| -> [f32; 4] {
        [anchor[0] + (x - design_w * 0.5) * scale, anchor[1] + (y - design_h * 0.5) * scale, w * scale, h * scale]
    };

    let mut cards = Vec::with_capacity(state.nodes.len());
    let shelf_top = (block_h - shelf_h) * 0.5;
    for (index, id) in ingredients.iter().enumerate() {
        let x = (index / rows.max(1)) as f32 * (m.vial[0] + m.col_gap);
        let y = shelf_top + (index % rows.max(1)) as f32 * (m.vial[1] + m.row_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Ingredient,
            recipe_step: Some(index + 1),
            feeds_only: feeds_only_one_bottle(state, id, bottles.len()),
            mirrored: false,
            rect: to_screen(x, y, m.vial[0], m.vial[1]),
        });
    }
    let bottles_top = (block_h - bottles_h) * 0.5;
    for (index, id) in bottles.iter().enumerate() {
        let y = bottles_top + (index % bottle_rows) as f32 * (bottle_size[1] + m.bottle_gap);
        let x = bottles_x + (index / bottle_rows) as f32 * (bottle_size[0] + m.col_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Bottle,
            recipe_step: Some(ingredients.len() + index + 1),
            feeds_only: None,
            mirrored: false,
            rect: to_screen(x, y, bottle_size[0], bottle_size[1]),
        });
    }
    let aside_top = block_h + ASIDE_GAP;
    for (index, id) in aside.iter().enumerate() {
        let x = (index % aside_per_row) as f32 * (m.vial[0] + m.col_gap);
        let y = aside_top + (index / aside_per_row) as f32 * (m.vial[1] + m.row_gap);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Aside,
            recipe_step: None,
            feeds_only: None,
            mirrored: false,
            rect: to_screen(x, y, m.vial[0], m.vial[1]),
        });
    }
    let pot = to_screen(pot_x, (block_h - pot_block_h) * 0.5 + headroom, m.pot[0], m.pot[1]);
    let ghost_bottle = bottles.is_empty().then(|| to_screen(bottles_x, (block_h - SINGLE_BOTTLE_SIZE[1]) * 0.5, SINGLE_BOTTLE_SIZE[0], SINGLE_BOTTLE_SIZE[1]));
    let plaque_top = pot[1] + pot[3] + 8.0 * scale;
    let aside_row_top = to_screen(0.0, aside_top, 0.0, 0.0)[1] - ASIDE_GAP * 0.5 * scale;
    let block_bottom = to_screen(0.0, block_h, 0.0, 0.0)[1] + 12.0 * scale;
    let zone_top = if aside.is_empty() { block_bottom.max(plaque_top + 40.0 * scale) } else { aside_row_top.max(plaque_top + 40.0 * scale) };
    let zone_top = zone_top.min(size.1 - 84.0).max(0.0);
    let aside_zone = [0.0, zone_top, size.0, (size.1 - zone_top).max(84.0)];
    (readable_fit, BrewPlan { scale, pot, plaque_top, ghost_bottle, aside_zone, vial: m.vial, lanes: None, flank: false, cards })
}
