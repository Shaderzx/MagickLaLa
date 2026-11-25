use magicklala_canvas::hearth::{
    HearthPalette, brand_cauldron_mark, category_tone, chevron_down_shape, empty_bottle_mark, hex, mini_vial_icon, search_icon_shape, sparkle_bar_v,
    sparkle_connector_h, stitched_sparkles_h, stream_ink, theme_icon_shape, with_alpha,
};
use magicklala_canvas::{ExecutionStatus, WorkbenchMode, interactive_canvas_view};
use magicklala_graph::{NodeSpec, RunStatus, SUBGRAPH_KIND, SocketType};
use rune::{Color, Cx, El, ScrollSpec, TextEdit, TextStyle, canvas, col, el, row, text, text_input};

use crate::{
    COMPACT_INSPECTOR_BELOW, COMPACT_PROMPT_EDITOR_HEIGHT, COMPACT_RUN_SLOT_WIDTH, CONTEXT_MENU_SIZE, ChatMessage, IDLE_TRACE_HEIGHT, LeftTab,
    PROMPT_EDITOR_HEIGHT, PROMPT_EDITOR_PADDING, QUICK_ADD_KIND, RUN_SLOT_WIDTH, RightTab, SUBGRAPH_SPELL_FIELD, StudioApp, TOOLBAR_HEIGHT, Theme, TraceLine,
    ValidationState, editable_field, palette_categories,
};

fn tones(theme: Theme) -> HearthPalette {
    HearthPalette::for_theme(theme == Theme::Light)
}

fn font(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
}

fn heading_font(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).tracking(-0.024 * size)
}

fn figure_font(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).features("'tnum' 1")
}

fn label(content: impl Into<String>, size: f32, weight: f32, color: Color) -> El {
    text(content).font(font(size, weight)).color(color)
}

fn heading(content: impl Into<String>, size: f32, weight: f32, color: Color) -> El {
    text(content).font(heading_font(size, weight)).color(color)
}

fn figure(content: impl Into<String>, size: f32, weight: f32, color: Color) -> El {
    text(content).font(figure_font(size, weight)).color(color)
}

fn category_title(category: &str) -> String {
    match category.to_ascii_lowercase().as_str() {
        "ai" => "AI".to_owned(),
        "io" => "Outputs".to_owned(),
        "json" => "JSON".to_owned(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ButtonKind {
    Primary,
    Secondary,
    Subtle,
    Danger,
    Disabled,
}

fn button_el(key: &str, caption: &str, kind: ButtonKind, padding: f32, t: &HearthPalette, action: fn(&mut StudioApp)) -> El {
    let (fill, ink, border, width) = match kind {
        ButtonKind::Primary => (t.primary_bg, t.primary_ink, t.primary_bg, 1.0),
        ButtonKind::Secondary => (t.card, t.ink, t.rule_2, 1.2),
        ButtonKind::Subtle => (t.wash_2, t.ink_2, t.rule, 1.0),
        ButtonKind::Danger => (t.hex_tint, t.hex, t.hex, 1.4),
        ButtonKind::Disabled => {
            if t.light {
                (t.off_bg, t.off_ink, with_alpha(t.rule, 0.55), 1.0)
            } else {
                (with_alpha(t.wash_2, 0.72), t.off_ink, with_alpha(t.rule_2, 0.45), 1.0)
            }
        }
    };
    let el = row()
        .key(key)
        .h(30.0)
        .px(padding)
        .radius(9.0)
        .continuous_corners()
        .items_center()
        .fill(fill)
        .border(width, border)
        .label(caption)
        .disabled(matches!(kind, ButtonKind::Disabled))
        .child(label(caption, 12.0, 650.0, ink));
    if matches!(kind, ButtonKind::Disabled) { el } else { el.on_press(action) }
}

fn sized_button(key: &str, caption: &str, primary: bool, danger: bool, padding: f32, t: &HearthPalette, action: fn(&mut StudioApp)) -> El {
    let kind = if danger {
        ButtonKind::Danger
    } else if primary {
        ButtonKind::Primary
    } else {
        ButtonKind::Secondary
    };
    button_el(key, caption, kind, padding, t, action)
}

struct MenuItemSpec<'a> {
    key: &'a str,
    caption: &'a str,
    shortcut: &'a str,
    enabled: bool,
    primary: bool,
    danger: bool,
}

fn menu_item(spec: MenuItemSpec<'_>, t: &HearthPalette, action: fn(&mut StudioApp)) -> El {
    let fill = if spec.primary { if t.light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.22) } } else { Color::TRANSPARENT };
    let ink = if !spec.enabled {
        t.off_ink
    } else if spec.danger {
        t.hex
    } else if spec.primary {
        if t.light { t.broth_deep } else { t.broth_foam }
    } else {
        t.ink
    };
    let item = row().key(spec.key).w_full().h(29.0).px(10.0).radius(8.0).continuous_corners().items_center().justify_between().fill(fill).child(label(
        spec.caption,
        12.0,
        if spec.primary || spec.danger { 650.0 } else { 500.0 },
        ink,
    ));
    let item = if spec.shortcut.is_empty() {
        item
    } else {
        item.child(text(spec.shortcut).font(if spec.shortcut == "⌫" { font(13.0, 500.0) } else { font(11.2, 500.0).mono() }).color(t.mute))
    };
    if spec.enabled { item.on_press(action) } else { item.role("button").label(spec.caption).disabled(true) }
}

fn context_tether(card: [f32; 4], menu: [f32; 4], t: &HearthPalette) -> Option<El> {
    let below = menu[1] >= card[1] + card[3];
    let above = menu[1] + menu[3] <= card[1];
    if below || above {
        let x = (card[0] + card[2] * 0.5).clamp(menu[0] + 24.0, menu[0] + menu[2] - 24.0).clamp(card[0] + 12.0, card[0] + card[2] - 12.0);
        let (card_y, menu_y) = if below { (card[1] + card[3], menu[1]) } else { (card[1], menu[1] + menu[3]) };
        let top = card_y.min(menu_y) - 4.0;
        let from = [4.0, card_y - top];
        let to = [4.0, menu_y - top];
        let thread = rune::Shape::new().line(from, to, 1.6, rune::LineCap::Round, t.gold);
        let thread = magicklala_canvas::hearth::fill_ellipse(thread, from[0], from[1], 3.0, 3.0, t.gold);
        let thread = magicklala_canvas::hearth::fill_ellipse(thread, to[0], to[1], 2.2, 2.2, t.gold);
        return Some(canvas(thread).key("context-tether").size(8.0, (menu_y - card_y).abs() + 8.0).absolute(x - 4.0, top));
    }
    let on_right = menu[0] >= card[0] + card[2];
    let card_edge = if on_right { card[0] + card[2] } else { card[0] };
    let menu_edge = if on_right { menu[0] } else { menu[0] + menu[2] };
    let menu_y = (card[1] + card[3] * 0.5).clamp(menu[1] + 20.0, menu[1] + 64.0);
    let card_y = menu_y.clamp(card[1] + card[3] * 0.3, card[1] + card[3] * 0.7);
    let left = card_edge.min(menu_edge) - 4.0;
    let top = menu_y.min(card_y) - 4.0;
    let width = (card_edge - menu_edge).abs() + 8.0;
    let height = (menu_y - card_y).abs() + 8.0;
    let from = [card_edge - left, card_y - top];
    let to = [menu_edge - left, menu_y - top];
    let bend = (to[0] - from[0]) * 0.55;
    let points: Vec<[f32; 2]> = (0..=16)
        .map(|step| {
            let k = step as f32 / 16.0;
            let c1 = [from[0] + bend, from[1]];
            let c2 = [to[0] - bend, to[1]];
            let mt = 1.0 - k;
            [
                mt * mt * mt * from[0] + 3.0 * mt * mt * k * c1[0] + 3.0 * mt * k * k * c2[0] + k * k * k * to[0],
                mt * mt * mt * from[1] + 3.0 * mt * mt * k * c1[1] + 3.0 * mt * k * k * c2[1] + k * k * k * to[1],
            ]
        })
        .collect();
    let thread = points.windows(2).fold(rune::Shape::new(), |shape, pair| shape.line(pair[0], pair[1], 1.6, rune::LineCap::Round, t.gold));
    let thread = magicklala_canvas::hearth::fill_ellipse(thread, from[0], from[1], 3.0, 3.0, t.gold);
    let thread = magicklala_canvas::hearth::fill_ellipse(thread, to[0], to[1], 2.2, 2.2, t.gold);
    Some(canvas(thread).key("context-tether").size(width, height).absolute(left, top))
}

const MODE_SWITCH_ZONE: [f32; 4] = [8.0, 6.0, 164.0, 44.0];

fn context_menu(app: &StudioApp, t: &HearthPalette) -> Option<(El, Option<El>, Option<[f32; 2]>)> {
    let anchor = app.canvas.context_menu?;
    let view = app.canvas_size();
    let plan = (app.canvas.mode == WorkbenchMode::Cauldron).then(|| magicklala_canvas::brew_plan(&app.canvas, view));
    let rows = match (app.canvas.selection.is_some(), plan.is_some()) {
        (true, true) => 4.0_f32,
        (true, false) => 3.0,
        (false, _) => 2.0,
    };
    let (menu_w, menu_h) = (CONTEXT_MENU_SIZE.0, (10.0 + 24.0 + 2.0 + rows * 29.0 + (rows + 2.0) * 2.0).min(CONTEXT_MENU_SIZE.1));
    let overlap_beyond = |[x, y]: [f32; 2], r: [f32; 4], slack: f32| {
        let w = (x + menu_w).min(r[0] + r[2]) - x.max(r[0]);
        let h = (y + menu_h).min(r[1] + r[3]) - y.max(r[1]);
        if w > slack && h > slack { w * h } else { 0.0 }
    };
    let overlap = |spot: [f32; 2], r: [f32; 4]| overlap_beyond(spot, r, 4.0);
    let pot_zone = plan.as_ref().map(|plan| {
        let body = plan.pot_body(false);
        let s = plan.scale;
        let top = body[1].min(plan.pot[1]);
        let left = body[0].min(plan.pot[0] - 32.0 * s);
        let right = (body[0] + body[2]).max(plan.pot[0] + plan.pot[2] + 32.0 * s);
        [left, top, right - left, (plan.plaque_top + 34.0 * s.max(0.6)) - top]
    });
    let picked = app.canvas.selection.clone();
    let others: Vec<[f32; 4]> = match &plan {
        Some(plan) => plan
            .cards
            .iter()
            .filter(|card| Some(&card.id) != picked.as_ref())
            .map(|card| [card.rect[0], card.rect[1] - 12.0, card.rect[2], card.rect[3] + 12.0])
            .collect(),
        None => app
            .canvas
            .nodes
            .iter()
            .filter(|card| Some(&card.id) != picked.as_ref())
            .map(|card| {
                let [x, y] = app.canvas.camera.world_to_screen(card.position);
                [x, y, card.size[0] * app.canvas.camera.zoom, card.size[1] * app.canvas.camera.zoom]
            })
            .collect(),
    };
    let others: Vec<[f32; 4]> = others.into_iter().chain(std::iter::once(MODE_SWITCH_ZONE)).collect();
    let clamp = |[x, y]: [f32; 2]| [x.min(view.0 - menu_w - 12.0).max(12.0), y.min(view.1 - menu_h - 12.0).max(12.0)];
    let beside = app.canvas.context_card.filter(|_| app.canvas.selection.is_some()).map(|card| {
        let below = card[1] + card[3] + 10.0;
        let above = card[1] - menu_h - 10.0;
        let mut columns = vec![card[0], card[0] + (card[2] - menu_w) * 0.5, card[0] + card[2] - menu_w];
        if let Some(zone) = pot_zone {
            columns.push(zone[0] - menu_w - 8.0);
            columns.push(zone[0] + zone[2] + 8.0);
        }
        let mut spots = vec![[card[0] + card[2] + 10.0, card[1]], [card[0] - menu_w - 10.0, card[1]]];
        for x in [card[0] + card[2] + 10.0, card[0] - menu_w - 10.0] {
            spots.push([x, card[1] + card[3] - menu_h]);
        }
        spots.extend(columns.iter().flat_map(|column| [[*column, below], [*column, above]]));
        let cost = |spot: [f32; 2]| {
            let pot = pot_zone.map_or(0.0, |zone| overlap_beyond(spot, zone, 0.0));
            let own = overlap(spot, card);
            let crowd: f32 = others.iter().map(|other| overlap(spot, *other)).sum();
            (pot > 0.0, own > 0.0, crowd.round() as i64)
        };
        let best = spots.into_iter().map(clamp).enumerate().min_by_key(|(order, spot)| (cost(*spot), *order)).map(|(_, spot)| spot).unwrap_or(clamp(anchor));
        let (pot, _, crowd) = cost(best);
        if pot || crowd > 0 { [view.0 + 8.0, 8.0] } else { best }
    });
    let docked = beside.is_some_and(|[x, _]| x > view.0);
    let [x, y] = beside.unwrap_or_else(|| clamp(anchor));
    let tether = app.canvas.context_card.filter(|_| beside.is_some() && !docked).and_then(|card| context_tether(card, [x, y, menu_w, menu_h], t));
    let selected_card = app.canvas.selected_card();
    let selected_title = selected_card.map(|card| app.canvas.display_title(&card.id));
    let quick_add_title = app.specs.iter().find(|spec| spec.kind == QUICK_ADD_KIND).map_or(QUICK_ADD_KIND, |spec| spec.title.as_str());
    let quick_add = format!("Add {quick_add_title}");
    let title_label = selected_title.as_deref().unwrap_or("Cauldron");
    let delete = "Remove";
    let header = row()
        .w_full()
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .justify_between()
        .child(if docked { el().grow(1.0) } else { label(title_label, 10.5, 700.0, t.mute).truncate().shrink().min_w(0.0) })
        .child(
            row()
                .key("context-dismiss")
                .size(20.0, 20.0)
                .radius(6.0)
                .items_center()
                .justify_center()
                .fill(t.wash_2)
                .child(label("×", 12.5, 700.0, t.ink_2))
                .on_press(StudioApp::close_context_menu),
        );
    let mut card = col()
        .key("context-menu")
        .w(CONTEXT_MENU_SIZE.0)
        .padding(5.0)
        .gap(2.0)
        .radius(13.0)
        .continuous_corners()
        .fill(t.card)
        .border(1.4, t.rule_2)
        .shadow(6.0, 20.0, t.shadow_soft)
        .child(header)
        .child(el().w_full().h(1.0).fill(t.rule));
    let idle = !app.is_running();
    let toss_item = selected_card.filter(|_| app.canvas.mode == WorkbenchMode::Cauldron).and_then(|card| {
        let plan = magicklala_canvas::brew_plan(&app.canvas, app.canvas_size());
        match plan.card(&card.id)?.role {
            magicklala_canvas::BrewRole::Ingredient => Some(("context-set-aside", "Set aside", StudioApp::set_aside_selected as fn(&mut StudioApp))),
            magicklala_canvas::BrewRole::Aside => Some(("context-pour-in", "Pour into cauldron", StudioApp::pour_selected_in as fn(&mut StudioApp))),
            magicklala_canvas::BrewRole::Bottle => Some(("context-set-aside", "Set aside", StudioApp::set_aside_selected as fn(&mut StudioApp))),
        }
    });
    card = match selected_title {
        Some(_) => card
            .child(menu_item(
                MenuItemSpec { key: "context-duplicate-node", caption: "Duplicate", shortcut: "", enabled: idle, primary: true, danger: false },
                t,
                StudioApp::duplicate_selected_node,
            ))
            .children(
                toss_item.map(|(key, caption, action)| {
                    menu_item(MenuItemSpec { key, caption, shortcut: "", enabled: idle, primary: false, danger: false }, t, action)
                }),
            )
            .child(if app.canvas.mode == WorkbenchMode::Grimoire {
                menu_item(
                    MenuItemSpec { key: "context-open-cauldron", caption: "Show in Cauldron", shortcut: "", enabled: true, primary: false, danger: false },
                    t,
                    StudioApp::open_cauldron,
                )
            } else {
                menu_item(
                    MenuItemSpec { key: "context-open-grimoire", caption: "Inspect in Grimoire", shortcut: "", enabled: true, primary: false, danger: false },
                    t,
                    StudioApp::open_grimoire,
                )
            })
            .child(el().w_full().h(1.0).fill(t.rule))
            .child(menu_item(
                MenuItemSpec { key: "context-delete-node", caption: delete, shortcut: "", enabled: idle, primary: false, danger: true },
                t,
                StudioApp::delete_selected_node,
            )),
        None => card
            .child(menu_item(
                MenuItemSpec {
                    key: &palette_key(QUICK_ADD_KIND).replacen("palette-", "context-add-", 1),
                    caption: &quick_add,
                    shortcut: "",
                    enabled: idle,
                    primary: true,
                    danger: false,
                },
                t,
                StudioApp::quick_add_from_menu,
            ))
            .child(el().w_full().h(1.0).fill(t.rule))
            .child(menu_item(
                MenuItemSpec { key: "context-fit-view", caption: "Fit canvas", shortcut: "", enabled: true, primary: false, danger: false },
                t,
                StudioApp::fit_view,
            )),
    };
    Some((card, tether, (!docked).then_some([x, y])))
}

