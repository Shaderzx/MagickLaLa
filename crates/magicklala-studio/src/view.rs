use magicklala_canvas::hearth::{
    HearthPalette, brand_cauldron_mark, category_tone, chevron_down_shape, empty_bottle_mark, hex, mini_vial_icon, search_icon_shape, sparkle_bar_v,
    sparkle_connector_h, stitched_sparkles_h, theme_icon_shape, with_alpha,
};
use magicklala_canvas::{ExecutionStatus, WorkbenchMode, interactive_canvas_view};
use magicklala_graph::{NodeSpec, RunStatus, SUBGRAPH_KIND, SocketType};
use rune::{Color, Cx, El, ScrollSpec, TextEdit, TextStyle, canvas, col, el, row, text, text_input};

use crate::{
    COMPACT_INSPECTOR_BELOW, COMPACT_PROMPT_EDITOR_HEIGHT, COMPACT_RUN_SLOT_WIDTH, CONTEXT_MENU_SIZE, ChatMessage, IDLE_TRACE_HEIGHT, LeftTab,
    PROMPT_EDITOR_HEIGHT, PROMPT_EDITOR_PADDING, QUICK_ADD_KIND, RUN_SLOT_WIDTH, RightTab, SUBGRAPH_SPELL_FIELD, StudioApp, TOOLBAR_HEIGHT, TRACE_HEIGHT,
    Theme, TraceLine, ValidationState, editable_field, palette_categories,
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
                (t.off_bg, with_alpha(t.off_ink, 0.50), with_alpha(t.rule, 0.55), 1.0)
            } else {
                (with_alpha(t.wash_2, 0.72), with_alpha(t.off_ink, 0.58), with_alpha(t.rule_2, 0.45), 1.0)
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
    if spec.enabled { item.on_press(action) } else { item }
}

