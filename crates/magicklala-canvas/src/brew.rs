use std::collections::{HashSet, VecDeque};

use magicklala_graph::NodeId;

use crate::{CanvasState, NodeCardModel, WireModel};

pub const VIAL_SIZE: [f32; 2] = [196.0, 58.0];
pub const POT_SIZE: [f32; 2] = [256.0, 248.0];
pub const SINGLE_BOTTLE_SIZE: [f32; 2] = [148.0, 196.0];
pub const MULTI_BOTTLE_SIZE: [f32; 2] = [150.0, 148.0];
const ROW_GAP: f32 = 22.0;
const COL_GAP: f32 = 28.0;
const SHELF_TO_POT: f32 = 92.0;
const POT_TO_BOTTLES: f32 = 104.0;
const PLAQUE_ROOM: f32 = 46.0;
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
    pub rect: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrewPlan {
    pub scale: f32,
    pub pot: [f32; 4],
    pub plaque_top: f32,
    pub ghost_bottle: Option<[f32; 4]>,
    pub aside_zone: [f32; 4],
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
    let has_incoming = |id: &NodeId| state.wires.iter().any(|wire| &wire.to_node == id);
    let mut queue: VecDeque<NodeId> =
        state.nodes.iter().filter(|card| card.category.eq_ignore_ascii_case("events") || !has_incoming(&card.id)).map(|card| card.id.clone()).collect();
    let mut seen: HashSet<NodeId> = queue.iter().cloned().collect();
    let mut order: Vec<NodeId> = Vec::with_capacity(state.nodes.len());
    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        let mut next: Vec<&WireModel> = state.wires.iter().filter(|wire| wire.from_node == node).collect();
        next.sort_by_key(|wire| !wire.ty.is_flow());
        for wire in next {
            if seen.insert(wire.to_node.clone()) {
                queue.push_back(wire.to_node.clone());
            }
        }
    }
    order.extend(state.nodes.iter().filter(|card| !seen.contains(&card.id)).map(|card| card.id.clone()));
    order
}

pub fn brewing_nodes(state: &CanvasState) -> HashSet<NodeId> {
    let bottles: Vec<NodeId> = state.nodes.iter().filter(|card| is_bottle(card)).map(|card| card.id.clone()).collect();
    if bottles.is_empty() {
        return state
            .nodes
            .iter()
            .filter(|card| state.wires.iter().any(|wire| wire.from_node == card.id || wire.to_node == card.id))
            .map(|card| card.id.clone())
            .collect();
    }
    upstream_of(state, &bottles)
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
    let cols = count.div_ceil(rows_fit.max(3));
    (cols, count.div_ceil(cols))
}

fn stack_height(count: usize, item: f32, gap: f32) -> f32 {
    if count == 0 { 0.0 } else { count as f32 * item + (count - 1) as f32 * gap }
}