fn status_chip(app: &StudioApp, empty: bool, t: &HearthPalette) -> El {
    let brewed = app.canvas.has_run() && !matches!(app.validation, ValidationState::Invalid(_));
    let green_bg = if t.light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.22) };
    let green_ink = if t.light { t.broth_deep } else { t.broth_foam };
    let (caption, bg, border, ink, dot, hollow) = if empty {
        ("Empty".to_owned(), t.wash_2, with_alpha(t.rule_2, 0.7), t.mute, t.mute, true)
    } else if app.is_running() {
        ("Brewing".to_owned(), t.wash_2, with_alpha(t.gold, 0.8), t.simmer_ink(), t.gold, false)
    } else if matches!(app.canvas.last_brew, Some(RunStatus::Failed | RunStatus::TimedOut)) {
        let caption = if app.canvas.last_brew == Some(RunStatus::TimedOut) { "Timed out" } else { "Failed" };
        (caption.to_owned(), t.hex_tint, with_alpha(t.hex, 0.75), t.hex, t.hex, false)
    } else if app.canvas.last_brew == Some(RunStatus::Cancelled) {
        ("Cancelled".to_owned(), t.wash_2, with_alpha(t.rule_2, 0.7), t.ink_2, t.mute, true)
    } else if matches!(app.validation, ValidationState::Invalid(_)) {
        ("Hexed".to_owned(), t.hex_tint, with_alpha(t.hex, 0.75), t.hex, t.hex, false)
    } else if brewed && app.active_release_version.is_some() {
        ("Draft brewed".to_owned(), green_bg, with_alpha(t.broth_mid, 0.72), green_ink, t.broth_mid, false)
    } else if brewed {
        ("Brewed".to_owned(), green_bg, with_alpha(t.broth_mid, 0.72), green_ink, t.broth_mid, false)
    } else if let Some(v) = app.active_release_version {
        (format!("Published v{v}"), green_bg, with_alpha(t.broth_mid, 0.72), green_ink, t.broth_mid, false)
    } else if matches!(app.validation, ValidationState::Valid) {
        ("Checked".to_owned(), green_bg, with_alpha(t.broth_mid, 0.72), green_ink, t.broth_mid, false)
    } else {
        ("Draft".to_owned(), t.wash_2, with_alpha(t.rule_2, 0.7), t.ink_2, t.gold, false)
    };
    let dot_el = if hollow { el().size(6.0, 6.0).radius(3.0).border(1.4, dot) } else { el().size(6.0, 6.0).radius(3.0).fill(dot) };
    row()
        .gap(5.5)
        .h(24.0)
        .px(8.5)
        .radius(12.0)
        .continuous_corners()
        .items_center()
        .fill(bg)
        .border(1.0, border)
        .child(dot_el)
        .child(label(caption, 11.0, 650.0, ink))
}

fn studio_text_width(text: &str, size: f32, weight: f32) -> f32 {
    thread_local! {
        static MEASURE: std::cell::RefCell<rune::text::TextSystem> = std::cell::RefCell::new(crate::studio_text_system());
    }
    MEASURE.with(|measure| measure.borrow_mut().measure(text, rune::TextStyle::new(size, weight), 1.0).0)
}

pub fn studio_view(app: &StudioApp, _: &mut Cx<StudioApp>) -> El {
    magicklala_canvas::measure::use_text_width(studio_text_width);
    let t = tones(app.theme);
    let middle = row()
        .key("workspace")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(left_rail(app, &t))
        .child(el().key("center").grow(1.0).min_w(0.0).h_full().child(canvas_with_menu(app, &t)))
        .child(inspector(app, &t, context_menu(app, &t).and_then(|(menu, _, spot)| spot.is_none().then_some(menu))));
    let studio = col().key("studio").w_full().h_full().fill(t.paper).child(toolbar(app, &t)).child(middle).child(trace_strip(app, &t));
    let root = el().key("studio-root").w_full().h_full().child(studio).on_layout(|app: &mut StudioApp, rect: [f32; 4]| app.follow_window((rect[2], rect[3])));
    match pantry_ghost(app, &t) {
        Some(ghost) => root.child(ghost),
        None => root,
    }
}

fn pantry_ghost(app: &StudioApp, t: &HearthPalette) -> Option<El> {
    let drag = app.pantry_drag.as_ref()?;
    let spec = app.specs.iter().find(|spec| spec.kind == drag.kind)?;
    let light = app.theme == Theme::Light;
    let tone = category_tone(&spec.category, light);
    let over = app.pantry_drag_over_pot();
    Some(
        row()
            .key("pantry-ghost")
            .gap(7.0)
            .px(9.0)
            .py(6.0)
            .radius(14.0)
            .continuous_corners()
            .items_center()
            .fill(t.card)
            .border(if over { 2.0 } else { 1.3 }, if over { t.gold_hi } else { tone.border })
            .shadow(6.0, 18.0, t.shadow_card)
            .opacity(0.94)
            .absolute(drag.at[0] + 12.0, drag.at[1] + 10.0)
            .child(canvas(mini_vial_icon(&spec.category, light, 20.0)).size(20.0, 20.0))
            .child(label(&spec.title, 12.0, 700.0, t.ink)),
    )
}

fn starter_shelf(app: &StudioApp, t: &HearthPalette) -> El {
    let size = app.canvas_size();
    let pot = magicklala_canvas::brew_plan(&app.canvas, size).pot;
    let light = app.theme == Theme::Light;
    let triggers: Vec<&NodeSpec> = app.specs.iter().filter(|spec| spec.is_trigger).collect();
    let vial = [176.0_f32, 34.0_f32];
    let gap = 8.0;
    let stack_h = triggers.len() as f32 * (vial[1] + gap) - gap;
    let left = pot[0] - vial[0] - 36.0;
    let x = if left >= 12.0 { left } else { (pot[0] + pot[2] + 36.0).min(size.0 - vial[0] - 12.0) };
    let y = (pot[1] + pot[3] * 0.5 - stack_h * 0.5).max(12.0);
    let tone = category_tone("events", light);
    triggers.iter().enumerate().fold(el().key("starter-shelf").absolute(0.0, 0.0), |shelf, (index, spec)| {
        let kind = spec.kind.clone();
        shelf.child(
            row()
                .key(format!("starter-{}", spec.kind.replace('/', "-")))
                .label(format!("Add {}", spec.title))
                .size(vial[0], vial[1])
                .px(10.0)
                .gap(8.0)
                .radius(17.0)
                .continuous_corners()
                .items_center()
                .fill(t.card)
                .border(1.4, tone.border)
                .shadow(0.0, 14.0, with_alpha(tone.liquid, if light { 0.35 } else { 0.5 }))
                .absolute(x, y + index as f32 * (vial[1] + gap))
                .child(canvas(mini_vial_icon(&spec.category, light, 18.0)).size(18.0, 18.0))
                .child(label(&spec.title, 12.0, 700.0, t.ink).truncate())
                .on_press(move |app: &mut StudioApp| {
                    app.spawn(&kind);
                }),
        )
    })
}

fn canvas_with_menu(app: &StudioApp, t: &HearthPalette) -> El {
    let mut board = interactive_canvas_view::<StudioApp>(&app.canvas, app.canvas_size(), |app| &mut app.canvas, StudioApp::toss);
    if app.canvas.nodes.is_empty() {
        let pantry_open = app.left_tab == LeftTab::Palette && !app.left_rail_collapsed();
        if app.canvas.mode == WorkbenchMode::Cauldron && !pantry_open {
            board = board.child(starter_shelf(app, t));
        }
    }
    match context_menu(app, t) {
        Some((menu, tether, Some([x, y]))) => board.children(tether).child(menu.absolute(x, y)),
        _ => board,
    }
}

fn toolbar(app: &StudioApp, t: &HearthPalette) -> El {
    let light = app.theme == Theme::Light;
    let compact = app.compact_toolbar();
    let empty_graph = app.canvas.nodes.is_empty();
    let (padding, gap, readout) = if compact { (7.0, 5.0, 34.0) } else { (11.0, 6.0, 42.0) };
    let divider = || el().w(1.0).h(20.0).fill(t.rule);
    let brand = row().gap(6.0).items_center().child(canvas(brand_cauldron_mark(light)).key("brand-orb").size(34.0, 28.0)).child(heading(
        "MagickLaLa",
        17.0,
        800.0,
        t.ink,
    ));
    let zoom = row()
        .gap(3.0)
        .items_center()
        .child(button_el("zoom-out", "−", ButtonKind::Subtle, if compact { 6.0 } else { 8.0 }, t, |app| {
            app.canvas.zoom_active(1.0 / 1.2, [app.canvas_size().0 / 2.0, app.canvas_size().1 / 2.0], app.canvas_size());
        }))
        .child(row().key("zoom-readout").w(readout).justify_center().child(figure(
            format!("{}%", app.canvas.active_camera().zoom_percent()),
            11.5,
            600.0,
            t.ink_2,
        )))
        .child(button_el("zoom-in", "+", ButtonKind::Subtle, if compact { 6.0 } else { 8.0 }, t, |app| {
            app.canvas.zoom_active(1.2, [app.canvas_size().0 / 2.0, app.canvas_size().1 / 2.0], app.canvas_size());
        }))
        .child(button_el("zoom-fit", "Fit", ButtonKind::Subtle, if compact { 6.0 } else { 9.0 }, t, StudioApp::fit_view));
    let plan = magicklala_canvas::brew_plan(&app.canvas, app.canvas_size());
    let ingredient_count = plan.with_role(magicklala_canvas::BrewRole::Ingredient).count();
    let aside_count = plan.with_role(magicklala_canvas::BrewRole::Aside).count();
    let bottle_count = plan.with_role(magicklala_canvas::BrewRole::Bottle).count();
    let i_word = if ingredient_count == 1 { "ingredient" } else { "ingredients" };
    let o_word = if bottle_count == 1 { "bottle" } else { "bottles" };
    let aside_part = if aside_count > 0 { format!(" · {aside_count} aside") } else { String::new() };
    let stats = if app.canvas.mode == WorkbenchMode::Grimoire {
        let w_word = if app.canvas.wires.len() == 1 { "wire" } else { "wires" };
        format!("{ingredient_count} {i_word}{aside_part} · {bottle_count} {o_word} · {} {w_word}", app.canvas.wires.len())
    } else {
        format!("{ingredient_count} {i_word}{aside_part} · {bottle_count} {o_word}")
    };
    let mut left = row()
        .key("toolbar-left")
        .gap(if compact { 5.0 } else { 11.0 })
        .items_center()
        .child(brand)
        .child(divider())
        .child(row().min_w(0.0).shrink().child(heading(&app.spell.name, 14.5, 750.0, t.ink).truncate()));
    if !empty_graph && !compact && app.canvas.mode == WorkbenchMode::Grimoire {
        let stats_box =
            row().h(24.0).px(8.5).radius(12.0).continuous_corners().items_center().fill(t.wash_2).border(1.0, t.rule).child(label(stats, 11.0, 550.0, t.ink_2));
        left = left.child(stats_box);
    }
    let hexed = matches!(app.validation, ValidationState::Invalid(_));
    let published_unchanged = app.is_unchanged_from_live_release();
    let check_kind = if empty_graph || app.is_running() { ButtonKind::Disabled } else { ButtonKind::Secondary };
    let brew_kind = if empty_graph || hexed || !app.has_recipe_trigger() { ButtonKind::Disabled } else { ButtonKind::Primary };
    let bottle_kind = if empty_graph || hexed || published_unchanged || app.is_running() { ButtonKind::Disabled } else { ButtonKind::Secondary };
    let save_kind = if (empty_graph && !app.dirty) || app.is_running() { ButtonKind::Disabled } else { ButtonKind::Secondary };
    let theme_caption = if app.theme == Theme::Dark { "Midnight" } else { "Sunlit" };
    let theme_a11y = if app.theme == Theme::Dark { "Switch to Sunlit" } else { "Switch to Midnight" };
    let theme_btn = if compact {
        row()
            .key("theme-toggle")
            .label(theme_a11y)
            .size(30.0, 30.0)
            .radius(9.0)
            .continuous_corners()
            .items_center()
            .justify_center()
            .fill(t.wash_2)
            .border(1.0, t.rule)
            .child(canvas(theme_icon_shape(app.theme == Theme::Light, light)).size(14.0, 14.0))
            .on_press(StudioApp::toggle_theme)
    } else {
        row()
            .key("theme-toggle")
            .label(theme_a11y)
            .h(30.0)
            .px(9.0)
            .gap(5.0)
            .radius(9.0)
            .continuous_corners()
            .items_center()
            .fill(t.wash_2)
            .border(1.0, t.rule)
            .child(canvas(theme_icon_shape(app.theme == Theme::Light, light)).size(14.0, 14.0))
            .child(label(theme_caption, 12.0, 650.0, t.ink_2))
            .on_press(StudioApp::toggle_theme)
    };
    let publish_btn = if hexed && !empty_graph {
        button_el("publish", "Publish", ButtonKind::Disabled, padding, t, |_| {}).on_press(|app: &mut StudioApp| {
            app.explain_publish_blocked();
        })
    } else {
        button_el("publish", "Publish", bottle_kind, padding, t, |app| {
            app.publish().ok();
        })
    };
    let right = row()
        .gap(gap)
        .items_center()
        .child(status_chip(app, empty_graph, t).key("validation-pill"))
        .child(button_el("validate", "Check", check_kind, padding, t, |app| {
            app.validate();
        }))
        .child(
            row().key("run-slot").w(if compact { COMPACT_RUN_SLOT_WIDTH } else { RUN_SLOT_WIDTH + 6.0 }).child(
                if app.is_running() {
                    button_el("cancel-run", "Quench", ButtonKind::Danger, padding, t, StudioApp::cancel_run)
                } else {
                    button_el("run-spell", if compact { "Brew" } else { "Brew potion" }, brew_kind, padding, t, |app| {
                        app.run_spell();
                    })
                }
                .w_full()
                .justify_center(),
            ),
        )
        .child(button_el("save", "Save", save_kind, padding, t, |app| {
            app.save_draft().ok();
        }))
        .child(publish_btn)
        .child(divider())
        .child(theme_btn)
        .child(divider())
        .child(zoom);
    row()
        .key("toolbar")
        .w_full()
        .h(TOOLBAR_HEIGHT)
        .px(if compact { 10.0 } else { 15.0 })
        .gap(gap)
        .items_center()
        .justify_between()
        .fill(t.shelf)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.rule)
        .child(left.min_w(0.0).shrink())
        .child(right)
}