fn context_menu(app: &StudioApp, t: &HearthPalette) -> Option<El> {
    let anchor = app.canvas.context_menu?;
    let view = app.canvas_size();
    let x = anchor[0].min(view.0 - CONTEXT_MENU_SIZE.0 - 12.0).max(12.0);
    let y = anchor[1].min(view.1 - CONTEXT_MENU_SIZE.1 - 12.0).max(12.0);
    let selected_card = app.canvas.selected_card();
    let selected_title = selected_card.map(|card| card.title.as_str());
    let quick_add_title = app.specs.iter().find(|spec| spec.kind == QUICK_ADD_KIND).map_or(QUICK_ADD_KIND, |spec| spec.title.as_str());
    let quick_add = format!("Add {quick_add_title}");
    let title_label = selected_title.unwrap_or("Cauldron");
    let delete = "Remove";
    let header = row().w_full().h(24.0).px(8.0).items_center().justify_between().child(label(title_label, 10.5, 700.0, t.mute)).child(
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
        .absolute(x, y)
        .child(header)
        .child(el().w_full().h(1.0).fill(t.rule));
    card = match selected_title {
        Some(title) => card
            .child(menu_item(
                MenuItemSpec {
                    key: "context-duplicate-node",
                    caption: &format!("Duplicate {title}"),
                    shortcut: "",
                    enabled: true,
                    primary: true,
                    danger: false,
                },
                t,
                StudioApp::duplicate_selected_node,
            ))
            .child(menu_item(
                MenuItemSpec { key: "context-open-grimoire", caption: "Inspect in Grimoire", shortcut: "", enabled: true, primary: false, danger: false },
                t,
                StudioApp::open_grimoire,
            ))
            .child(el().w_full().h(1.0).fill(t.rule))
            .child(menu_item(
                MenuItemSpec { key: "context-delete-node", caption: delete, shortcut: "", enabled: true, primary: false, danger: true },
                t,
                StudioApp::delete_selected_node,
            )),
        None => card
            .child(menu_item(
                MenuItemSpec {
                    key: &palette_key(QUICK_ADD_KIND).replacen("palette-", "context-add-", 1),
                    caption: &quick_add,
                    shortcut: "",
                    enabled: true,
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
    Some(card)
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

pub fn studio_view(app: &StudioApp, _: &mut Cx<StudioApp>) -> El {
    let t = tones(app.theme);
    let middle = row()
        .key("workspace")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(left_rail(app, &t))
        .child(el().key("center").grow(1.0).min_w(0.0).h_full().child(canvas_with_menu(app, &t)))
        .child(inspector(app, &t));
    let studio = col().key("studio").w_full().h_full().fill(t.paper).child(toolbar(app, &t)).child(middle).child(trace_strip(app, &t));
    let root = el().key("studio-root").w_full().h_full().child(studio);
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
    let hint = match (app.canvas.mode, over, app.pantry_drag_point().is_some()) {
        (WorkbenchMode::Cauldron, true, _) => "Drop to stir in",
        (WorkbenchMode::Cauldron, false, true) => "Drop to set aside",
        (WorkbenchMode::Grimoire, _, true) => "Drop to place",
        _ => "Drag onto the cauldron",
    };
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
            .child(col().gap(1.0).child(label(&spec.title, 12.0, 700.0, t.ink)).child(label(hint, 10.0, 550.0, if over { t.simmer_ink() } else { t.mute }))),
    )
}

fn empty_canvas_overlay(app: &StudioApp, t: &HearthPalette) -> El {
    let size = app.canvas_size();
    let card_w = 400.0_f32.min(size.0 - 32.0).max(240.0);
    let x = ((size.0 - card_w) * 0.5).max(16.0);
    let y = (size.1 * 0.62).min(size.1 - 152.0).max(216.0);
    col()
        .key("empty-canvas")
        .w(card_w)
        .px(16.0)
        .py(12.0)
        .items_center()
        .gap(8.0)
        .absolute(x, y)
        .child(heading("Start a new potion", 24.0, 800.0, t.ink))
        .child(col().items_center().gap(2.0).child(label("Add an ingredient from the Pantry,", 13.0, 450.0, t.mute)).child(label(
            "or start with On Message.",
            13.0,
            450.0,
            t.mute,
        )))
        .child(row().pt(4.0).child(sized_button("empty-add-trigger", "Add On Message", true, false, 16.0, t, |app| {
            app.spawn("event/on_message");
        })))
}

fn canvas_with_menu(app: &StudioApp, t: &HearthPalette) -> El {
    let mut board = interactive_canvas_view::<StudioApp>(&app.canvas, app.canvas_size(), |app| &mut app.canvas, StudioApp::toss);
    if app.canvas.nodes.is_empty() {
        board = board.child(empty_canvas_overlay(app, t));
    }
    match context_menu(app, t) {
        Some(menu) => board.child(menu),
        None => board,
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
            app.canvas.zoom_active(1.0 / 1.2, [app.canvas_size().0 / 2.0, app.canvas_size().1 / 2.0]);
        }))
        .child(row().key("zoom-readout").w(readout).justify_center().child(figure(
            format!("{}%", app.canvas.active_camera().zoom_percent()),
            11.5,
            600.0,
            t.ink_2,
        )))
        .child(button_el("zoom-in", "+", ButtonKind::Subtle, if compact { 6.0 } else { 8.0 }, t, |app| {
            app.canvas.zoom_active(1.2, [app.canvas_size().0 / 2.0, app.canvas_size().1 / 2.0]);
        }))
        .child(button_el("zoom-fit", "Fit", ButtonKind::Subtle, if compact { 6.0 } else { 9.0 }, t, StudioApp::fit_view));
    let bottle_count = app.canvas.nodes.iter().filter(|card| card.kind == "io/send_message" || card.category.eq_ignore_ascii_case("io")).count();
    let ingredient_count = app.canvas.nodes.len().saturating_sub(bottle_count);
    let i_word = if ingredient_count == 1 { "ingredient" } else { "ingredients" };
    let o_word = if bottle_count == 1 { "output" } else { "outputs" };
    let stats = if app.canvas.mode == WorkbenchMode::Grimoire {
        format!("{ingredient_count} {i_word} · {bottle_count} {o_word} · {} wires", app.canvas.wires.len())
    } else {
        format!("{ingredient_count} {i_word} · {bottle_count} {o_word}")
    };
    let mut left = row().key("toolbar-left").gap(if compact { 7.0 } else { 11.0 }).items_center().child(brand).child(divider()).child(heading(
        &app.spell.name,
        14.5,
        750.0,
        t.ink,
    ));
    if !empty_graph {
        let stats_box =
            row().h(24.0).px(8.5).radius(12.0).continuous_corners().items_center().fill(t.wash_2).border(1.0, t.rule).child(label(stats, 11.0, 550.0, t.ink_2));
        left = left.child(stats_box);
    }
    let hexed = matches!(app.validation, ValidationState::Invalid(_));
    let published_unchanged = app.is_unchanged_from_live_release();
    let check_kind = if empty_graph { ButtonKind::Disabled } else { ButtonKind::Secondary };
    let brew_kind = if empty_graph || hexed { ButtonKind::Disabled } else { ButtonKind::Primary };
    let bottle_kind = if empty_graph || hexed || published_unchanged { ButtonKind::Disabled } else { ButtonKind::Secondary };
    let save_kind = if empty_graph { ButtonKind::Disabled } else { ButtonKind::Secondary };
    let theme_caption = if app.theme == Theme::Dark { "Sunlit" } else { "Midnight" };
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
            .child(canvas(theme_icon_shape(app.theme == Theme::Dark, light)).size(14.0, 14.0))
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
            .child(canvas(theme_icon_shape(app.theme == Theme::Dark, light)).size(14.0, 14.0))
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
        .child(left)
        .child(right)
}

fn tab_sized(key: &str, caption: &str, active: bool, compact: bool, light: bool, t: &HearthPalette, action: fn(&mut StudioApp)) -> El {
    let underline_w = if compact { 44.0 } else { 58.0 };
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

fn tab(key: &str, caption: &str, active: bool, light: bool, t: &HearthPalette, action: fn(&mut StudioApp)) -> El {
    tab_sized(key, caption, active, false, light, t, action)
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
    let tabs = row()
        .w_full()
        .px(12.0)
        .gap(8.0)
        .fill(t.shelf)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.rule)
        .child(tab("tab-palette", "Pantry", app.left_tab == LeftTab::Palette, light, t, |app| app.left_tab = LeftTab::Palette))
        .child(tab("tab-agents", "Potions", app.left_tab == LeftTab::Agents, light, t, |app| {
            app.left_tab = LeftTab::Agents;
        }));
    let query = app.palette_query.text.trim().to_ascii_lowercase();
    let mut rail = col().key("left-rail").w(rail_w).h_full().min_h(0.0).fill(t.shelf).border_sides([0.0, 1.0, 0.0, 0.0], t.rule).child(tabs);
    match app.left_tab {
        LeftTab::Palette => {
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
                        .px(6.0)
                        .h(20.0)
                        .radius(6.0)
                        .items_center()
                        .justify_center()
                        .fill(t.wash_2)
                        .border(1.0, t.rule_2)
                        .child(label("Clear", 10.0, 650.0, if light { t.broth_deep } else { t.broth_foam }))
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
                    .child(label("Try another herb, flow, or spirit name.", 11.5, 450.0, t.mute).wrap())
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
                        let in_pot = app.canvas.nodes.iter().any(|card| card.kind == spec.kind);
                        let mut right_actions = row().gap(4.0).items_center();
                        if in_pot {
                            right_actions = right_actions.child(
                                row().px(5.0).py(1.0).radius(6.0).fill(if light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.24) }).child(label(
                                    "✓",
                                    10.0,
                                    750.0,
                                    if light { t.broth_deep } else { t.broth_foam },
                                )),
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
                            let end = pos + query.len();
                            let pre = &spec.title[..pos];
                            let hit = &spec.title[pos..end];
                            let post = &spec.title[end..];
                            let mut pieces = row().grow(1.0).min_w(0.0).items_center();
                            if !pre.is_empty() {
                                pieces = pieces.child(label(pre, 12.0, 500.0, t.ink));
                            }
                            pieces = pieces.child(label(hit, 12.0, 800.0, if light { t.broth_deep } else { t.broth_foam }));
                            if !post.is_empty() {
                                pieces = pieces.child(label(post, 12.0, 500.0, t.ink).truncate());
                            }
                            pieces
                        } else {
                            label(&spec.title, 12.0, 550.0, t.ink).truncate()
                        };
                        let item = row()
                            .key(palette_key(&spec.kind))
                            .w_full()
                            .h(29.0)
                            .px(7.0)
                            .gap(7.0)
                            .radius(9.0)
                            .continuous_corners()
                            .items_center()
                            .justify_between()
                            .fill(t.card)
                            .border(1.0, if in_pot { with_alpha(t.broth_mid, 0.55) } else { t.rule })
                            .child(
                                row()
                                    .grow(1.0)
                                    .min_w(0.0)
                                    .gap(7.0)
                                    .items_center()
                                    .child(canvas(mini_vial_icon(&spec.category, light, 18.0)).size(18.0, 18.0))
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
            let in_pot_count = app.canvas.nodes.iter().filter(|card| card.kind != "io/send_message").count();
            let footer_text = if !query.is_empty() {
                let m_word = if filtered.len() == 1 { "match" } else { "matches" };
                format!("{} {m_word} · {} in cauldron", filtered.len(), in_pot_count)
            } else if app.canvas.nodes.is_empty() {
                format!("{} ingredients · click + to add", app.specs.len())
            } else {
                format!("{} ingredients · {} in cauldron", app.specs.len(), in_pot_count)
            };
            let fade_overlay = el().w_full().h(30.0).absolute(0.0, -30.0).gradient(with_alpha(t.shelf, 0.0), t.shelf);
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
            let footer_wrap = if query.is_empty() { el().w_full().child(fade_overlay).child(footer) } else { footer };
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
    let ingr_count = app.canvas.nodes.iter().filter(|card| card.kind != "io/send_message").count();
    let ingr_word = if ingr_count == 1 { "ingredient" } else { "ingredients" };
    let spells_header = row()
        .w_full()
        .pt(14.0)
        .pb(6.0)
        .items_center()
        .justify_between()
        .child(heading("Spells & potions", 14.5, 800.0, t.ink))
        .child(button_el("new-spell", "+ New", ButtonKind::Secondary, 9.0, t, StudioApp::request_new_spell));
    let mut agent_names: Vec<String> = Vec::new();
    if app.spell.name != "Support Agent" && app.spell.name != "Research Agent" {
        agent_names.push(app.spell.name.clone());
    }
    agent_names.push("Support Agent".to_owned());
    agent_names.push("Research Agent".to_owned());
    let agent_rows = agent_names.iter().fold(col().gap(6.0), |list, name| {
        let active = name == &app.spell.name;
        let published = app.active_release_version;
        let draft_tested = active && app.canvas.has_run() && !spell_invalid && published.is_some();
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
                format!("{ingr_count} {ingr_word} · v{} live", published.unwrap_or(1)),
                "Live",
                if light { hex(0xE6F6EF) } else { with_alpha(t.broth_mid, 0.24) },
                t.broth_mid,
                if light { t.broth_deep } else { t.broth_foam },
                false,
            )
        } else if active {
            (
                format!("{ingr_count} {ingr_word}"),
                "Draft",
                with_alpha(t.gold, if light { 0.14 } else { 0.22 }),
                t.gold,
                if light { t.ink } else { t.gold_hi },
                false,
            )
        } else {
            let sample_count = if name == "Research Agent" { "3 ingredients" } else { "4 ingredients" };
            (sample_count.to_owned(), "Open ›", t.card, t.rule_2, t.ink, true)
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
                    .child(canvas(mini_vial_icon(if active { "memory" } else { "knowledge" }, light, 22.0)).size(22.0, 22.0))
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
        let mut empty_card = col()
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
                if spell_invalid { ButtonKind::Disabled } else { ButtonKind::Secondary },
                14.0,
                t,
                |app| {
                    app.publish().ok();
                },
            )));
        if spell_invalid {
            empty_card = empty_card.child(label("Fix the hex to publish", 10.5, 600.0, t.hex));
        }
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
        let parts = release.nodes.saturating_sub(1).max(1);
        let p_word = if parts == 1 { "ingredient" } else { "ingredients" };
        let sub_meta = app.release_notes.get(&release.version).cloned().unwrap_or_else(|| format!("{parts} {p_word}"));
        let stamp = el().size(28.0, 24.0).radius(8.0).continuous_corners().fill(if is_active { t.primary_bg } else { t.wash }).center().child(figure(
            format!("v{}", release.version),
            10.5,
            750.0,
            if is_active { t.primary_ink } else { t.ink },
        ));
        let version = release.version;
        let right_action = if is_active {
            row().px(7.0).py(2.5).radius(8.0).fill(t.primary_bg).child(label("Live", 9.8, 750.0, t.primary_ink))
        } else {
            row().px(6.5).py(2.5).radius(8.0).fill(Color::TRANSPARENT).border(1.0, t.rule_2).child(label("Restore", 9.8, 650.0, t.ink_2)).on_press(
                move |app: &mut StudioApp| {
                    app.request_restore_release(version);
                },
            )
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
                        .child(label(if is_active { "Published brew" } else { "Archived brew" }, 11.2, 650.0, t.ink).truncate())
                        .child(label(sub_meta, 9.8, 500.0, t.mute).truncate()),
                ),
            )
            .child(right_action);
        let mut entry = col()
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

fn inspector(app: &StudioApp, t: &HearthPalette) -> El {
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
        let remove_name = variable.name.clone();
        let toggle_name = variable.name.clone();
        let menu_open = app.open_var_type.as_deref() == Some(variable.name.as_str());
        let pending_reset = app.pending_type_reset.as_ref().filter(|(n, _)| n == &variable.name).map(|(_, ty)| *ty);
        let (bound_hint, bound_active) = app.variable_binding_hint(&variable.name);
        let name_widget = if let Some(name_edit) = app.var_name_edits.get(idx) {
            text_input(name_edit, font(12.0, 700.0).mono())
                .single_line()
                .key(format!("var-name-{idx}"))
                .label(format!("Spice {} name", variable.name))
                .w_full()
                .h(18.0)
                .color(t.ink)
                .on_edit(move |app: &mut StudioApp| {
                    while app.var_name_edits.len() <= idx {
                        app.var_name_edits.push(TextEdit::new(""));
                    }
                    &mut app.var_name_edits[idx]
                })
        } else {
            text(&variable.name).font(font(12.0, 700.0).mono()).color(t.ink).truncate()
        };
        let name_col = col().grow(1.0).min_w(0.0).gap(1.5).child(name_widget).child(label(
            bound_hint,
            11.0,
            600.0,
            if bound_active { if t.light { t.broth_deep } else { t.broth_foam } } else { t.mute },
        ));
        let row_top = row().w_full().gap(6.0).items_center().justify_between().child(name_col).child(
            row()
                .gap(6.0)
                .items_center()
                .child(
                    row()
                        .key(format!("var-type-{}", variable.name))
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
                            app.toggle_var_type_menu(&toggle_name);
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
                            app.remove_variable(&remove_name);
                        }),
                ),
        );
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
                        while app.var_default_edits.len() <= idx {
                            app.var_default_edits.push(TextEdit::new(""));
                        }
                        &mut app.var_default_edits[idx]
                    }),
            );
            if def_edit.text.is_empty() {
                field = field.child(row().h(18.0).items_center().absolute(0.0, 0.0).child(label("Enter value…", 11.0, 450.0, t.placeholder)));
            }
            field
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
            let var_name = variable.name.clone();
            let current_ty = variable.ty;
            let choices = [SocketType::String, SocketType::Int, SocketType::Float, SocketType::Bool, SocketType::Any];
            let dropdown = choices.into_iter().fold(
                col()
                    .key(format!("var-type-menu-{}", variable.name))
                    .w_full()
                    .padding(4.0)
                    .gap(2.0)
                    .radius(9.0)
                    .continuous_corners()
                    .fill(t.paper)
                    .border(1.2, t.broth_mid),
                |menu, choice| {
                    let selected = choice == current_ty;
                    let pick_name = var_name.clone();
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
                            .child(label(if selected { "✓" } else { "" }, 11.0, 750.0, if t.light { t.broth_deep } else { t.broth_foam }))
                            .on_press(move |app: &mut StudioApp| app.set_variable_type(&pick_name, choice)),
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
        found.push(format!("{} fizzled during brewing.", card.title));
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
    let status_label = if succeeded { "Brewed" } else { "Hexed" };
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
                        .child(heading(format!("Brew #{}", app.runs), 15.5, 800.0, t.ink))
                        .child(label("·", 14.0, 600.0, t.mute))
                        .child(heading(status_label, 15.5, 800.0, if succeeded { if t.light { t.broth_deep } else { t.broth_foam } } else { t.hex })),
                )
                .child(label(format!("{} steps · {} events · {total_label}", finished.len(), app.trace.len()), 11.0, 500.0, t.mute)),
        );
    let steps = finished.iter().enumerate().fold(col().gap(5.0), |list, (index, line)| {
        let target = line.node.clone();
        let mut item = row()
            .w_full()
            .h(34.0)
            .px(9.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .justify_between()
            .fill(t.card)
            .border(1.0, t.rule)
            .child(
                row()
                    .gap(8.0)
                    .items_center()
                    .child(row().size(18.0, 18.0).radius(6.0).fill(t.primary_bg).items_center().justify_center().child(figure(
                        (index + 1).to_string(),
                        10.0,
                        750.0,
                        t.primary_ink,
                    )))
                    .child(label(&line.detail, 12.0, 550.0, t.ink).truncate()),
            )
            .child(row().px(7.0).py(2.5).radius(7.0).fill(t.wash_2).border(1.0, t.rule).child(label("Inspect ›", 10.5, 650.0, t.ink_2)));
        if let Some(node_id) = target {
            item = item.on_press(move |app: &mut StudioApp| {
                app.canvas.select(Some(node_id.clone()));
                app.right_tab = RightTab::Inspector;
            });
        }
        list.child(item)
    });
    col().key("inspector-trace").gap(10.0).pt(12.0).child(header).child(inspector_section_heading("Brewing sequence", t)).child(steps)
}