pub fn brew_plan(state: &CanvasState, size: (f32, f32)) -> BrewPlan {
    let order = recipe_order(state);
    let brewing = brewing_nodes(state);
    let bottles: Vec<NodeId> = order.iter().filter(|id| state.card(id).is_some_and(is_bottle)).cloned().collect();
    let ingredients: Vec<NodeId> = order.iter().filter(|id| brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();
    let aside: Vec<NodeId> = order.iter().filter(|id| !brewing.contains(*id) && !bottles.contains(*id)).cloned().collect();

    let usable_h = (size.1 - TOP_ROOM - BOTTOM_ROOM).max(120.0);
    let rows_fit = ((usable_h + ROW_GAP) / (VIAL_SIZE[1] + ROW_GAP)).floor() as usize;
    let (cols, rows) = shelf_grid(ingredients.len(), rows_fit);
    let shelf_w = if cols == 0 { 0.0 } else { cols as f32 * VIAL_SIZE[0] + (cols - 1) as f32 * COL_GAP };
    let shelf_h = stack_height(rows, VIAL_SIZE[1], ROW_GAP);
    let bottle_size = if bottles.len() > 1 { MULTI_BOTTLE_SIZE } else { SINGLE_BOTTLE_SIZE };
    let bottles_h = stack_height(bottles.len().max(1), bottle_size[1], BOTTLE_GAP);
    let pot_block_h = POT_SIZE[1] + PLAQUE_ROOM;
    let block_h = shelf_h.max(pot_block_h).max(bottles_h);
    let pot_x = if cols == 0 { 0.0 } else { shelf_w + SHELF_TO_POT };
    let bottles_x = pot_x + POT_SIZE[0] + POT_TO_BOTTLES;
    let block_w = bottles_x + bottle_size[0];
    let aside_per_row = ((block_w + COL_GAP) / (VIAL_SIZE[0] + COL_GAP)).floor().max(1.0) as usize;
    let aside_rows = aside.len().div_ceil(aside_per_row);
    let aside_h = if aside.is_empty() { 0.0 } else { ASIDE_GAP + stack_height(aside_rows, VIAL_SIZE[1], ROW_GAP) };
    let design_w = block_w;
    let design_h = block_h + aside_h;

    let fit = ((size.0 - 2.0 * SIDE_ROOM) / design_w).min(usable_h / design_h).clamp(0.05, 1.0);
    let scale = fit * state.brew_camera.zoom;
    let anchor = [size.0 * 0.5 + state.brew_camera.pan_x, TOP_ROOM + usable_h * 0.5 + state.brew_camera.pan_y];
    let to_screen = |x: f32, y: f32, w: f32, h: f32| -> [f32; 4] {
        [anchor[0] + (x - design_w * 0.5) * scale, anchor[1] + (y - design_h * 0.5) * scale, w * scale, h * scale]
    };

    let mut cards = Vec::with_capacity(state.nodes.len());
    let shelf_top = (block_h - shelf_h) * 0.5;
    for (index, id) in ingredients.iter().enumerate() {
        let x = (index / rows.max(1)) as f32 * (VIAL_SIZE[0] + COL_GAP);
        let y = shelf_top + (index % rows.max(1)) as f32 * (VIAL_SIZE[1] + ROW_GAP);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Ingredient,
            recipe_step: Some(index + 1),
            feeds_only: feeds_only_one_bottle(state, id, bottles.len()),
            rect: to_screen(x, y, VIAL_SIZE[0], VIAL_SIZE[1]),
        });
    }
    let bottles_top = (block_h - bottles_h) * 0.5;
    for (index, id) in bottles.iter().enumerate() {
        let y = bottles_top + index as f32 * (bottle_size[1] + BOTTLE_GAP);
        cards.push(BrewCard {
            id: id.clone(),
            role: BrewRole::Bottle,
            recipe_step: Some(ingredients.len() + index + 1),
            feeds_only: None,
            rect: to_screen(bottles_x, y, bottle_size[0], bottle_size[1]),
        });
    }
    let aside_top = block_h + ASIDE_GAP;
    for (index, id) in aside.iter().enumerate() {
        let x = (index % aside_per_row) as f32 * (VIAL_SIZE[0] + COL_GAP);
        let y = aside_top + (index / aside_per_row) as f32 * (VIAL_SIZE[1] + ROW_GAP);
        cards.push(BrewCard { id: id.clone(), role: BrewRole::Aside, recipe_step: None, feeds_only: None, rect: to_screen(x, y, VIAL_SIZE[0], VIAL_SIZE[1]) });
    }
    let pot = to_screen(pot_x, (block_h - pot_block_h) * 0.5, POT_SIZE[0], POT_SIZE[1]);
    let ghost_bottle = bottles.is_empty().then(|| to_screen(bottles_x, (block_h - SINGLE_BOTTLE_SIZE[1]) * 0.5, SINGLE_BOTTLE_SIZE[0], SINGLE_BOTTLE_SIZE[1]));
    let plaque_top = pot[1] + pot[3] + 8.0 * scale;
    let aside_row_top = to_screen(0.0, aside_top, 0.0, 0.0)[1] - ASIDE_GAP * 0.5 * scale;
    let block_bottom = to_screen(0.0, block_h, 0.0, 0.0)[1] + 12.0 * scale;
    let zone_top = if aside.is_empty() { block_bottom.max(plaque_top + 40.0 * scale) } else { aside_row_top.max(plaque_top + 40.0 * scale) };
    let zone_top = zone_top.min(size.1 - 84.0).max(0.0);
    let aside_zone = [0.0, zone_top, size.0, (size.1 - zone_top).max(84.0)];
    BrewPlan { scale, pot, plaque_top, ghost_bottle, aside_zone, cards }
}