fn tab_sized(key: &str, caption: &str, active: bool, compact: bool, light: bool, t: &HearthPalette, action: fn(&mut StudioApp)) -> El {
    let underline_w = if compact { 40.0 } else { 58.0 };
    let underline = if active {
        row().w_full().h(5.0).items_center().justify_center().child(canvas(sparkle_connector_h(underline_w, 5.0, light)).size(underline_w, 5.0))
    } else {
        el().w_full().h(5.0)
    };
    col()
        .key(key)
        .grow(1.0)
        .min_w(0.0)
        .h(38.0)
        .justify_between()
        .items_center()
        .child(row().grow(1.0).items_center().justify_center().child(label(
            caption,
            if compact { 11.5 } else { 12.5 },
            if active { 750.0 } else { 500.0 },
            if active { t.ink } else { t.mute },
        )))
        .child(underline)
        .on_press(action)
}

fn palette_key(kind: &str) -> String {
    format!("palette-{}", kind.replace('/', "-"))
}

fn left_rail(app: &StudioApp, t: &HearthPalette) -> El {
    let light = app.theme == Theme::Light;
    let rail_w = app.left_rail_width();
    if app.left_rail_collapsed() {
        let strip_btn = |key: &str, icon_el: El, caption: &str, action: fn(&mut StudioApp)| {
            col()
                .key(key)
                .label(format!("Expand {caption}"))
                .w(56.0)
                .h(54.0)
                .radius(11.0)
                .continuous_corners()
                .items_center()
                .justify_center()
                .gap(4.0)
                .fill(t.card)
                .border(1.0, t.rule_2)
                .child(icon_el)
                .child(label(caption, 10.6, 650.0, t.ink))
                .on_press(action)
        };
        return col()
            .key("left-rail")
            .w(rail_w)
            .h_full()
            .py(12.0)
            .gap(9.0)
            .items_center()
            .fill(t.shelf)
            .border_sides([0.0, 1.0, 0.0, 0.0], t.rule)
            .child(strip_btn("expand-nodes", canvas(mini_vial_icon("events", light, 20.0)).size(20.0, 20.0), "Pantry", |app| {
                app.left_tab = LeftTab::Palette;
                app.toggle_left_rail();
            }))
            .child(strip_btn("expand-agents", canvas(empty_bottle_mark(light, 20.0)).size(20.0, 20.0), "Potions", |app| {
                app.left_tab = LeftTab::Agents;
                app.toggle_left_rail();
            }));
    }
    let mut tabs = row()
        .w_full()
        .px(if app.compact_toolbar() { 8.0 } else { 12.0 })
        .gap(if app.compact_toolbar() { 10.0 } else { 8.0 })
        .items_center()
        .fill(t.shelf)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.rule)
        .child(tab_sized("tab-palette", "Pantry", app.left_tab == LeftTab::Palette, app.compact_toolbar(), light, t, |app| app.left_tab = LeftTab::Palette))
        .child(tab_sized("tab-agents", "Potions", app.left_tab == LeftTab::Agents, app.compact_toolbar(), light, t, |app| {
            app.left_tab = LeftTab::Agents;
        }));
    if app.compact_toolbar() {
        tabs = tabs.child(button_el("collapse-left-rail", "Hide", ButtonKind::Subtle, 7.0, t, StudioApp::toggle_left_rail));
    }
    let query = app.palette_query.text.trim().to_ascii_lowercase();
    let mut rail = col().key("left-rail").w(rail_w).h_full().min_h(0.0).fill(t.shelf).border_sides([0.0, 1.0, 0.0, 0.0], t.rule).child(tabs);
    match app.left_tab {
        LeftTab::Palette => {
            let pot_plan = magicklala_canvas::brew_plan(&app.canvas, app.canvas_size());
            let filtered: Vec<&NodeSpec> = app
                .specs
                .iter()
                .filter(|spec| {
                    query.is_empty()
                        || spec.title.to_ascii_lowercase().contains(&query)
                        || spec.kind.to_ascii_lowercase().contains(&query)
                        || spec.category.to_ascii_lowercase().contains(&query)
                })
                .collect();
            let search_field = text_input(&app.palette_query, font(12.0, 450.0))
                .single_line()
                .key("palette-search")
                .label("Search ingredients")
                .grow(1.0)
                .min_w(0.0)
                .h(16.0)
                .color(t.ink)
                .on_edit(|app: &mut StudioApp| &mut app.palette_query);
            let mut search_box = row()
                .w_full()
                .h(30.0)
                .px(9.0)
                .gap(7.0)
                .radius(9.0)
                .continuous_corners()
                .items_center()
                .fill(t.card)
                .border(1.0, t.rule_2)
                .child(canvas(search_icon_shape(light)).size(14.0, 14.0))
                .child(search_field);
            if app.palette_query.text.is_empty() {
                search_box = search_box.child(row().h(28.0).items_center().absolute(30.0, 1.0).child(label("Search pantry…", 12.0, 450.0, t.placeholder)));
            } else {
                search_box = search_box.child(
                    row()
                        .key("palette-inline-clear")
                        .px(8.0)
                        .h(24.0)
                        .min_w(44.0)
                        .radius(6.0)
                        .items_center()
                        .justify_center()
                        .fill(t.wash_2)
                        .border(1.0, t.rule_2)
                        .child(label("Clear", 11.0, 650.0, if light { t.broth_deep } else { t.broth_foam }))
                        .on_press(|app: &mut StudioApp| {
                            app.palette_query = TextEdit::new("");
                        }),
                );
            }
            let search_wrap = col().w_full().px(12.0).pt(10.0).pb(6.0).gap(5.0).child(search_box);
            let list_body = if filtered.is_empty() {
                col()
                    .w_full()
                    .pt(22.0)
                    .px(10.0)
                    .items_center()
                    .gap(8.0)
                    .child(canvas(empty_bottle_mark(light, 36.0)).size(36.0, 36.0))
                    .child(heading("No matching ingredients", 13.5, 750.0, t.ink))
                    .child(button_el("palette-clear-search", "Clear search", ButtonKind::Secondary, 12.0, t, |app| {
                        app.palette_query = TextEdit::new("");
                    }))
            } else {
                palette_categories(&app.specs).iter().enumerate().fold(col().gap(2.5).pb(44.0), |list, (cat_index, category)| {
                    let entries: Vec<&NodeSpec> = filtered.iter().copied().filter(|spec| spec.category.eq_ignore_ascii_case(category)).collect();
                    if entries.is_empty() {
                        return list;
                    }
                    let tone = category_tone(category, light);
                    let heading = row()
                        .w_full()
                        .pt(if cat_index == 0 { 4.0 } else { 9.0 })
                        .pb(3.0)
                        .px(3.0)
                        .items_center()
                        .justify_between()
                        .child(row().gap(6.0).items_center().child(el().size(7.0, 7.0).radius(3.5).fill(tone.liquid)).child(label(
                            category_title(category),
                            11.5,
                            750.0,
                            t.ink,
                        )))
                        .child(figure(entries.len().to_string(), 10.5, 600.0, t.mute));
                    entries.into_iter().fold(list.child(heading), |list, spec| {
                        let kind = spec.kind.clone();
                        let drag_kind = spec.kind.clone();
                        let in_pot = app.canvas.nodes.iter().any(|card| {
                            card.kind == spec.kind
                                && !app.canvas.is_shelved(&card.id)
                                && pot_plan.card(&card.id).is_some_and(|placed| placed.role != magicklala_canvas::BrewRole::Aside)
                        });
                        let mut right_actions = row().gap(4.0).items_center();
                        if in_pot {
                            right_actions = right_actions.child(
                                row()
                                    .key(format!("pantry-in-{}", spec.kind.replace('/', "-")))
                                    .px(5.0)
                                    .py(1.0)
                                    .radius(6.0)
                                    .fill(if light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.24) })
                                    .child(magicklala_canvas::hearth::check_mark(if light { t.broth_deep } else { t.broth_foam }, 10.0)),
                            );
                        }
                        right_actions = right_actions.child(
                            row()
                                .size(22.0, 22.0)
                                .radius(6.0)
                                .items_center()
                                .justify_center()
                                .fill(t.wash_2)
                                .border(1.0, t.rule)
                                .child(label("+", 13.0, 750.0, t.ink)),
                        );
                        let title_el = if !query.is_empty()
                            && let Some(pos) = spec.title.to_ascii_lowercase().find(&query)
                        {
                            let hit = rune::Span::new(pos, pos + query.len()).weight(800.0).color(if light { t.broth_deep } else { t.broth_foam });
                            label(&spec.title, 12.0, 550.0, t.ink).spans(vec![hit]).wrap().max_lines(2).shrink().min_w(0.0)
                        } else {
                            label(&spec.title, 12.0, 550.0, t.ink).wrap().max_lines(2).shrink().min_w(0.0)
                        };
                        let starter = spec.is_trigger && app.canvas.nodes.is_empty();
                        let item = row()
                            .key(palette_key(&spec.kind))
                            .w_full()
                            .min_h(29.0)
                            .py(3.0)
                            .px(7.0)
                            .gap(7.0)
                            .radius(9.0)
                            .continuous_corners()
                            .items_center()
                            .justify_between()
                            .fill(t.card)
                            .border(
                                if starter { 1.4 } else { 1.0 },
                                if starter {
                                    tone.border
                                } else if in_pot {
                                    with_alpha(t.broth_mid, 0.55)
                                } else {
                                    t.rule
                                },
                            )
                            .shadow(
                                0.0,
                                if starter { 12.0 } else { 0.0 },
                                if starter { with_alpha(tone.liquid, if light { 0.35 } else { 0.5 }) } else { Color::TRANSPARENT },
                            )
                            .child(
                                row()
                                    .grow(1.0)
                                    .min_w(0.0)
                                    .gap(7.0)
                                    .items_center()
                                    .child(if starter {
                                        el().key(format!("pantry-glow-{}", spec.kind.replace('/', "-")))
                                            .size(18.0, 18.0)
                                            .radius(9.0)
                                            .shadow(0.0, 10.0, with_alpha(tone.liquid, if light { 0.55 } else { 0.7 }))
                                            .child(canvas(mini_vial_icon(&spec.category, light, 18.0)).size(18.0, 18.0))
                                    } else {
                                        canvas(mini_vial_icon(&spec.category, light, 18.0)).size(18.0, 18.0)
                                    })
                                    .child(title_el),
                            )
                            .child(right_actions)
                            .on_press(move |app: &mut StudioApp| {
                                app.spawn(&kind);
                            })
                            .on_drag(move |app: &mut StudioApp, drag, tx| {
                                tx.instant();
                                match drag.phase {
                                    rune::Phase::Down => app.pantry_drag = None,
                                    rune::Phase::Move if drag.dx.hypot(drag.dy) >= 6.0 => app.pantry_drag_move(&drag_kind, drag.window_point()),
                                    rune::Phase::Move => {}
                                    rune::Phase::Up => {
                                        app.pantry_drop(drag.cancelled());
                                    }
                                }
                            });
                        list.child(item)
                    })
                })
            };
            let footer_text = if !query.is_empty() {
                let m_word = if filtered.len() == 1 { "match" } else { "matches" };
                format!("{} {m_word}", filtered.len())
            } else {
                format!("{} ingredients", app.specs.len())
            };
            let scroll_list = col()
                .key("left-rail-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(11.0)
                .pb(8.0)
                .clip_rounded(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list_body);
            let footer = el().w_full().child(
                col()
                    .w_full()
                    .child(el().w_full().h(1.0).fill(t.rule))
                    .child(row().w_full().h(28.0).px(12.0).items_center().justify_between().fill(t.wash_2).child(label(footer_text, 10.5, 550.0, t.ink_2))),
            );
            let footer_wrap = footer;
            rail = rail.child(search_wrap).child(scroll_list).child(footer_wrap);
        }
        LeftTab::Agents => {
            rail = rail.child(
                col()
                    .key("left-rail-scroll")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .px(12.0)
                    .pb(14.0)
                    .clip_rounded(0.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(agents_and_releases(app, light, t)),
            );
        }
    }
    rail
}