fn inspector_body(app: &StudioApp, light: bool, t: &HearthPalette) -> El {
    let Some(card) = app.canvas.selected_card() else {
        if !app.trace.is_empty() {
            return run_summary_inspector(app, t);
        }
        return col()
            .key("inspector-empty")
            .w_full()
            .pt(108.0)
            .px(12.0)
            .items_center()
            .gap(10.0)
            .child(canvas(empty_bottle_mark(light, 42.0)).key("inspector-empty-bobbin").size(42.0, 42.0))
            .child(heading("No ingredient picked", 15.5, 800.0, t.ink))
            .child(col().items_center().gap(2.0).child(label("Select an ingredient in the cauldron", 12.0, 450.0, t.mute)).child(label(
                "to edit it.",
                12.0,
                450.0,
                t.mute,
            )));
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
            el().size(8.5, 8.5).radius(4.25).fill(if flagged { t.hex } else { t.broth_mid })
        } else if flagged {
            el().size(7.0, 7.0).radius(2.0).fill(t.card).border(1.5, t.hex)
        } else if wired || supplied {
            el().size(7.0, 7.0).radius(2.0).fill(tone.liquid)
        } else {
            el().size(7.0, 7.0).radius(2.0).fill(t.card).border(1.5, tone.liquid)
        };
        let mut right_meta = row().gap(8.0).items_center();
        if flagged {
            right_meta = right_meta.child(row().px(6.0).py(1.0).radius(6.0).border(1.0, t.hex).fill(t.hex_tint).child(label("Missing", 9.5, 700.0, t.hex)));
        }
        right_meta = right_meta.child(label(format!("{:?}", socket.ty), if compact { 10.5 } else { 11.0 }, 550.0, if flagged { t.hex } else { t.mute }));
        row()
            .w_full()
            .h(socket_h)
            .items_center()
            .justify_between()
            .border_sides([0.0, 0.0, 1.0, 0.0], if flagged { with_alpha(t.hex, 0.42) } else { t.rule })
            .child(row().gap(8.0).items_center().child(marker).child(label(
                &socket.name,
                if compact { 11.5 } else { 12.0 },
                if socket.ty.is_flow() || flagged { 650.0 } else { 500.0 },
                if flagged { t.hex } else { t.ink },
            )))
            .child(right_meta)
    };
    let mut sockets = col().gap(0.0);
    if !card.inputs.is_empty() {
        sockets = sockets.child(row().w_full().pt(2.0).pb(2.0).child(label("Inputs", 10.5, 650.0, t.mute)));
        sockets = card.inputs.iter().fold(sockets, |list, socket| list.child(socket_row_el(socket, true)));
    }
    if !card.outputs.is_empty() {
        sockets = sockets.child(row().w_full().pt(if compact { 4.0 } else { 6.0 }).pb(2.0).child(label("Outputs", 10.5, 650.0, t.mute)));
        sockets = card.outputs.iter().fold(sockets, |list, socket| list.child(socket_row_el(socket, false)));
    }
    let id = card.id.clone();
    let field = editable_field(&card.kind);
    let editor_height = if has_error { 86.0 } else { prompt_editor_height(app.size) };
    let editor = field.and_then(|field| app.prompts.get(&card.id).map(|edit| (field, edit))).map(|(field, edit)| {
        let input = text_input(edit, prompt_font())
            .key("prompt-field")
            .label(format!("{} {field}", card.title))
            .w_full()
            .color(t.ink)
            .on_edit(move |app: &mut StudioApp| app.prompts.entry(id.clone()).or_insert_with(|| TextEdit::new("")));
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
            box_el = box_el.child(label("Write an incantation or wire the prompt essence…", 11.5, 450.0, t.placeholder).wrap());
        }
        box_el
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
                .child(heading(&card.title, if compact { 14.5 } else { 15.5 }, 800.0, t.ink).truncate())
                .child(text(&card.kind).font(font(10.5, 500.0).mono()).color(t.mute).truncate()),
        );
    let mut column = col().key("inspector-card").gap(if compact { 6.0 } else { 9.0 }).pt(if compact { 8.0 } else { 10.0 }).child(card_header);
    if has_error {
        let flagged_socket = card.inputs.iter().find(|socket| socket.flagged).map(|socket| socket.name.clone());
        let headline = match &flagged_socket {
            Some(socket) => format!("{} needs \"{socket}\"", card.title),
            None => diagnostics.first().cloned().unwrap_or_default(),
        };
        let mend = app.hex_mend_source(&card.id);
        let fix_button = match (&mend, flagged_socket.as_deref()) {
            (Some((_, caption)), _) => Some(button_el("inspector-fix", caption, ButtonKind::Primary, 9.0, t, StudioApp::mend_selected_hex)),
            (None, Some("prompt")) => {
                Some(button_el("inspector-fix", "Use default prompt", ButtonKind::Primary, 9.0, t, StudioApp::fill_default_prompt_for_selected))
            }
            _ => None,
        };
        let mut actions = row().pt(2.0).gap(8.0).items_center();
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
    let mut column = column.child(inspector_section_heading("Sockets", t)).child(sockets);
    if card.kind == "ai/generate_text" {
        let temp_value = app.selected_temperature().to_string();
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
                    .px(if narrow_inspector { 7.0 } else { 9.0 })
                    .py(tile_py)
                    .gap(3.0)
                    .radius(10.0)
                    .continuous_corners()
                    .fill(t.card)
                    .border(1.0, t.rule_2)
                    .child(label("Model", 10.0, 600.0, t.mute))
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
                    .grow(1.0)
                    .min_w(0.0)
                    .px(if narrow_inspector { 6.0 } else { 8.0 })
                    .py(tile_py)
                    .gap(3.0)
                    .radius(10.0)
                    .continuous_corners()
                    .fill(t.card)
                    .border(1.0, t.rule_2)
                    .child(label("Temperature", 10.0, 600.0, t.mute))
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
            let choices = [("openai", "gpt-4o-mini"), ("openai", "gpt-4o"), ("anthropic", "claude-sonnet-4"), ("google", "gemini-2.5-pro")];
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
                            .child(label(if selected { "✓" } else { "" }, 11.0, 750.0, if t.light { t.broth_deep } else { t.broth_foam }))
                            .on_press(move |app: &mut StudioApp| app.select_model(&pick_provider, &pick_model)),
                    )
                },
            );
            column = column.child(menu);
        }
    }
    let column = column.pb(if compact { 4.0 } else { 8.0 });
    let column = match (field, editor) {
        (Some(field), Some(editor)) => column.child(inspector_section_heading(editor_caption(field), t)).child(editor),
        _ => column,
    };
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
            .on_edit(move |app: &mut StudioApp| app.versions.entry(id.clone()).or_insert_with(|| TextEdit::new(""))),
    )
}