fn agents_and_releases(app: &StudioApp, light: bool, t: &HearthPalette) -> El {
    let section_bar = |title: &str| row().w_full().pt(14.0).pb(6.0).child(heading(title, 14.5, 800.0, t.ink));
    let spell_invalid = matches!(app.validation, ValidationState::Invalid(_));
    let roles = magicklala_canvas::brew_plan(&app.canvas, app.canvas_size());
    let ingr_count = roles.with_role(magicklala_canvas::BrewRole::Ingredient).count();
    let aside_count = roles.with_role(magicklala_canvas::BrewRole::Aside).count();
    let ingr_word = if ingr_count == 1 { "ingredient" } else { "ingredients" };
    let ingr_word = if aside_count > 0 { format!("{ingr_word} · {aside_count} aside") } else { ingr_word.to_owned() };
    let spells_header = row().w_full().pt(14.0).pb(6.0).items_center().justify_between().child(heading("Potions", 14.5, 800.0, t.ink)).child(button_el(
        "new-spell",
        "+ New",
        ButtonKind::Secondary,
        9.0,
        t,
        StudioApp::request_new_spell,
    ));
    let mut agent_names: Vec<String> = Vec::new();
    if app.spell.name != "Support Agent" && app.spell.name != "Research Agent" {
        agent_names.push(app.spell.name.clone());
    }
    agent_names.push("Support Agent".to_owned());
    agent_names.push("Research Agent".to_owned());
    for (saved, _) in &app.saved_potions {
        if !agent_names.contains(saved) {
            agent_names.push(saved.clone());
        }
    }
    let agent_rows = agent_names.iter().fold(col().gap(6.0), |list, name| {
        let active = name == &app.spell.name;
        let published = app.active_release_version;
        let shelved_live = (!active).then(|| app.saved_live.iter().find(|(saved, _)| saved == name).map(|(_, version)| *version)).flatten();
        let draft_tested = active && app.canvas.has_run() && !spell_invalid && published.is_some();
        let live_pill = format!("v{} Live", published.unwrap_or(1));
        let (sub_label, badge_label, badge_bg, badge_border, badge_ink, is_action) = if active && spell_invalid {
            (format!("{ingr_count} {ingr_word}"), "Hexed", t.hex_tint, t.hex, t.hex, false)
        } else if draft_tested {
            (
                format!("v{} live · draft tested", published.unwrap_or(1)),
                "Tested",
                if light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.24) },
                t.broth_mid,
                if light { t.broth_deep } else { t.broth_foam },
                false,
            )
        } else if active && published.is_some() {
            (
                format!("{ingr_count} {ingr_word}"),
                live_pill.as_str(),
                if light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.24) },
                t.broth_mid,
                if light { t.broth_deep } else { t.broth_foam },
                false,
            )
        } else if active {
            (
                match app.last_published.as_ref() {
                    Some(release) => format!("{ingr_count} {ingr_word} · v{} live", release.version),
                    None => format!("{ingr_count} {ingr_word}"),
                },
                "Draft",
                with_alpha(t.gold, if light { 0.14 } else { 0.22 }),
                t.gold,
                if light { t.ink } else { t.gold_hi },
                false,
            )
        } else {
            let sample_count = match app.saved_potions.iter().find(|(saved, _)| saved == name) {
                Some((_, summary)) => summary.clone(),
                None if name == "Research Agent" => "3 ingredients".to_owned(),
                None => "4 ingredients".to_owned(),
            };
            let sample_count = match shelved_live {
                Some(version) => format!("{sample_count} · v{version} live"),
                None => sample_count,
            };
            (sample_count, "Open ›", t.card, t.rule_2, t.ink, true)
        };
        let badge = row()
            .px(if is_action { 7.5 } else { 6.5 })
            .py(if is_action { 3.0 } else { 2.0 })
            .radius(if is_action { 7.0 } else { 8.0 })
            .border(if is_action { 1.2 } else { 1.0 }, badge_border)
            .fill(badge_bg)
            .child(label(badge_label, if is_action { 10.2 } else { 10.5 }, if is_action { 650.0 } else { 700.0 }, badge_ink));
        let open_name = name.clone();
        let card = row()
            .key(format!("spell-{}", name.to_ascii_lowercase().replace(' ', "-")))
            .w_full()
            .px(8.0)
            .py(9.0)
            .gap(6.0)
            .radius(12.0)
            .continuous_corners()
            .items_center()
            .justify_between()
            .fill(t.card)
            .border(
                if active { 1.5 } else { 1.0 },
                if active && spell_invalid {
                    t.hex
                } else if active {
                    t.broth_mid
                } else {
                    t.rule
                },
            )
            .child(
                row()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(7.0)
                    .items_center()
                    .child(
                        el().key(format!("potion-mark-{}", name.to_ascii_lowercase().replace(' ', "-"))).size(24.0, 30.0).child(
                            canvas(magicklala_canvas::hearth::potion_bottle_shape(
                                potion_mood(app, name),
                                light,
                                56.0,
                                70.0,
                                (active && published.is_some()) || shelved_live.is_some(),
                                None,
                            ))
                            .size(56.0, 70.0)
                            .absolute(-16.0, -20.0)
                            .scale(0.43),
                        ),
                    )
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(1.0)
                            .child(label(name, 12.0, if active { 750.0 } else { 550.0 }, t.ink).truncate())
                            .child(label(sub_label, 10.5, 500.0, t.mute).truncate()),
                    ),
            )
            .child(badge)
            .on_press(move |app: &mut StudioApp| {
                app.request_open_sample(&open_name);
            });
        list.child(card)
    });
    let mut column = col().gap(6.0).child(spells_header).child(agent_rows);
    if app.pending_switch.is_some() {
        column =
            column.child(
                col()
                    .key("confirm-switch")
                    .w_full()
                    .padding(8.0)
                    .gap(6.0)
                    .radius(10.0)
                    .fill(t.card)
                    .border(1.2, t.gold)
                    .child(label("Discard unsaved edits?", 11.0, 650.0, t.ink))
                    .child(
                        row()
                            .gap(6.0)
                            .child(button_el("confirm-switch-go", "Switch", ButtonKind::Primary, 9.0, t, StudioApp::confirm_switch))
                            .child(button_el("confirm-switch-cancel", "Cancel", ButtonKind::Subtle, 9.0, t, StudioApp::cancel_switch)),
                    ),
            );
    }
    let mut column = column.child(section_bar("Releases"));
    if let Some(error) = &app.store_error {
        column = column.child(
            col()
                .w_full()
                .mt(4.0)
                .padding(10.0)
                .radius(11.0)
                .continuous_corners()
                .fill(t.hex_tint)
                .border(1.2, t.hex)
                .child(label(error, 11.5, 600.0, t.hex).wrap()),
        );
    } else if app.releases.is_empty() {
        let empty_card = col()
            .w_full()
            .py(18.0)
            .px(10.0)
            .radius(14.0)
            .continuous_corners()
            .fill(t.card)
            .border(1.0, t.rule)
            .items_center()
            .gap(8.0)
            .child(canvas(empty_bottle_mark(light, 44.0)).key("releases-bobbin").size(44.0, 44.0))
            .child(heading("No releases yet", 14.5, 800.0, t.ink))
            .child(row().pt(2.0).child(button_el(
                "empty-publish",
                "Publish",
                if spell_invalid || app.canvas.nodes.is_empty() || app.is_running() { ButtonKind::Disabled } else { ButtonKind::Secondary },
                14.0,
                t,
                |app| {
                    app.publish().ok();
                },
            )));
        column = column.child(empty_card);
    }
    let show_status = app.draft_status.as_ref().or_else(|| app.publish_status.as_ref().filter(|_| app.active_release_version.is_none()));
    if app.store_error.is_none()
        && let Some(status) = show_status
    {
        column = column.child(
            row().w_full().mt(2.0).padding(8.0).radius(10.0).fill(t.card).border(1.0, t.broth_mid).child(label(status, 11.5, 650.0, t.ink).key("draft-status")),
        );
    }
    app.releases.iter().rev().fold(column, |list, release| {
        let is_active = app.active_release_version == Some(release.version);
        let is_pending = app.pending_restore_version == Some(release.version);
        let is_live = app.last_published.as_ref().is_some_and(|live| live.version == release.version);
        let parts = app.release_graphs.get(&release.version).map_or(release.nodes, |graph| {
            let canvas = magicklala_canvas::CanvasState::from_graph(graph, &app.specs);
            magicklala_canvas::brew_plan(&canvas, app.canvas_size()).with_role(magicklala_canvas::BrewRole::Ingredient).count()
        });
        let p_word = if parts == 1 { "ingredient" } else { "ingredients" };
        let stamp = el().size(28.0, 24.0).radius(8.0).continuous_corners().fill(if is_live { t.primary_bg } else { t.wash }).center().child(figure(
            format!("v{}", release.version),
            10.5,
            750.0,
            if is_live { t.primary_ink } else { t.ink },
        ));
        let version = release.version;
        let restore = row()
            .key(format!("restore-v{version}"))
            .px(6.5)
            .py(2.5)
            .radius(8.0)
            .fill(Color::TRANSPARENT)
            .border(1.0, t.rule_2)
            .child(label("Restore", 9.8, 650.0, t.ink_2))
            .on_press(move |app: &mut StudioApp| {
                app.request_restore_release(version);
            });
        let live_pill = row().px(7.0).py(2.5).radius(8.0).fill(t.primary_bg).child(label("Live", 9.8, 750.0, t.primary_ink));
        let right_action = match (is_live, is_active) {
            (true, true) => row().child(live_pill),
            (true, false) => row().gap(4.0).items_center().child(restore).child(live_pill),
            (false, _) => row().child(restore),
        };
        let top_row = row()
            .w_full()
            .gap(6.0)
            .items_center()
            .justify_between()
            .child(
                row().grow(1.0).min_w(0.0).gap(7.0).items_center().child(stamp).child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(1.0)
                        .child(label(format!("{parts} {p_word}"), 11.2, 650.0, t.ink).truncate())
                        .children(app.release_notes.get(&release.version).map(|note| label(note, 9.8, 500.0, t.mute).truncate())),
                ),
            )
            .child(right_action);
        let mut entry = col()
            .key(format!("release-v{version}"))
            .w_full()
            .px(8.0)
            .py(9.0)
            .gap(6.0)
            .radius(12.0)
            .continuous_corners()
            .fill(t.card)
            .border(
                if is_active || is_pending { 1.5 } else { 1.0 },
                if is_active {
                    t.broth_mid
                } else if is_pending {
                    t.gold
                } else {
                    t.rule
                },
            )
            .child(top_row);
        if is_pending {
            let confirm_bar = row()
                .w_full()
                .pt(4.0)
                .gap(5.0)
                .items_center()
                .justify_between()
                .border_sides([1.0, 0.0, 0.0, 0.0], t.rule)
                .child(label(format!("Load v{version} to draft?"), 10.0, 600.0, t.ink_2))
                .child(
                    row()
                        .gap(4.0)
                        .items_center()
                        .child(
                            row()
                                .key(format!("confirm-restore-v{version}"))
                                .px(6.5)
                                .py(2.0)
                                .radius(6.0)
                                .fill(t.primary_bg)
                                .child(label("Load", 9.8, 700.0, t.primary_ink))
                                .on_press(StudioApp::confirm_restore_to_draft),
                        )
                        .child(
                            row()
                                .key(format!("cancel-restore-v{version}"))
                                .px(6.0)
                                .py(2.0)
                                .radius(6.0)
                                .fill(t.wash_2)
                                .border(1.0, t.rule)
                                .child(label("Cancel", 9.8, 600.0, t.ink_2))
                                .on_press(StudioApp::cancel_restore_release),
                        ),
                );
            entry = entry.child(confirm_bar);
        }
        list.child(entry)
    })
}

fn inspector(app: &StudioApp, t: &HearthPalette, docked: Option<El>) -> El {
    let light = app.theme == Theme::Light;
    let compact = app.compact_toolbar();
    let panel_w = app.inspector_width();
    let trace_tab = tab_sized("tab-trace", "Trace", app.right_tab == RightTab::Trace, compact, light, t, |app| app.right_tab = RightTab::Trace);
    let tabs = row()
        .w_full()
        .px(if compact { 6.0 } else { 10.0 })
        .gap(if compact { 2.0 } else { 4.0 })
        .fill(t.shelf)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.rule)
        .child(tab_sized("tab-inspector", "Recipe", app.right_tab == RightTab::Inspector, compact, light, t, |app| app.right_tab = RightTab::Inspector))
        .child(trace_tab)
        .child(tab_sized("tab-variables", "Variables", app.right_tab == RightTab::Variables, compact, light, t, |app| app.right_tab = RightTab::Variables))
        .child(tab_sized("tab-playground", "Tasting", app.right_tab == RightTab::Playground, compact, light, t, |app| app.right_tab = RightTab::Playground));
    let mut panel = col().key("inspector").w(panel_w).h_full().min_h(0.0).fill(t.shelf).border_sides([0.0, 0.0, 0.0, 1.0], t.rule).child(tabs);
    let opened = app.canvas.context_menu;
    if docked.is_some() && app.docked_menu_seen != opened {
        panel = panel.child(el().key("docked-menu-opener").size(0.0, 0.0).on_layout(move |app: &mut StudioApp, _| {
            if app.docked_menu_seen != opened {
                app.docked_menu_seen = opened;
                app.right_tab = RightTab::Inspector;
            }
        }));
    }
    let docked = docked.filter(|_| app.right_tab == RightTab::Inspector).map(El::w_full);
    match app.right_tab {
        RightTab::Playground => panel = panel.child(playground_panel(app, light, t)),
        RightTab::Trace => {
            panel = panel.child(
                col()
                    .key("inspector-scroll")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .px(14.0)
                    .pb(14.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(run_summary_inspector(app, t)),
            );
        }
        RightTab::Inspector => {
            panel = panel.child(
                col()
                    .key("inspector-scroll")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .px(14.0)
                    .pb(14.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .children(docked.map(|menu| row().w_full().pt(8.0).pb(4.0).child(menu)))
                    .child(inspector_body(app, light, t)),
            );
        }
        RightTab::Variables => {
            panel = panel.child(
                col()
                    .key("inspector-scroll")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .px(14.0)
                    .pb(14.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(variables_table(app, t)),
            );
        }
    }
    panel
}

fn variables_table(app: &StudioApp, t: &HearthPalette) -> El {
    let count = app.spell.variables.len();
    let count_word = if count == 1 { "1 variable" } else { &format!("{count} variables") };
    let top_bar = row()
        .w_full()
        .pt(12.0)
        .pb(10.0)
        .items_center()
        .justify_between()
        .child(col().gap(1.0).child(heading("Variables", 14.5, 800.0, t.ink)).child(label(count_word, 11.0, 500.0, t.mute)))
        .child(button_el("add-variable", "+ Add", ButtonKind::Secondary, 10.0, t, StudioApp::add_variable));
    let list = col().gap(8.0).child(top_bar);
    if app.spell.variables.is_empty() {
        return list.child(col().w_full().py(24.0).items_center().gap(6.0).child(label("No variables yet", 12.5, 500.0, t.mute)));
    }
    app.spell.variables.iter().enumerate().fold(list, |list, (idx, variable)| {
        let menu_open = app.open_var_type == Some(idx);
        let pending_reset = app.pending_type_reset.filter(|(open, _)| *open == idx).map(|(_, ty)| ty);
        let (bound_hint, bound_active) = app.variable_binding_hint(app.var_name_edits.get(idx).map_or(variable.name.as_str(), |edit| edit.text.trim()));
        let name_widget = if let Some(name_edit) = app.var_name_edits.get(idx) {
            text_input(name_edit, font(12.0, 700.0).mono())
                .single_line()
                .key(format!("var-name-{idx}"))
                .label(format!("Variable {} name", variable.name))
                .w_full()
                .h(18.0)
                .color(t.ink)
                .on_edit(move |app: &mut StudioApp| {
                    if app.is_running() {
                        return &mut app.frozen_edit;
                    }
                    while app.var_name_edits.len() <= idx {
                        app.var_name_edits.push(TextEdit::new(""));
                    }
                    &mut app.var_name_edits[idx]
                })
        } else {
            text(&variable.name).font(font(12.0, 700.0).mono()).color(t.ink).truncate()
        };
        let name_problem = app.var_name_error(idx);
        let name_col = col()
            .key(format!("var-label-{idx}"))
            .grow(1.0)
            .min_w(0.0)
            .gap(1.5)
            .child(name_widget)
            .children(name_problem.map(|problem| row().key(format!("var-name-error-{idx}")).child(label(problem, 10.5, 650.0, t.hex))))
            .child(label(bound_hint, 11.0, 600.0, if bound_active { if t.light { t.broth_deep } else { t.broth_foam } } else { t.mute }));
        let controls = row()
            .gap(6.0)
            .items_center()
            .child(
                row()
                    .key(format!("var-type-{idx}"))
                    .w(74.0)
                    .h(28.0)
                    .px(8.0)
                    .radius(8.0)
                    .continuous_corners()
                    .items_center()
                    .justify_between()
                    .fill(t.wash_2)
                    .border(1.0, if menu_open || pending_reset.is_some() { t.broth_mid } else { t.rule_2 })
                    .child(label(format!("{:?}", variable.ty), 11.5, 650.0, t.ink))
                    .child(canvas(chevron_down_shape(t.ink_2)).size(9.0, 7.0))
                    .on_press(move |app: &mut StudioApp| {
                        app.toggle_var_type_menu_at(idx);
                    }),
            )
            .child(
                row()
                    .px(7.0)
                    .h(26.0)
                    .radius(7.0)
                    .items_center()
                    .justify_center()
                    .fill(Color::TRANSPARENT)
                    .border(1.0, t.rule)
                    .child(label("Remove", 10.5, 600.0, t.ink_2))
                    .on_press(move |app: &mut StudioApp| {
                        app.remove_variable_at(idx);
                    }),
            );
        let row_top = if app.compact_toolbar() {
            col().w_full().gap(6.0).child(name_col).child(controls)
        } else {
            row().w_full().gap(6.0).items_center().justify_between().child(name_col).child(controls)
        };
        let default_input = if let Some(def_edit) = app.var_default_edits.get(idx) {
            let mut field = row().grow(1.0).min_w(0.0).h(18.0).items_center().child(
                text_input(def_edit, font(11.5, 550.0).mono())
                    .single_line()
                    .key(format!("var-default-{idx}"))
                    .label(format!("{} default value", variable.name))
                    .w_full()
                    .h(18.0)
                    .color(t.ink)
                    .on_edit(move |app: &mut StudioApp| {
                        if app.is_running() {
                            return &mut app.frozen_edit;
                        }
                        while app.var_default_edits.len() <= idx {
                            app.var_default_edits.push(TextEdit::new(""));
                        }
                        &mut app.var_default_edits[idx]
                    }),
            );
            if def_edit.text.is_empty() {
                field = field.child(row().h(18.0).items_center().absolute(0.0, 0.0).child(label("Enter value…", 11.0, 450.0, t.placeholder)));
            }
            match app.var_default_error(idx) {
                Some(problem) => col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(field.border_sides([0.0, 0.0, 1.5, 0.0], t.hex))
                    .child(row().key(format!("var-default-error-{idx}")).child(label(problem, 10.5, 650.0, t.hex))),
                None => field,
            }
        } else {
            text("—").font(font(11.5, 550.0).mono()).color(t.mute)
        };
        let row_default = row().w_full().gap(8.0).items_center().child(row().w(48.0).child(label("Default", 11.0, 600.0, t.mute))).child(
            row().grow(1.0).min_w(0.0).h(30.0).px(9.0).radius(8.0).continuous_corners().items_center().fill(t.paper).border(1.0, t.rule_2).child(default_input),
        );
        let mut var_card = col().w_full().padding(10.0).gap(8.0).radius(12.0).continuous_corners().fill(t.card).border(1.0, t.rule_2).child(row_top);
        if let Some(target_ty) = pending_reset {
            var_card = var_card.child(
                row()
                    .w_full()
                    .px(8.0)
                    .py(6.0)
                    .gap(6.0)
                    .radius(8.0)
                    .items_center()
                    .justify_between()
                    .fill(t.wash_2)
                    .border(1.0, t.gold)
                    .child(label(format!("Reset value for {target_ty:?}?"), 10.2, 600.0, t.ink).truncate())
                    .child(
                        row()
                            .gap(4.0)
                            .child(
                                row()
                                    .key(format!("confirm-type-reset-{idx}"))
                                    .px(6.5)
                                    .py(2.0)
                                    .radius(6.0)
                                    .fill(t.primary_bg)
                                    .child(label("Reset", 9.8, 700.0, t.primary_ink))
                                    .on_press(StudioApp::confirm_variable_type_reset),
                            )
                            .child(
                                row()
                                    .key(format!("cancel-type-reset-{idx}"))
                                    .px(6.0)
                                    .py(2.0)
                                    .radius(6.0)
                                    .fill(t.card)
                                    .border(1.0, t.rule)
                                    .child(label("Keep", 9.8, 600.0, t.ink_2))
                                    .on_press(StudioApp::cancel_variable_type_reset),
                            ),
                    ),
            );
        }
        if menu_open {
            let current_ty = variable.ty;
            let choices = [SocketType::String, SocketType::Int, SocketType::Float, SocketType::Bool, SocketType::Json, SocketType::Any];
            let dropdown = choices.into_iter().fold(
                col()
                    .key(format!("var-type-menu-{idx}"))
                    .w_full()
                    .padding(4.0)
                    .gap(2.0)
                    .radius(9.0)
                    .continuous_corners()
                    .fill(t.paper)
                    .border(1.2, t.broth_mid),
                |menu, choice| {
                    let selected = choice == current_ty;
                    menu.child(
                        row()
                            .w_full()
                            .h(25.0)
                            .px(8.0)
                            .radius(6.5)
                            .items_center()
                            .justify_between()
                            .fill(if selected { if t.light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.22) } } else { Color::TRANSPARENT })
                            .child(label(format!("{choice:?}"), 11.5, if selected { 700.0 } else { 500.0 }, t.ink))
                            .child(if selected {
                                magicklala_canvas::hearth::check_mark(if t.light { t.broth_deep } else { t.broth_foam }, 11.0)
                            } else {
                                el().size(11.0, 11.0)
                            })
                            .on_press(move |app: &mut StudioApp| app.set_variable_type_at(idx, choice)),
                    )
                },
            );
            var_card = var_card.child(dropdown);
        }
        list.child(var_card.child(row_default))
    })
}

fn node_diagnostics(app: &StudioApp, card: &magicklala_canvas::NodeCardModel) -> Vec<String> {
    let mut found = Vec::new();
    if matches!(app.validation, ValidationState::Invalid(_)) {
        found.extend(app.hex_findings.iter().filter(|finding| finding.node.as_ref() == Some(&card.id)).map(|finding| finding.readable.clone()));
    }
    if found.is_empty() && card.status == ExecutionStatus::Failed {
        let own = app.trace.iter().rev().find(|line| line.kind == "NodeFailed" && line.node.as_ref() == Some(&card.id)).map(|line| {
            if line.detail.contains("no LLM gateway is configured") {
                format!("{} · set MAGICKLALA_LLM_BASE_URL and MAGICKLALA_LLM_API_KEY, then reopen Studio", line.detail)
            } else {
                line.detail.clone()
            }
        });
        let elsewhere = app.trace.iter().find(|line| line.kind == "NodeFailed").map(|line| format!("Stopped when {}", line.detail));
        found.push(own.or(elsewhere).unwrap_or_else(|| format!("{} failed during the brew.", app.canvas.display_title(&card.id))));
    }
    found
}

fn inspector_section_heading(title: &str, t: &HearthPalette) -> El {
    row().w_full().pb(4.0).border_sides([0.0, 0.0, 1.0, 0.0], t.rule_2).child(label(title, 12.0, 750.0, t.ink))
}