pub fn prompt_font() -> TextStyle {
    font(12.0, 450.0).mono()
}

pub fn prompt_editor_height(size: (f32, f32)) -> f32 {
    if size.1 < COMPACT_INSPECTOR_BELOW { COMPACT_PROMPT_EDITOR_HEIGHT } else { PROMPT_EDITOR_HEIGHT }
}

fn editor_caption(field: &str) -> &'static str {
    match field {
        "script" => "Rhai rune",
        "template" => "Template",
        SUBGRAPH_SPELL_FIELD => "Spell ID",
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
        col().w_full().pt(40.0).px(14.0).items_center().gap(10.0).child(canvas(empty_bottle_mark(light, 42.0)).size(42.0, 42.0)).child(heading(
            "No messages yet",
            15.0,
            800.0,
            t.ink,
        ))
    } else {
        transcript.iter().enumerate().fold(col().gap(2.0).pt(8.0).pb(12.0), |mut list, (index, message)| {
            if message.from_user && index > 0 {
                list = list.child(el().w_full().pt(8.0).pb(2.0).clip_rounded(0.0).child(canvas(stitched_sparkles_h(268.0, light)).size(268.0, 7.0)));
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
                let lines = (message.text.len() as f32 / 38.0).ceil().max(1.0);
                let bar_h = (lines * 17.0 - 2.0).clamp(22.0, 114.0);
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
    let send_btn = row()
        .key("chat-send")
        .h(34.0)
        .px(13.0)
        .radius(9.0)
        .continuous_corners()
        .items_center()
        .justify_center()
        .fill(t.primary_bg)
        .child(label("Send", 12.0, 700.0, t.primary_ink))
        .on_press(StudioApp::send_chat);
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

pub fn studio_status(app: &StudioApp) -> Option<(&str, bool)> {
    match (&app.store_error, &app.draft_status, &app.publish_status) {
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

fn trace_display_kind(kind: &str) -> &str {
    match kind {
        "RunStarted" => "Lit",
        "NodeStarted" => "Simmering",
        "NodeFinished" => "Brewed",
        "NodeFailed" => "Hexed",
        "WireActivated" => "Pour",
        "StreamChunk" => "Sip",
        "VariableSet" => "Spice",
        "RunFinished" => "Brewed",
        "RunRejected" => "Hexed",
        "RunCancelled" => "Quenched",
        "RunTimedOut" => "Boiled over",
        other => other,
    }
}

fn trace_strip(app: &StudioApp, t: &HearthPalette) -> El {
    let light = app.theme == Theme::Light;
    let idle = app.trace.is_empty() && !app.is_running();
    let count = app.trace.len();
    let event_word = if count == 1 { "event" } else { "events" };
    if idle {
        let run_hint = if app.canvas.nodes.is_empty() {
            "Idle · no brews yet.".to_owned()
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
            .child(canvas(brand_cauldron_mark(light)).key("trace-bobbin").size(26.0, 22.0))
            .child(heading("Cauldron Log", 13.0, 800.0, t.ink))
            .child(label(run_hint, 12.0, 450.0, t.mute));
        if let Some(undo_msg) = &app.undo_banner {
            bar = bar.child(
                row()
                    .key("undo-banner")
                    .gap(7.0)
                    .px(9.0)
                    .py(3.0)
                    .radius(9.0)
                    .items_center()
                    .fill(t.card)
                    .border(1.0, t.rule_2)
                    .child(label(undo_msg, 11.2, 600.0, t.ink))
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
            );
        }
        if let Some(badge) = status_badge(app, t) {
            bar = bar.child(el().grow(1.0).min_w(0.0)).child(badge);
        }
        return bar;
    }
    let finished_steps = app.trace.iter().filter(|line| line.kind == "NodeFinished" || line.kind == "NodeFailed").count();
    let blocked_pre_run = !app.is_running() && finished_steps == 0 && app.trace.iter().all(|line| line.kind == "RunRejected");
    let show_expand_chip = !app.is_running() && finished_steps > 0 && count > 1;
    let step_word = if finished_steps == 1 { "step" } else { "steps" };
    let issue_word = if count == 1 { "issue" } else { "issues" };
    let summary = if app.is_running() {
        format!("Brew #{} · simmering · {} {} · Quench stops it", app.runs, count, event_word)
    } else if blocked_pre_run {
        format!("Check · {count} {issue_word}")
    } else if show_expand_chip {
        format!("Brew #{}", app.runs)
    } else if finished_steps > 0 {
        format!("Brew #{} · {finished_steps} {step_word}", app.runs)
    } else {
        format!("Brew #{} · {count} {event_word}", app.runs)
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
        .child(canvas(brand_cauldron_mark(light)).key("trace-bobbin").size(22.0, 19.0))
        .child(heading("Cauldron Log", 13.0, 800.0, t.ink))
        .child(label(summary, 12.0, 500.0, t.mute));
    if show_expand_chip {
        let toggle_label = if app.trace_expanded { "Key steps ▴".to_owned() } else { format!("{finished_steps} {step_word} · {count} {event_word} ▾") };
        header = header.child(
            row()
                .key("trace-expand")
                .px(8.0)
                .py(2.0)
                .radius(8.0)
                .fill(t.card)
                .border(1.0, t.rule_2)
                .child(label(toggle_label, 11.0, 650.0, t.ink))
                .on_press(|app: &mut StudioApp| app.trace_expanded = !app.trace_expanded),
        );
    }
    if let Some(badge) = status_badge(app, t) {
        header = header.child(el().grow(1.0).min_w(0.0)).child(badge);
    }
    let run_done = !app.is_running();
    let total = milestones.len();
    let mut step_num = 0usize;
    let body = milestones.iter().enumerate().fold(row().gap(0.0).items_center(), |list, (index, line)| {
        let dot_color = match line.status {
            ExecutionStatus::Failed => t.hex,
            ExecutionStatus::Running if !run_done => t.gold,
            ExecutionStatus::Running | ExecutionStatus::Succeeded => t.broth_mid,
            ExecutionStatus::Idle => t.mute,
        };
        let mut entry = row()
            .gap(6.0)
            .h(29.0)
            .px(9.0)
            .radius(10.0)
            .continuous_corners()
            .items_center()
            .fill(if line.status == ExecutionStatus::Failed { t.hex_tint } else { t.card })
            .border(1.0, if line.status == ExecutionStatus::Failed { t.hex } else { t.rule_2 });
        if line.kind == "NodeFinished" {
            step_num += 1;
            entry = entry
                .child(row().size(17.0, 17.0).radius(6.0).fill(t.primary_bg).items_center().justify_center().child(figure(
                    step_num.to_string(),
                    9.5,
                    750.0,
                    t.primary_ink,
                )))
                .child(label(&line.detail, 11.5, 600.0, t.ink));
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
        if let Some(target_node) = line.node.clone() {
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
    col()
        .key("trace-strip")
        .w_full()
        .h(TRACE_HEIGHT)
        .min_h(0.0)
        .px(16.0)
        .pt(13.0)
        .pb(10.0)
        .gap(9.0)
        .fill(t.shelf)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.rule)
        .child(header)
        .child(col().key("trace-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: true }).child(body))
}