fn run_summary_inspector(app: &StudioApp, t: &HearthPalette) -> El {
    if app.trace.is_empty() {
        return col()
            .key("inspector-trace-empty")
            .w_full()
            .pt(96.0)
            .px(12.0)
            .items_center()
            .gap(10.0)
            .child(canvas(empty_bottle_mark(app.theme == Theme::Light, 42.0)).size(42.0, 42.0))
            .child(heading("No brews yet", 15.5, 800.0, t.ink));
    }
    let finished: Vec<&TraceLine> = app.trace.iter().filter(|line| line.kind == "NodeFinished" || line.kind == "NodeFailed").collect();
    let total_ms = app.trace.last().map_or(0, |line| line.at_ms);
    let total_label = if total_ms == 0 { "<1ms total".to_owned() } else { format!("{total_ms}ms total") };
    let succeeded = !app.trace.iter().any(|line| line.status == ExecutionStatus::Failed);
    let checked_only = !app.is_running() && finished.is_empty() && app.trace.iter().all(|line| line.kind == "RunRejected");
    let outcome = app.trace.iter().rev().find(|line| line.kind == "RunFinished").map(|line| line.detail.as_str());
    let status_label = if checked_only {
        "Hexed"
    } else if app.is_running() {
        "Brewing"
    } else if app.canvas.last_brew.is_none() && app.last_run.is_some() {
        "Last brew"
    } else if outcome == Some("Cancelled") {
        "Cancelled"
    } else if outcome == Some("TimedOut") {
        "Timed out"
    } else if succeeded {
        "Brewed"
    } else {
        "Failed"
    };
    let header = row()
        .w_full()
        .px(12.0)
        .py(10.0)
        .radius(13.0)
        .continuous_corners()
        .fill(if succeeded { with_alpha(t.broth_mid, if t.light { 0.14 } else { 0.22 }) } else { t.hex_tint })
        .border(1.2, if succeeded { t.broth_mid } else { t.hex })
        .items_center()
        .justify_between()
        .child(
            col()
                .gap(2.0)
                .child(
                    row()
                        .gap(6.0)
                        .items_center()
                        .child(heading(if checked_only { "Check".to_owned() } else { format!("Brew #{}", app.runs.max(1)) }, 15.5, 800.0, t.ink))
                        .child(label("·", 14.0, 600.0, t.mute))
                        .child(heading(status_label, 15.5, 800.0, if succeeded { if t.light { t.broth_deep } else { t.broth_foam } } else { t.hex })),
                )
                .child(label(
                    if checked_only {
                        plural(app.trace.len(), "issue", "issues")
                    } else {
                        format!(
                            "{} · {} · {total_label}",
                            plural(crate::top_step_count(&app.trace), "step", "steps"),
                            plural(app.trace.len(), "event", "events")
                        )
                    },
                    11.0,
                    500.0,
                    t.summary_ink(),
                )),
        );
    let listed: Vec<(String, &TraceLine)> = if checked_only {
        app.trace.iter().filter(|line| line.kind == "RunRejected").enumerate().map(|(index, line)| ((index + 1).to_string(), line)).collect()
    } else {
        crate::numbered_steps(&app.trace)
    };
    let steps = listed.iter().enumerate().fold(col().gap(5.0), |list, (index, (number, line))| {
        let target = line.canvas_node();
        let mut item = row()
            .key(if checked_only { format!("trace-issue-{index}") } else { format!("trace-step-{index}") })
            .w_full()
            .min_h(34.0)
            .py(6.0)
            .px(9.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .justify_between()
            .fill(t.card)
            .border(1.0, t.rule)
            .child(
                row()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(8.0)
                    .items_center()
                    .child(
                        row()
                            .min_w(18.0)
                            .h(18.0)
                            .px(if number.len() > 1 { 4.0 } else { 0.0 })
                            .radius(6.0)
                            .fill(if checked_only || line.status == ExecutionStatus::Failed { t.hex } else { t.primary_bg })
                            .items_center()
                            .justify_center()
                            .child(figure(
                                number.clone(),
                                10.0,
                                750.0,
                                if checked_only || line.status == ExecutionStatus::Failed { t.card } else { t.primary_ink },
                            )),
                    )
                    .child(if checked_only {
                        label(&line.detail, 12.0, 550.0, t.ink).wrap()
                    } else {
                        let (name, took) = step_name_and_time(app, line);
                        let cause =
                            (line.kind == "NodeFailed").then(|| line.detail.split_once(": ").map_or(line.detail.as_str(), |(_, cause)| cause).to_owned());
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(1.0)
                            .child(label(name, 12.0, 600.0, t.ink).wrap())
                            .children(cause.map(|cause| label(cause, 11.0, 600.0, t.hex).wrap()))
                            .children(took.map(|took| text(took).font(font(10.5, 550.0).mono().features("'tnum' 1")).color(t.mute)))
                    }),
            )
            .child(row().px(7.0).py(2.5).radius(7.0).fill(t.wash_2).border(1.0, t.rule).child(label(
                if app.compact_toolbar() { "›" } else { "Inspect ›" },
                10.5,
                650.0,
                t.ink_2,
            )));
        if let Some(node_id) = target {
            item = item.on_press(move |app: &mut StudioApp| {
                app.canvas.select(Some(node_id.clone()));
                app.right_tab = RightTab::Inspector;
            });
        }
        list.child(item)
    });
    col()
        .key("inspector-trace")
        .gap(10.0)
        .pt(12.0)
        .child(header)
        .child(inspector_section_heading(if checked_only { "Issues" } else { "Brewing sequence" }, t))
        .child(steps)
}

fn step_name_and_time(app: &StudioApp, line: &TraceLine) -> (String, Option<String>) {
    let (head, took) = match line.detail.rsplit_once(" · ") {
        Some((head, took)) if took.ends_with("ms") || took.ends_with('s') => (head.to_owned(), Some(took.to_owned())),
        _ => (line.detail.clone(), None),
    };
    (line.node.as_ref().filter(|id| app.canvas.card(id).is_some()).map_or(head, |id| app.canvas.display_title(id)), took)
}

fn recipe_overview(app: &StudioApp, light: bool, t: &HearthPalette) -> El {
    use magicklala_canvas::BrewRole;
    let plan = magicklala_canvas::brew_plan(&app.canvas, app.canvas_size());
    let order = magicklala_canvas::recipe_order(&app.canvas);
    let group = |role: BrewRole| -> Vec<&magicklala_canvas::NodeCardModel> {
        order.iter().filter(|id| plan.card(id).is_some_and(|placed| placed.role == role)).filter_map(|id| app.canvas.card(id)).collect()
    };
    let (ingredients, bottles, aside) = (group(BrewRole::Ingredient), group(BrewRole::Bottle), group(BrewRole::Aside));
    let mut counts = vec![plural(ingredients.len(), "ingredient", "ingredients"), plural(bottles.len(), "bottle", "bottles")];
    if !aside.is_empty() {
        counts.push(format!("{} aside", aside.len()));
    }
    let header = col()
        .key("recipe-overview-head")
        .w_full()
        .px(12.0)
        .py(10.0)
        .gap(2.0)
        .radius(13.0)
        .continuous_corners()
        .fill(t.card)
        .border(1.2, t.rule_2)
        .child(heading(&app.spell.name, 15.5, 800.0, t.ink).wrap())
        .child(label(counts.join(" · "), 11.0, 500.0, t.summary_ink()));
    let entry = |card: &magicklala_canvas::NodeCardModel| {
        let id = card.id.clone();
        let (word, ink) = match card.status {
            ExecutionStatus::Succeeded => (Some("Brewed"), if light { t.broth_deep } else { t.broth_foam }),
            ExecutionStatus::Failed => (Some("Failed"), t.hex),
            ExecutionStatus::Running => (Some("Simmering"), t.ink_2),
            ExecutionStatus::Idle if app.canvas.brew_completed() && !app.canvas.is_shelved(&card.id) => (Some("Untaken"), t.mute),
            ExecutionStatus::Idle => (None, t.mute),
        };
        row()
            .key(format!("recipe-row-{}", card.id.0))
            .w_full()
            .min_h(32.0)
            .px(9.0)
            .py(5.0)
            .gap(8.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .fill(t.card)
            .border(1.0, t.rule)
            .child(canvas(mini_vial_icon(&card.category, light, 16.0)).size(16.0, 16.0))
            .child(label(app.canvas.display_title(&card.id), 12.0, 600.0, t.ink).wrap_anywhere().grow(1.0).min_w(0.0))
            .children(word.map(|word| label(word, 10.5, 700.0, ink)))
            .on_press(move |app: &mut StudioApp| app.canvas.select(Some(id.clone())))
    };
    let section = |title: &str, cards: &[&magicklala_canvas::NodeCardModel]| {
        let empty_pot = app.canvas.nodes.is_empty() && title != "Set aside";
        (!cards.is_empty() || empty_pot).then(|| {
            let list = col().w_full().gap(5.0).child(inspector_section_heading(title, t));
            if cards.is_empty() {
                list.child(
                    row()
                        .w_full()
                        .min_h(32.0)
                        .px(9.0)
                        .radius(10.0)
                        .continuous_corners()
                        .items_center()
                        .border(1.0, with_alpha(t.rule_2, 0.8))
                        .child(label("None yet", 11.0, 500.0, t.mute)),
                )
            } else {
                cards.iter().fold(list, |list, card| list.child(entry(card)))
            }
        })
    };
    col()
        .key("recipe-overview")
        .w_full()
        .gap(10.0)
        .pt(12.0)
        .child(header)
        .children(section("In the pot", &ingredients))
        .children(section("Bottles", &bottles))
        .children(section("Set aside", &aside))
}

fn inspector_body(app: &StudioApp, light: bool, t: &HearthPalette) -> El {
    let Some(card) = app.canvas.selected_card() else {
        return recipe_overview(app, light, t);
    };
    let compact = app.size.1 < COMPACT_INSPECTOR_BELOW;
    let socket_h = if compact { 21.0 } else { 24.0 };
    let diagnostics = node_diagnostics(app, card);
    let has_error = !diagnostics.is_empty();
    let tone = category_tone(&card.category, light);
    let socket_row_el = |socket: &magicklala_canvas::SocketModel, is_input: bool| {
        let flagged = is_input && socket.flagged;
        let wired = app
            .canvas
            .wires
            .iter()
            .any(|w| if is_input { w.to_node == card.id && w.to_socket == socket.name } else { w.from_node == card.id && w.from_socket == socket.name });
        let supplied =
            is_input && editable_field(&card.kind) == Some(socket.name.as_str()) && app.prompts.get(&card.id).is_some_and(|e| !e.text.trim().is_empty());
        let marker = if socket.ty.is_flow() {
            el().size(8.5, 8.5).radius(4.25).fill(if flagged { t.hex } else { t.socket_flow_mark() })
        } else if flagged {
            el().size(7.0, 7.0).radius(2.0).fill(t.card).border(1.5, t.hex)
        } else if wired || supplied {
            el().size(7.0, 7.0).radius(2.0).fill(stream_ink(&card.category, light))
        } else {
            el().size(7.0, 7.0).radius(2.0).fill(t.card).border(1.5, stream_ink(&card.category, light))
        };
        let mut right_meta = row().gap(8.0).items_center();
        if flagged {
            right_meta = right_meta
                .child(label(if socket.mistyped { "Wrong type" } else { "Missing" }, 10.5, 700.0, t.hex).key(format!("socket-missing-{}", socket.name)));
        }
        let fed_from = app
            .canvas
            .wires
            .iter()
            .find(|w| is_input && !socket.ty.is_flow() && w.to_node == card.id && w.to_socket == socket.name)
            .map(|w| format!("from {} · {}", app.canvas.display_title(&w.from_node), w.from_socket));
        let has_plug_button = is_input && !socket.ty.is_flow() && !app.is_running();
        if has_plug_button {
            let name = socket.name.clone();
            let node = card.id.clone();
            let wire_id = app.canvas.wires.iter().find(|w| w.to_node == card.id && w.to_socket == socket.name).map(|w| w.id.clone());
            let action = match wire_id {
                Some(wire) => row()
                    .key(format!("unplug-{name}"))
                    .min_h(24.0)
                    .min_w(24.0)
                    .px(10.0)
                    .radius(8.0)
                    .continuous_corners()
                    .items_center()
                    .justify_center()
                    .fill(t.card)
                    .border(1.0, t.rule_2)
                    .shadow(1.0, 3.0, with_alpha(t.ink, 0.08))
                    .child(label("Unplug", 11.0, 650.0, t.ink))
                    .on_press(move |app: &mut StudioApp| app.unplug_wire(&wire)),
                None => row()
                    .key(format!("plug-{name}"))
                    .min_h(24.0)
                    .min_w(24.0)
                    .px(10.0)
                    .radius(8.0)
                    .continuous_corners()
                    .items_center()
                    .justify_center()
                    .fill(if flagged { t.hex } else { t.card })
                    .border(1.0, if flagged { t.hex } else { t.rule_2 })
                    .shadow(1.0, 3.0, with_alpha(t.ink, 0.08))
                    .child(label("Plug in", 11.0, 700.0, if flagged { t.primary_ink } else { t.ink }))
                    .on_press(move |app: &mut StudioApp| app.toggle_plug_menu(&node, &name)),
            };
            right_meta = right_meta.child(action);
        }
        right_meta = right_meta.child(label(format!("{:?}", socket.ty), if compact { 10.5 } else { 11.0 }, 550.0, if flagged { t.hex } else { t.mute }));
        row()
            .w_full()
            .min_h(socket_h)
            .py(if has_plug_button { 0.0 } else { 2.0 })
            .gap(8.0)
            .items_center()
            .justify_between()
            .border_sides([0.0, 0.0, 1.0, 0.0], if flagged { with_alpha(t.hex, 0.42) } else { t.rule })
            .child(
                row().grow(1.0).min_w(0.0).gap(8.0).items_center().child(marker).child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            label(
                                &socket.name,
                                if compact { 11.5 } else { 12.0 },
                                if socket.ty.is_flow() || flagged { 650.0 } else { 500.0 },
                                if flagged { t.hex } else { t.ink },
                            )
                            .wrap_anywhere(),
                        )
                        .children(fed_from.map(|source| label(source, 10.0, 500.0, t.mute).key(format!("plug-source-{}", socket.name)).wrap_anywhere())),
                ),
            )
            .child(right_meta)
    };
    let plug_menu_el = |socket: &magicklala_canvas::SocketModel| -> Option<El> {
        let (node, name) = app.plug_menu.as_ref()?;
        if node != &card.id || name != &socket.name {
            return None;
        }
        let sources = app.earlier_fitting_outputs(&card.id, &socket.name);
        let mut menu =
            col().key(format!("plug-menu-{}", socket.name)).w_full().mt(4.0).mb(4.0).padding(6.0).gap(3.0).radius(10.0).fill(t.card).border(1.0, t.rule_2);
        if sources.is_empty() {
            menu = menu.child(label(format!("No earlier step makes {}", crate::type_words(socket.ty)), 11.0, 550.0, t.mute).wrap());
        }
        for source in sources {
            let to = magicklala_canvas::SocketRef::new(card.id.clone(), socket.name.clone());
            let caption = format!("{} · {}", app.canvas.display_title(&source.node), source.socket);
            menu = menu.child(
                row()
                    .key(format!("plug-{}-{}-{}", socket.name, source.node.0, source.socket))
                    .w_full()
                    .px(8.0)
                    .py(5.0)
                    .radius(7.0)
                    .fill(t.wash)
                    .child(label(caption, 11.0, 600.0, t.ink).wrap_anywhere().grow(1.0).min_w(0.0))
                    .on_press(move |app: &mut StudioApp| app.plug_input(&source, &to)),
            );
        }
        Some(menu)
    };
    let mut sockets = col().gap(0.0);
    if !card.inputs.is_empty() {
        sockets = sockets.child(row().w_full().pt(2.0).pb(2.0).child(label("Inputs", 10.5, 650.0, t.mute)));
        sockets = card.inputs.iter().fold(sockets, |list, socket| list.child(socket_row_el(socket, true)).children(plug_menu_el(socket)));
    }
    if !card.outputs.is_empty() {
        sockets = sockets.child(row().w_full().pt(if compact { 4.0 } else { 6.0 }).pb(2.0).child(label("Outputs", 10.5, 650.0, t.mute)));
        sockets = card.outputs.iter().fold(sockets, |list, socket| list.child(socket_row_el(socket, false)));
    }
    let id = card.id.clone();
    let field = editable_field(&card.kind);
    let editor_height = if has_error || (compact && app.trace_height() > IDLE_TRACE_HEIGHT) { 86.0 } else { prompt_editor_height(app.size) };
    let wired_source = field.and_then(|field| {
        app.canvas.wires.iter().find(|wire| wire.to_node == card.id && wire.to_socket == field).map(|wire| {
            let title = app.canvas.card(&wire.from_node).map_or_else(|| wire.from_node.0.clone(), |source| app.canvas.display_title(&source.id));
            (format!("From {title} · {}", wire.from_socket), wire.id.clone())
        })
    });
    let wired_editor = wired_source.map(|(source, wire)| {
        let unplug =
            sized_button("prompt-unplug", "Type it instead", false, false, 10.0, t, |_| {}).on_press(move |app: &mut StudioApp| app.unplug_wire(&wire));
        let shell =
            col().gap(if compact { 6.0 } else { 8.0 }).child(label(source, if compact { 12.0 } else { 12.5 }, 650.0, t.ink).wrap()).child(row().child(unplug));
        shell.key("prompt-field").w_full().padding(PROMPT_EDITOR_PADDING).radius(12.0).continuous_corners().fill(t.card).border(1.2, t.rule_2)
    });
    let editor = field.filter(|_| wired_editor.is_none()).and_then(|field| app.prompts.get(&card.id).map(|edit| (field, edit))).map(|(field, edit)| {
        let input = text_input(edit, prompt_font()).key("prompt-field").label(format!("{} {field}", card.title)).w_full().color(t.ink).on_edit(
            move |app: &mut StudioApp| if app.is_running() { &mut app.frozen_edit } else { app.prompts.entry(id.clone()).or_insert_with(|| TextEdit::new("")) },
        );
        let mut box_el = col()
            .key("prompt-editor")
            .w_full()
            .h(editor_height)
            .padding(PROMPT_EDITOR_PADDING)
            .radius(12.0)
            .continuous_corners()
            .fill(t.card)
            .border(1.5, if has_error && edit.text.is_empty() { t.hex } else { t.rule_2 })
            .clip_rounded(12.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(input);
        if edit.text.is_empty() {
            box_el = box_el.child(label(format!("{}…", editor_caption(field)), 11.5, 450.0, t.placeholder).wrap());
        }
        let literal: Vec<String> = if card.kind.starts_with("ai/") {
            crate::template_roots(&edit.text).into_iter().map(|word| format!("{{{{{word}}}}}")).collect()
        } else {
            Vec::new()
        };
        let verb = if literal.len() == 1 { "reaches" } else { "reach" };
        let note = (!literal.is_empty())
            .then(|| label(format!("{} {verb} the model as typed", literal.join(", ")), 11.5, 550.0, t.mute).wrap().key("prompt-literal-note"));
        col().w_full().gap(6.0).child(box_el).children(note)
    });
    let card_header = row()
        .w_full()
        .px(10.0)
        .py(if compact { 7.0 } else { 9.0 })
        .gap(10.0)
        .radius(13.0)
        .continuous_corners()
        .items_center()
        .fill(tone.tint)
        .border(1.2, tone.border)
        .child(
            el().size(if compact { 28.0 } else { 34.0 }, if compact { 28.0 } else { 34.0 }).radius(9.0).fill(t.card).border(1.0, tone.border).center().child(
                canvas(mini_vial_icon(&card.category, light, if compact { 22.0 } else { 26.0 }))
                    .size(if compact { 22.0 } else { 26.0 }, if compact { 22.0 } else { 26.0 }),
            ),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.5)
                .child(heading(app.canvas.display_title(&card.id), if compact { 14.5 } else { 15.5 }, 800.0, t.ink).wrap())
                .child(text(&card.kind).font(font(10.5, 500.0).mono()).color(t.mute).truncate()),
        );
    let mut column = col().key("inspector-card").gap(if compact { 6.0 } else { 9.0 }).pt(if compact { 8.0 } else { 10.0 }).child(card_header);
    if has_error {
        let flagged_socket = card.inputs.iter().find(|socket| socket.flagged).map(|socket| socket.name.clone());
        let mistyped = card.inputs.iter().any(|socket| socket.flagged && socket.mistyped);
        let headline = match &flagged_socket {
            Some(socket) if !mistyped => format!("{} needs \"{socket}\"", app.canvas.display_title(&card.id)),
            _ => diagnostics.first().cloned().unwrap_or_default(),
        };
        let mend = app.hex_mend_source(&card.id);
        let fix_button = match (&mend, flagged_socket.as_deref()) {
            (Some((_, caption)), _) => Some(button_el("inspector-fix", caption, ButtonKind::Primary, 9.0, t, StudioApp::mend_selected_hex)),
            (None, Some("prompt")) => {
                Some(button_el("inspector-fix", "Use default prompt", ButtonKind::Primary, 9.0, t, StudioApp::fill_default_prompt_for_selected))
            }
            _ => None,
        };
        let mut actions = if app.compact_toolbar() { col().pt(2.0).gap(6.0).items_start() } else { row().pt(2.0).gap(8.0).items_center() };
        if let Some(button) = fix_button {
            actions = actions.child(button);
        }
        column = column.child(
            col()
                .key("inspector-error")
                .w_full()
                .padding(10.0)
                .gap(6.0)
                .radius(12.0)
                .continuous_corners()
                .fill(t.hex_tint)
                .border(1.4, t.hex)
                .child(label(headline, 12.0, 750.0, t.hex).wrap())
                .child(actions.child(button_el("inspector-open-grimoire", "Grimoire", ButtonKind::Secondary, 8.0, t, StudioApp::open_grimoire))),
        );
    }
    let mut prompt_block = match (field, editor, wired_editor) {
        (Some(field), Some(editor), _) => {
            Some(col().w_full().gap(if compact { 6.0 } else { 9.0 }).child(inspector_section_heading(editor_caption(field), t)).child(editor))
        }
        (Some(field), None, Some(wired)) => {
            Some(col().w_full().gap(if compact { 6.0 } else { 9.0 }).child(inspector_section_heading(editor_caption(field), t)).child(wired))
        }
        _ => None,
    };
    if has_error {
        column = column.children(prompt_block.take());
    }
    let mut column = column.child(inspector_section_heading("Sockets", t)).child(sockets);
    if matches!(card.kind.as_str(), "ai/generate_text" | "ai/stream_text" | "ai/structured_json") {
        let temp_value = app.selected_temperature().map_or_else(|| "Auto".to_owned(), |temperature| temperature.to_string());
        let model_name = app.selected_model_name();
        let narrow_inspector = app.compact_toolbar();
        let tile_py = if compact || has_error { 5.0 } else { 7.0 };
        let meta = row()
            .w_full()
            .gap(if narrow_inspector { 6.0 } else { 8.0 })
            .child(
                col()
                    .key("model-picker")
                    .grow(1.0)
                    .min_w(0.0)
                    .justify_between()
                    .px(if narrow_inspector { 7.0 } else { 9.0 })
                    .py(tile_py)
                    .gap(3.0)
                    .radius(10.0)
                    .continuous_corners()
                    .fill(t.card)
                    .border(1.0, t.rule_2)
                    .child(label(format!("Model · {}", provider_name(&app.selected_provider())), 10.0, 600.0, t.mute).truncate())
                    .child(
                        row()
                            .w_full()
                            .h(22.0)
                            .gap(4.0)
                            .items_center()
                            .justify_between()
                            .child(text(&model_name).font(font(if narrow_inspector { 10.2 } else { 11.5 }, 650.0).mono()).color(t.ink).truncate())
                            .child(canvas(chevron_down_shape(t.ink_2)).size(9.0, 7.0)),
                    )
                    .on_press(StudioApp::toggle_model_menu),
            )
            .child(
                col()
                    .key("temperature-tile")
                    .grow(1.0)
                    .min_w(0.0)
                    .justify_between()
                    .px(if narrow_inspector { 6.0 } else { 8.0 })
                    .py(tile_py)
                    .gap(3.0)
                    .radius(10.0)
                    .continuous_corners()
                    .fill(t.card)
                    .border(1.0, t.rule_2)
                    .child(match app.selected_temperature_source() {
                        Some(source) if source.eq_ignore_ascii_case("temperature") => {
                            label("Temperature", 10.5, 650.0, if light { t.broth_deep } else { t.broth_foam })
                        }
                        Some(source) if compact => label(source, 10.5, 650.0, if light { t.broth_deep } else { t.broth_foam }).wrap(),
                        Some(source) => label(format!("Temperature · {source}"), 10.5, 650.0, if light { t.broth_deep } else { t.broth_foam }).wrap(),
                        None => label("Temperature", 10.5, 600.0, t.mute),
                    })
                    .child(
                        row()
                            .w_full()
                            .h(22.0)
                            .items_center()
                            .justify_between()
                            .child(
                                row()
                                    .key("temp-dec")
                                    .size(if narrow_inspector { 20.0 } else { 22.0 }, if narrow_inspector { 20.0 } else { 22.0 })
                                    .radius(6.0)
                                    .fill(t.wash_2)
                                    .items_center()
                                    .justify_center()
                                    .child(label("−", 12.0, 700.0, t.ink_2))
                                    .on_press(|app: &mut StudioApp| app.adjust_temperature(-0.1)),
                            )
                            .child(text(temp_value).font(font(if narrow_inspector { 10.5 } else { 11.5 }, 650.0).mono()).color(t.ink))
                            .child(
                                row()
                                    .key("temp-inc")
                                    .size(if narrow_inspector { 20.0 } else { 22.0 }, if narrow_inspector { 20.0 } else { 22.0 })
                                    .radius(6.0)
                                    .fill(t.wash_2)
                                    .items_center()
                                    .justify_center()
                                    .child(label("+", 12.0, 700.0, t.ink_2))
                                    .on_press(|app: &mut StudioApp| app.adjust_temperature(0.1)),
                            ),
                    ),
            );
        column = column.child(meta);
        if app.model_menu_open {
            let choices = [("openai", "gpt-4o-mini"), ("openai", "gpt-4o"), ("anthropic", "claude-sonnet-4"), ("gemini", "gemini-2.5-pro")];
            let menu = choices.into_iter().fold(
                col().key("model-dropdown").w_full().padding(4.0).gap(2.0).radius(10.0).continuous_corners().fill(t.card).border(1.2, t.broth_mid),
                |list, (provider, choice)| {
                    let selected = choice == model_name;
                    let pick_provider = provider.to_owned();
                    let pick_model = choice.to_owned();
                    list.child(
                        row()
                            .w_full()
                            .h(26.0)
                            .px(8.0)
                            .radius(7.0)
                            .items_center()
                            .justify_between()
                            .fill(if selected { if t.light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.22) } } else { Color::TRANSPARENT })
                            .child(text(choice).font(font(11.5, if selected { 700.0 } else { 500.0 }).mono()).color(t.ink))
                            .child(if selected {
                                magicklala_canvas::hearth::check_mark(if t.light { t.broth_deep } else { t.broth_foam }, 11.0)
                            } else {
                                el().size(11.0, 11.0)
                            })
                            .on_press(move |app: &mut StudioApp| app.select_model(&pick_provider, &pick_model)),
                    )
                },
            );
            column = column.child(menu);
        }
    }
    let column = column.pb(if compact { 4.0 } else { 8.0 }).children(prompt_block);
    match release_version_editor(app, card, t) {
        Some(version) => column.child(inspector_section_heading("Release version", t)).child(version),
        None => column,
    }
}

fn release_version_editor(app: &StudioApp, card: &magicklala_canvas::NodeCardModel, t: &HearthPalette) -> Option<El> {
    let edit = app.versions.get(&card.id).filter(|_| card.kind == SUBGRAPH_KIND)?;
    let id = card.id.clone();
    Some(
        text_input(edit, prompt_font())
            .single_line()
            .key("release-version-field")
            .label(format!("{} release version, blank runs the saved draft", card.title))
            .w_full()
            .h(34.0)
            .px(10.0)
            .radius(10.0)
            .fill(t.card)
            .border(1.2, t.rule_2)
            .color(t.ink)
            .on_edit(
                move |app: &mut StudioApp| {
                    if app.is_running() { &mut app.frozen_edit } else { app.versions.entry(id.clone()).or_insert_with(|| TextEdit::new("")) }
                },
            ),
    )
}

pub fn prompt_font() -> TextStyle {
    font(12.0, 450.0).mono()
}

pub fn prompt_editor_height(size: (f32, f32)) -> f32 {
    if size.1 < COMPACT_INSPECTOR_BELOW { COMPACT_PROMPT_EDITOR_HEIGHT } else { PROMPT_EDITOR_HEIGHT }
}

fn provider_name(id: &str) -> String {
    match id {
        "openai" => "OpenAI".to_owned(),
        "anthropic" => "Anthropic".to_owned(),
        "google" | "gemini" => "Google".to_owned(),
        "mistral" => "Mistral".to_owned(),
        "groq" => "Groq".to_owned(),
        "ollama" => "Ollama".to_owned(),
        "openrouter" => "OpenRouter".to_owned(),
        other => other.to_owned(),
    }
}

fn editor_caption(field: &str) -> &'static str {
    match field {
        "script" => "Script",
        "template" => "Template",
        "name" => "Variable",
        "text" => "Text",
        SUBGRAPH_SPELL_FIELD => "Spell ID",
        "cases" => "Cases",
        "outputs" => "Outputs",
        "keys" => "Keys",
        _ => "Prompt",
    }
}

fn playground_panel(app: &StudioApp, light: bool, t: &HearthPalette) -> El {
    let has_conversation = app.chat.iter().any(|message| message.from_user);
    let transcript: Vec<&ChatMessage> = if has_conversation {
        app.chat.iter().filter(|message| message.from_user || message.text != "Send a message to run this spell.").collect()
    } else {
        Vec::new()
    };
    let mut panel = col().key("playground-panel").w_full().grow(1.0).min_h(0.0);
    if has_conversation {
        let turn_count = transcript.iter().filter(|m| m.from_user).count();
        let turn_label = if turn_count == 1 { "1 message".to_owned() } else { format!("{turn_count} messages") };
        let header_caption = format!("Draft test · {turn_label}");
        panel = panel.child(
            row()
                .w_full()
                .px(14.0)
                .py(8.0)
                .items_center()
                .justify_between()
                .border_sides([0.0, 0.0, 1.0, 0.0], t.rule)
                .child(label(header_caption, 11.2, 650.0, t.mute))
                .child(
                    row()
                        .h(24.0)
                        .px(9.0)
                        .radius(7.0)
                        .items_center()
                        .fill(t.card)
                        .border(1.0, t.rule_2)
                        .child(label("Clear", 11.0, 600.0, t.ink_2))
                        .on_press(StudioApp::clear_chat),
                ),
        );
    }
    let body = if transcript.is_empty() {
        let empty = col().w_full().pt(40.0).px(14.0).items_center().gap(10.0).child(canvas(empty_bottle_mark(light, 42.0)).size(42.0, 42.0)).child(heading(
            "No messages yet",
            15.0,
            800.0,
            t.ink,
        ));
        let failed = app.canvas.invalid_brew || app.canvas.last_brew.is_some_and(|status| status != RunStatus::Succeeded);
        match app.canvas.brew_summary.as_ref().filter(|_| failed && !app.is_running()) {
            Some(reason) => empty.child(label(reason, 12.0, 600.0, t.hex).wrap().key("tasting-reason")),
            None => empty,
        }
    } else {
        transcript.iter().enumerate().fold(col().gap(2.0).pt(8.0).pb(12.0), |mut list, (index, message)| {
            if message.from_user && index > 0 {
                let divider_w = app.inspector_width() - 28.0;
                list = list.child(
                    el().key("tasting-divider")
                        .w(divider_w)
                        .pt(8.0)
                        .pb(2.0)
                        .clip_rounded(0.0)
                        .child(canvas(stitched_sparkles_h(divider_w, light)).size(divider_w, 7.0)),
                );
            }
            let turn_row = if message.from_user {
                row()
                    .w_full()
                    .pt(6.0)
                    .pb(4.0)
                    .gap(8.0)
                    .items_start()
                    .child(col().w(16.0).pt(5.0).items_center().child(el().size(9.0, 9.0).radius(4.5).fill(t.gold)))
                    .child(col().grow(1.0).min_w(0.0).child(heading(&message.text, 14.0, 750.0, t.ink).wrap()))
            } else {
                let per_line = if app.compact_toolbar() { 26.0 } else { 38.0 };
                let lines: f32 = message.text.split('\n').map(|line| (line.chars().count() as f32 / per_line).ceil().max(1.0)).sum();
                let bar_h = (lines * 17.0 - 2.0).clamp(22.0, 600.0);
                row()
                    .w_full()
                    .py(4.0)
                    .gap(8.0)
                    .items_start()
                    .child(row().w(16.0).pt(2.0).justify_center().child(canvas(sparkle_bar_v(bar_h, 3.5, light)).size(7.0, bar_h)))
                    .child(col().grow(1.0).min_w(0.0).child(label(&message.text, 13.0, 450.0, t.prose).wrap()))
            };
            list.child(turn_row)
        })
    };
    let can_send = !app.is_running() && app.chat_door().is_some();
    let send_btn = row()
        .key("chat-send")
        .h(34.0)
        .px(13.0)
        .radius(9.0)
        .continuous_corners()
        .items_center()
        .justify_center()
        .fill(if can_send { t.primary_bg } else { t.off_bg })
        .child(label("Send", 12.0, 700.0, if can_send { t.primary_ink } else { t.off_ink }));
    let send_btn = if can_send { send_btn.on_press(StudioApp::send_chat) } else { send_btn };
    let input_el = text_input(&app.chat_input, font(13.0, 450.0))
        .single_line()
        .key("chat-input")
        .label("Message")
        .w_full()
        .h(20.0)
        .fill(t.card)
        .color(t.ink)
        .on_edit(|app: &mut StudioApp| &mut app.chat_input)
        .on_submit(|app: &mut StudioApp, _| app.send_chat());
    let mut input_slot = row().grow(1.0).min_w(0.0).h(32.0).px(8.0).items_center().child(input_el);
    if app.chat_input.text.is_empty() {
        input_slot = input_slot.child(row().h(32.0).px(8.0).items_center().absolute(0.0, 0.0).child(label("Message…", 12.5, 450.0, t.placeholder)));
    }
    let composer_box = row()
        .w_full()
        .h(42.0)
        .padding(4.0)
        .gap(6.0)
        .radius(13.0)
        .continuous_corners()
        .items_center()
        .fill(t.card)
        .border(1.4, t.rule_2)
        .shadow(4.0, 12.0, t.shadow_card)
        .child(input_slot)
        .child(send_btn);
    let composer_dock = row().w_full().px(14.0).py(12.0).fill(t.shelf).border_sides([1.0, 0.0, 0.0, 0.0], t.rule).child(composer_box);
    panel.child(col().key("inspector-scroll").w_full().grow(1.0).min_h(0.0).px(14.0).scroll(ScrollSpec { stick_bottom: true }).child(body)).child(composer_dock)
}

pub fn potion_mood(app: &StudioApp, name: &str) -> magicklala_canvas::CauldronMood {
    use magicklala_canvas::CauldronMood;
    if name != app.spell.name {
        return CauldronMood::Ready;
    }
    if app.is_running() {
        CauldronMood::Simmering
    } else if matches!(app.validation, ValidationState::Invalid(_))
        || app.canvas.invalid_brew
        || app.canvas.last_brew.is_some_and(|status| status != RunStatus::Succeeded)
    {
        CauldronMood::Hexed
    } else if app.canvas.nodes.is_empty() {
        CauldronMood::Empty
    } else if app.canvas.last_brew.is_some() {
        CauldronMood::Brewed
    } else {
        CauldronMood::Ready
    }
}

pub fn studio_status(app: &StudioApp) -> Option<(&str, bool)> {
    match (app.wire_error.as_ref().or(app.store_error.as_ref()), &app.draft_status, &app.publish_status) {
        (Some(error), _, _) => Some((error, true)),
        (None, Some(status), _) | (None, None, Some(status)) => Some((status, false)),
        (None, None, None) => None,
    }
}

fn status_badge(app: &StudioApp, t: &HearthPalette) -> Option<El> {
    let (message, failed) = studio_status(app)?;
    let (bg, border, ink, dot) = if failed {
        (t.hex_tint, t.hex, t.hex, t.hex)
    } else {
        (if t.light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.22) }, t.broth_mid, if t.light { t.broth_deep } else { t.broth_foam }, t.broth_mid)
    };
    Some(
        row()
            .key("studio-status-badge")
            .gap(6.0)
            .px(9.0)
            .py(3.0)
            .radius(10.0)
            .min_w(0.0)
            .items_center()
            .fill(bg)
            .border(1.2, border)
            .child(el().size(6.0, 6.0).radius(3.0).fill(dot))
            .child(label(message, 11.5, 700.0, ink).key("studio-status").truncate()),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum ChipShape {
    Full,
    Dot,
    Span(usize),
    More(usize),
}

fn chip_width(line: &TraceLine, shape: &ChipShape) -> f32 {
    match shape {
        ChipShape::Dot => 22.0 + 10.0,
        ChipShape::Span(_) => 46.0 + 10.0,
        ChipShape::More(_) => 120.0 + 16.0,
        ChipShape::Full => 18.0 + 6.0 + 17.0 + 16.0 + line.detail.chars().count() as f32 * 5.8,
    }
}

fn log_chips<'a>(lines: &[&'a TraceLine], compact: bool, room_for_dots: usize, budget: f32) -> Vec<(&'a TraceLine, ChipShape)> {
    let mut chips = log_chips_by_count(lines, compact, room_for_dots);
    if !compact {
        return chips;
    }
    let spent = |chips: &[(&TraceLine, ChipShape)]| chips.iter().map(|(line, shape)| chip_width(line, shape)).sum::<f32>();
    let last_step = chips.iter().rposition(|(line, _)| line.kind == "NodeFinished");
    let mut cursor = 0;
    while spent(&chips) > budget && cursor < chips.len() {
        if chips[cursor].0.kind == "NodeFinished" && matches!(chips[cursor].1, ChipShape::Full) && Some(cursor) != last_step {
            chips[cursor].1 = ChipShape::Dot;
        }
        cursor += 1;
    }
    if spent(&chips) > budget
        && let Some(last) = last_step.filter(|_| chips.len() > 1)
    {
        chips[last].1 = ChipShape::Dot;
    }
    let issues: Vec<usize> = chips
        .iter()
        .enumerate()
        .filter(|(_, (line, shape))| matches!(line.kind, "NodeFailed" | "RunRejected") && matches!(shape, ChipShape::Full))
        .map(|(index, _)| index)
        .collect();
    if spent(&chips) > budget && issues.len() > 1 {
        let more = issues.len() - 1;
        chips[issues[1]].1 = ChipShape::More(more);
        for &index in issues[2..].iter().rev() {
            chips.remove(index);
        }
    }
    chips
}

fn log_chips_by_count<'a>(lines: &[&'a TraceLine], compact: bool, room_for_dots: usize) -> Vec<(&'a TraceLine, ChipShape)> {
    let steps = lines.iter().filter(|line| line.kind == "NodeFinished").count();
    let last_step = lines.iter().rposition(|line| line.kind == "NodeFinished");
    let issues: Vec<usize> = lines.iter().enumerate().filter(|(_, line)| matches!(line.kind, "NodeFailed" | "RunRejected")).map(|(index, _)| index).collect();
    let mut chips = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let issue_rank = issues.iter().position(|issue| *issue == index);
        match issue_rank {
            Some(1) if compact && issues.len() > 2 => chips.push((*line, ChipShape::More(issues.len() - 1))),
            Some(rank) if compact && issues.len() > 2 && rank > 1 => {}
            _ if compact && steps > 5 && line.kind == "NodeFinished" && Some(index) != last_step => chips.push((*line, ChipShape::Dot)),
            _ => chips.push((*line, ChipShape::Full)),
        }
    }
    let dots = chips.iter().filter(|(_, shape)| matches!(shape, ChipShape::Dot)).count();
    if dots > room_for_dots.max(2) {
        let folded = dots + 1 - room_for_dots.max(2);
        let mut seen = 0;
        chips.retain_mut(|(_, shape)| {
            if !matches!(shape, ChipShape::Dot) {
                return true;
            }
            seen += 1;
            match seen {
                1 => {
                    *shape = ChipShape::Span(folded);
                    true
                }
                n => n > folded,
            }
        });
    }
    chips
}

pub(crate) fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn log_kind_word(line: &TraceLine) -> &str {
    match (line.kind, line.detail.as_str()) {
        ("RunFinished", "Failed") => "Failed",
        ("RunFinished", "Cancelled") => "Cancelled",
        ("RunFinished", "TimedOut") => "Timed out",
        (kind, _) => trace_display_kind(kind),
    }
}

fn trace_display_kind(kind: &str) -> &str {
    match kind {
        "RunStarted" => "Lit",
        "NodeStarted" => "Simmering",
        "NodeFinished" => "Brewed",
        "NodeFailed" => "Failed",
        "WireActivated" => "Pour",
        "StreamChunk" => "Sip",
        "VariableSet" => "Spice",
        "RunFinished" => "Brewed",
        "RunRejected" => "Hexed",
        "RunCancelled" => "Cancelled",
        "RunTimedOut" => "Timed out",
        other => other,
    }
}

fn undo_pill(app: &StudioApp, t: &HearthPalette) -> Option<El> {
    let undo_msg = app.undo_banner.as_ref()?;
    Some(
        row()
            .key("undo-banner")
            .gap(7.0)
            .px(9.0)
            .py(3.0)
            .radius(9.0)
            .items_center()
            .min_w(0.0)
            .fill(t.card)
            .border(1.0, t.rule_2)
            .child(label(undo_msg, 11.2, 600.0, t.ink).truncate())
            .child(
                row()
                    .key("undo-action")
                    .px(7.0)
                    .py(1.5)
                    .radius(6.5)
                    .fill(t.primary_bg)
                    .child(label("Undo", 10.5, 700.0, t.primary_ink))
                    .on_press(StudioApp::undo),
            ),
    )
}

fn trace_strip(app: &StudioApp, t: &HearthPalette) -> El {
    let light = app.theme == Theme::Light;
    let idle = app.trace.is_empty() && !app.is_running();
    let count = app.trace.len();
    let event_word = if count == 1 { "event" } else { "events" };
    if idle {
        let run_hint = if app.canvas.nodes.is_empty() {
            "Not brewed yet".to_owned()
        } else if let Some(v) = app.active_release_version {
            format!("Published v{v} · not brewed yet")
        } else {
            "Not brewed yet".to_owned()
        };
        let mut bar = row()
            .key("trace-strip")
            .w_full()
            .h(IDLE_TRACE_HEIGHT)
            .px(16.0)
            .gap(10.0)
            .items_center()
            .fill(t.shelf)
            .border_sides([1.0, 0.0, 0.0, 0.0], t.rule)
            .child(canvas(brand_cauldron_mark(light)).key("trace-bobbin").size(34.0, 28.0))
            .child(heading("Cauldron Log", 13.0, 800.0, t.ink))
            .child(label(run_hint, 12.0, 450.0, t.mute));
        if let Some(pill) = undo_pill(app, t) {
            bar = bar.child(pill);
        }
        if let Some(badge) = status_badge(app, t) {
            bar = bar.child(el().grow(1.0).min_w(0.0)).child(badge);
        }
        return bar;
    }
    let finished_steps = crate::top_step_count(&app.trace);
    let blocked_pre_run = !app.is_running() && finished_steps == 0 && app.trace.iter().all(|line| line.kind == "RunRejected");
    let show_expand_chip = !app.is_running() && finished_steps > 0 && count > 1;
    let step_word = if finished_steps == 1 { "step" } else { "steps" };
    let issue_word = if count == 1 { "issue" } else { "issues" };
    let summary = if app.is_running() {
        format!("Brew #{} · simmering · {} {} · Quench stops it", app.runs.max(1), count, event_word)
    } else if blocked_pre_run {
        format!("Check · {count} {issue_word}")
    } else if app.trace_stale {
        format!("Last brew #{}", app.runs.max(1))
    } else if show_expand_chip {
        format!("Brew #{}", app.runs.max(1))
    } else if finished_steps > 0 {
        format!("Brew #{} · {finished_steps} {step_word}", app.runs.max(1))
    } else {
        format!("Brew #{} · {count} {event_word}", app.runs.max(1))
    };
    let milestones: Vec<&TraceLine> = if count > 6 && !app.trace_expanded {
        let filtered: Vec<&TraceLine> = app
            .trace
            .iter()
            .filter(|line| matches!(line.kind, "NodeFinished" | "NodeFailed" | "RunFinished" | "RunRejected" | "RunCancelled" | "RunTimedOut"))
            .collect();
        if filtered.is_empty() { app.trace.iter().rev().take(6).rev().collect() } else { filtered }
    } else {
        app.trace.iter().collect()
    };
    let mut header = row()
        .w_full()
        .gap(9.0)
        .items_center()
        .child(canvas(brand_cauldron_mark(light)).key("trace-bobbin").size(34.0, 28.0))
        .child(heading("Cauldron Log", 13.0, 800.0, t.ink))
        .child(label(summary, 12.0, 500.0, t.mute));
    if show_expand_chip {
        let toggle_label = if app.trace_expanded { "Key steps".to_owned() } else { format!("{finished_steps} {step_word} · {count} {event_word}") };
        let chevron = canvas(chevron_down_shape(t.ink_2)).size(9.0, 7.0).rotate(if app.trace_expanded { std::f32::consts::PI } else { 0.0 });
        header = header.child(
            row()
                .key("trace-expand")
                .px(8.0)
                .py(2.0)
                .radius(8.0)
                .fill(t.card)
                .border(1.0, t.rule_2)
                .items_center()
                .gap(5.0)
                .child(label(toggle_label, 11.0, 650.0, t.ink))
                .child(chevron)
                .on_press(|app: &mut StudioApp| app.trace_expanded = !app.trace_expanded),
        );
    }
    if let Some(pill) = undo_pill(app, t).filter(|_| !app.is_running()) {
        header = header.child(pill);
    }
    if let Some(badge) = status_badge(app, t) {
        header = header.child(el().grow(1.0).min_w(0.0)).child(badge);
    }
    let run_done = !app.is_running();
    let pinned = (!app.trace_expanded && run_done && !blocked_pre_run).then(|| log_result(app, t)).flatten();
    let pinned_room = pinned.as_ref().map_or(0.0, |(_, width)| *width + 16.0);
    let budget = (app.size.0 - 32.0 - pinned_room).max(120.0);
    let numbers = crate::numbered_steps(&app.trace);
    let number_of = |line: &TraceLine| numbers.iter().find(|(_, known)| std::ptr::eq(*known, line)).map(|(number, _)| number.clone());
    let shown: Vec<&TraceLine> = if pinned.is_some() {
        let tops: Vec<&(String, &TraceLine)> = numbers.iter().filter(|(number, _)| !number.contains('.')).collect();
        let mut seen: Vec<&str> = Vec::new();
        tops.iter()
            .filter(|(number, _)| {
                let fresh = !seen.contains(&number.as_str());
                seen.push(number.as_str());
                fresh
            })
            .map(|(number, first)| tops.iter().find(|(same, line)| same == number && line.kind == "NodeFailed").map_or(*first, |(_, failed)| *failed))
            .collect()
    } else {
        milestones.clone()
    };
    let chips = log_chips(&shown, !app.trace_expanded, ((app.size.0 - 420.0) / 32.0).max(0.0) as usize, budget);
    let total = chips.len();
    let chips_spent: f32 = chips.iter().map(|(line, shape)| chip_width(line, shape)).sum();
    let collapsed = !app.trace_expanded;
    let mut step_num = 0usize;
    let body = chips.iter().enumerate().fold(row().gap(0.0).items_center(), |list, (index, (line, shape))| {
        let line = *line;
        let link = |chip: El| {
            if index + 1 < total {
                row().items_center().child(chip).child(canvas(sparkle_connector_h(10.0, 6.0, light)).size(10.0, 6.0))
            } else {
                row().items_center().child(chip)
            }
        };
        match shape {
            ChipShape::Dot => {
                step_num += 1;
                let number = number_of(line).unwrap_or_else(|| step_num.to_string());
                let mut dot = row()
                    .key(format!("log-step-{step_num}"))
                    .label(line.detail.clone())
                    .min_w(22.0)
                    .h(22.0)
                    .px(if number.len() > 1 { 4.0 } else { 0.0 })
                    .radius(7.5)
                    .continuous_corners()
                    .items_center()
                    .justify_center()
                    .fill(if line.status == ExecutionStatus::Failed { t.hex } else { t.primary_bg })
                    .child(figure(number, 9.5, 750.0, t.primary_ink));
                if let Some(target_node) = line.canvas_node() {
                    dot = dot.on_press(move |app: &mut StudioApp| app.canvas.select(Some(target_node.clone())));
                }
                return list.child(link(dot));
            }
            ChipShape::Span(count) => {
                let at = shown.iter().position(|known| std::ptr::eq(*known, line)).unwrap_or(0);
                let first = number_of(line).unwrap_or_else(|| (step_num + 1).to_string());
                step_num += count;
                let step_num = shown.get(at + count - 1).and_then(|last| number_of(last)).unwrap_or_else(|| step_num.to_string());
                let span = row()
                    .key("log-step-span")
                    .label(format!("Steps {first} to {step_num}"))
                    .h(22.0)
                    .px(7.0)
                    .radius(7.5)
                    .continuous_corners()
                    .items_center()
                    .justify_center()
                    .fill(t.primary_bg)
                    .child(figure(format!("{first}–{step_num}"), 9.5, 750.0, t.primary_ink))
                    .on_press(|app: &mut StudioApp| app.right_tab = RightTab::Trace);
                return list.child(link(span));
            }
            ChipShape::More(more) => {
                let chip = row()
                    .key("log-more-issues")
                    .h(29.0)
                    .px(9.0)
                    .radius(10.0)
                    .continuous_corners()
                    .items_center()
                    .fill(t.hex_tint)
                    .border(1.0, t.hex)
                    .child(label(plural(*more, "more issue", "more issues"), 11.5, 700.0, t.hex))
                    .on_press(|app: &mut StudioApp| app.right_tab = RightTab::Trace);
                return list.child(link(chip));
            }
            ChipShape::Full => {}
        }
        let dot_color = match line.status {
            ExecutionStatus::Failed => t.hex,
            ExecutionStatus::Running if !run_done => t.gold,
            ExecutionStatus::Running | ExecutionStatus::Succeeded => t.broth_mid,
            ExecutionStatus::Idle => t.mute,
        };
        let room = budget - (chips_spent - chip_width(line, shape)) - 16.0;
        let mut entry = row()
            .gap(6.0)
            .h(29.0)
            .min_w(0.0)
            .max_w(if collapsed { room.max(96.0) } else { f32::INFINITY })
            .px(9.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .fill(if line.status == ExecutionStatus::Failed { t.hex_tint } else { t.card })
            .border(1.0, if line.status == ExecutionStatus::Failed { t.hex } else { t.rule_2 });
        if let Some(number) = number_of(line).filter(|_| matches!(line.kind, "NodeFinished" | "NodeFailed")) {
            step_num += 1;
            let failed = line.kind == "NodeFailed";
            entry = entry
                .child(
                    row()
                        .min_w(17.0)
                        .h(17.0)
                        .px(if number.len() > 1 { 4.0 } else { 0.0 })
                        .radius(6.0)
                        .fill(if failed { t.hex } else { t.primary_bg })
                        .items_center()
                        .justify_center()
                        .child(figure(number, 9.5, 750.0, t.primary_ink)),
                )
                .child(label(&line.detail, 11.5, 600.0, if failed { t.hex } else { t.ink }).truncate().shrink());
        } else if line.kind == "RunFinished" {
            let outcome = if line.detail == "Succeeded" || line.detail.is_empty() {
                if app.active_release_version.is_some() { "Draft brewed" } else { "Brewed" }
            } else {
                line.detail.as_str()
            };
            entry = entry
                .child(el().size(7.0, 7.0).radius(3.5).fill(dot_color))
                .child(label(
                    outcome,
                    11.5,
                    750.0,
                    if line.status == ExecutionStatus::Failed {
                        t.hex
                    } else if light {
                        t.broth_deep
                    } else {
                        t.broth_foam
                    },
                ))
                .child(
                    text(if line.at_ms == 0 { "<1ms".to_owned() } else { format!("{}ms", line.at_ms) })
                        .font(font(10.5, 550.0).mono().features("'tnum' 1"))
                        .color(if light { t.ink_2 } else { hex(0xD4CEE6) }),
                );
        } else {
            entry = entry
                .child(el().size(7.0, 7.0).radius(3.5).fill(dot_color))
                .child(label(trace_display_kind(line.kind), 11.5, 700.0, if line.status == ExecutionStatus::Failed { t.hex } else { t.ink }))
                .child(label(&line.detail, 11.5, 450.0, t.ink_2));
            if line.status == ExecutionStatus::Failed && line.node.is_some() {
                entry = entry.child(label("→", 11.5, 700.0, t.hex));
            }
        }
        if let Some(target_node) = line.canvas_node() {
            entry = entry.on_press(move |app: &mut StudioApp| {
                app.canvas.select(Some(target_node.clone()));
            });
        }
        let unit = if index + 1 < total {
            row().items_center().child(entry).child(canvas(sparkle_connector_h(16.0, 6.0, light)).size(16.0, 6.0))
        } else {
            row().items_center().child(entry)
        };
        list.child(unit)
    });
    let lane = if app.trace_expanded {
        col().key("trace-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: true }).child(log_list(app, t))
    } else {
        let steps = row().key("trace-scroll").grow(1.0).min_w(0.0).h_full().items_center().scroll_x().child(body);
        let mut lane = row().w_full().grow(1.0).min_h(0.0).gap(16.0).items_center().child(steps);
        if let Some((result, _)) = pinned {
            lane = lane.child(result);
        }
        lane
    };
    col()
        .key("trace-strip")
        .w_full()
        .h(app.trace_height())
        .min_h(0.0)
        .px(16.0)
        .pt(13.0)
        .pb(10.0)
        .gap(9.0)
        .fill(t.shelf)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.rule)
        .child(header)
        .child(lane)
}

fn log_result(app: &StudioApp, t: &HearthPalette) -> Option<(El, f32)> {
    if app.trace_stale {
        let checked = matches!(app.validation, ValidationState::Valid);
        let ink = if t.light { t.broth_deep } else { t.broth_foam };
        let chip = row()
            .key("log-result")
            .h(29.0)
            .px(9.0)
            .gap(6.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .fill(t.card)
            .border(1.0, t.rule_2)
            .child(el().size(7.0, 7.0).radius(3.5).fill(if checked { t.broth_mid } else { t.mute }))
            .child(label(if checked { "Checked" } else { "Edited" }, 11.5, 750.0, if checked { ink } else { t.ink_2 }));
        return Some((chip, 90.0));
    }
    let last = app.trace.iter().rev().find(|line| matches!(line.kind, "RunFinished" | "RunCancelled" | "RunTimedOut"))?;
    let failed: Vec<&TraceLine> = app.trace.iter().filter(|line| line.kind == "NodeFailed").collect();
    let light = t.light;
    let time = if last.at_ms == 0 { "<1ms".to_owned() } else { format!("{}ms", last.at_ms) };
    let first_failed = failed.first().and_then(|line| line.canvas_node());
    let (word, name) = match failed.first() {
        Some(line) => (
            "Failed".to_owned(),
            line.node.as_ref().map_or_else(|| line.detail.split(':').next().unwrap_or_default().to_owned(), |id| app.canvas.display_title(id)),
        ),
        None if last.kind == "RunFinished" && (last.detail == "Succeeded" || last.detail.is_empty()) => {
            ((if app.active_release_version.is_some() { "Draft brewed" } else { "Brewed" }).to_owned(), String::new())
        }
        None => (trace_display_kind(last.kind).to_owned(), if last.kind == "RunFinished" { last.detail.clone() } else { String::new() }),
    };
    let more = failed.len().saturating_sub(1);
    let sour = last.status == ExecutionStatus::Failed || !failed.is_empty();
    let ink = if sour {
        t.hex
    } else if light {
        t.broth_deep
    } else {
        t.broth_foam
    };
    let name = if more > 0 { format!("{name} +{more}") } else { name };
    let width = 9.0 * 2.0 + 7.0 + 6.0 * 3.0 + (word.chars().count() as f32 * 7.4) + (name.chars().count() as f32 * 6.6) + 30.0;
    let mut chip = row()
        .key("log-result")
        .h(29.0)
        .px(9.0)
        .gap(6.0)
        .radius(10.0)
        .continuous_corners()
        .items_center()
        .fill(if sour { t.hex_tint } else { t.card })
        .border(1.0, if sour { t.hex } else { t.rule_2 })
        .child(el().size(7.0, 7.0).radius(3.5).fill(if sour { t.hex } else { t.broth_mid }))
        .child(label(word, 11.5, 750.0, ink));
    if !name.is_empty() {
        chip = chip.child(label(name, 11.5, 600.0, t.ink));
    }
    chip = chip.child(text(time).font(font(10.5, 550.0).mono().features("'tnum' 1")).color(if light { t.ink_2 } else { hex(0xD4CEE6) }));
    if let Some(node) = first_failed {
        chip = chip.on_press(move |app: &mut StudioApp| {
            app.canvas.select(Some(node.clone()));
            app.right_tab = RightTab::Inspector;
        });
    }
    Some((chip, width))
}

fn log_list(app: &StudioApp, t: &HearthPalette) -> El {
    app.trace.iter().enumerate().fold(col().w_full().gap(3.0), |list, (index, line)| {
        let sour = line.status == ExecutionStatus::Failed;
        let dot = match line.status {
            ExecutionStatus::Failed => t.hex,
            ExecutionStatus::Running if app.is_running() => t.gold,
            ExecutionStatus::Running | ExecutionStatus::Succeeded => t.broth_mid,
            ExecutionStatus::Idle => t.mute,
        };
        let mut item = row()
            .key(format!("log-line-{index}"))
            .w_full()
            .min_h(22.0)
            .px(8.0)
            .py(2.0)
            .gap(8.0)
            .radius(7.0)
            .items_center()
            .fill(if sour { t.hex_tint } else { Color::TRANSPARENT })
            .child(
                text(if line.at_ms == 0 { "<1ms".to_owned() } else { format!("{}ms", line.at_ms) })
                    .font(font(10.5, 550.0).mono().features("'tnum' 1"))
                    .color(t.mute)
                    .w(44.0),
            )
            .child(el().size(7.0, 7.0).radius(3.5).fill(dot))
            .child(label(log_kind_word(line), 11.5, 700.0, if sour { t.hex } else { t.ink }).w(78.0))
            .child(label(if line.kind == "RunFinished" { "" } else { line.detail.as_str() }, 11.5, 450.0, t.ink_2).wrap().grow(1.0).min_w(0.0));
        if let Some(node) = line.canvas_node() {
            item = item.on_press(move |app: &mut StudioApp| app.canvas.select(Some(node.clone())));
        }
        list.child(item)
    })
}
