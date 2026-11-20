use std::path::PathBuf;
use std::process::ExitCode;

use magicklala_canvas::ExecutionStatus;
use magicklala_graph::{NodeCatalog, SpellValidator};
use magicklala_studio::{StudioApp, Theme, ValidationState, render_headless_snapshot_themed, studio_view};
use rune::{Env, Runtime};
use serde_json::json;

const TOOLBAR_KEYS: [&str; 10] =
    ["toolbar-left", "validation-pill", "validate", "run-spell", "publish", "theme-toggle", "zoom-out", "zoom-readout", "zoom-in", "zoom-fit"];
const CONTEXT_MENU_ITEMS: [&str; 4] = ["context-add-ai-generate_text", "context-delete-node", "context-fit-view", "context-dismiss"];

fn click(rt: &mut Runtime<StudioApp>, key: &str) -> Result<(), String> {
    rt.frame(0.0);
    let (x, y) = rt.find(key).ok_or_else(|| format!("no element keyed {key}"))?;
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    Ok(())
}

fn right_click(rt: &mut Runtime<StudioApp>, point: (f32, f32)) {
    rt.frame(0.0);
    rt.pointer_move(point.0, point.1);
    rt.secondary_down(point.0, point.1);
    rt.secondary_up(point.0, point.1);
    rt.frame(0.0);
}

fn drag(rt: &mut Runtime<StudioApp>, key: &str, delta: (f32, f32)) -> Result<(), String> {
    rt.frame(0.0);
    let (x, y) = rt.find(key).ok_or_else(|| format!("no element keyed {key}"))?;
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    for step in 1..=8 {
        let share = step as f32 / 8.0;
        rt.pointer_move(x + delta.0 * share, y + delta.1 * share);
        rt.frame(0.0);
    }
    rt.pointer_up(x + delta.0, y + delta.1);
    rt.frame(0.0);
    Ok(())
}

fn entropy(path: &PathBuf) -> Result<f64, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut counts = [0usize; 256];
    bytes.iter().for_each(|byte| counts[*byte as usize] += 1);
    let total = bytes.len() as f64;
    Ok(counts.iter().filter(|count| **count > 0).map(|count| *count as f64 / total).map(|share| -share * share.log2()).sum())
}

fn rect(rt: &Runtime<StudioApp>, key: &str) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|element| element.key.as_deref() == Some(key)).map(|element| element.rect)
}

fn sized_runtime(size: (f32, f32), theme: Theme) -> Runtime<StudioApp> {
    let mut app = StudioApp::demo();
    app.size = size;
    if theme != app.theme {
        app.toggle_theme();
    }
    let viewport = app.canvas_size();
    app.canvas.fit(viewport);
    let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn toolbar_bounds(size: (f32, f32), theme: Theme) -> serde_json::Value {
    let rt = sized_runtime(size, theme);
    let items: Vec<serde_json::Value> = TOOLBAR_KEYS
        .iter()
        .map(|key| match rect(&rt, key) {
            Some([x, y, w, h]) => json!({ "key": key, "left": x, "right": x + w, "top": y, "bottom": y + h }),
            None => json!({ "key": key, "left": null, "right": null, "top": null, "bottom": null }),
        })
        .collect();
    json!({ "width": size.0, "height": size.1, "theme": theme.label(), "items": items })
}

fn context_menu(rt: &mut Runtime<StudioApp>) -> Result<serde_json::Value, String> {
    let canvas = rect(rt, "canvas").ok_or("canvas missing")?;
    let corner = (canvas[0] + canvas[2] - 6.0, canvas[1] + canvas[3] - 6.0);
    right_click(rt, corner);
    let opened = rt.state.canvas.context_menu.is_some() && rt.find("context-menu").is_some();
    let menu = rect(rt, "context-menu");
    let inside =
        menu.is_some_and(|[x, y, w, h]| x >= canvas[0] && y >= canvas[1] && x + w <= canvas[0] + canvas[2] + 0.5 && y + h <= canvas[1] + canvas[3] + 0.5);
    let items: Vec<String> = CONTEXT_MENU_ITEMS.iter().filter(|key| rt.find(key).is_some()).map(|key| (*key).to_owned()).collect();

    let add_before = rt.state.canvas.nodes.len();
    let add_clicked = click(rt, "context-add-ai-generate_text").is_ok();
    let add_after = rt.state.canvas.nodes.len();
    let added_kind = if add_after > add_before { rt.state.canvas.nodes.last().map(|card| card.kind.clone()) } else { None };
    let closed_after_add = rt.state.canvas.context_menu.is_none() && rt.find("context-menu").is_none();
    let spell_nodes_after_add = rt.state.spell.nodes.len();

    let target = rt
        .state
        .canvas
        .nodes
        .iter()
        .find(|card| card.id.0 == "search")
        .map(|card| rt.state.canvas.camera.world_to_screen([card.position[0] + 20.0, card.position[1] + 12.0]));
    let delete = match target {
        Some(local) => {
            right_click(rt, (canvas[0] + local[0], canvas[1] + local[1]));
            let selected = rt.state.canvas.selection.as_ref().map(|id| id.0.clone());
            let before = rt.state.canvas.nodes.len();
            let clicked = click(rt, "context-delete-node").is_ok();
            let still_there = rt.state.canvas.nodes.iter().any(|card| card.id.0 == "search");
            json!({ "selected": selected, "clicked": clicked, "before": before, "after": rt.state.canvas.nodes.len(), "searchRemains": still_there, "spellNodes": rt.state.spell.nodes.len() })
        }
        None => json!({ "selected": null, "clicked": false, "before": 0, "after": 0, "searchRemains": true, "spellNodes": 0 }),
    };

    rt.update(|app, _| app.canvas.camera.zoom_by(1.0 / 3.0, [10.0, 10.0]));
    let zoom_before_fit = rt.state.canvas.camera.zoom;
    right_click(rt, (canvas[0] + 30.0, canvas[1] + 30.0));
    let fit_clicked = click(rt, "context-fit-view").is_ok();
    let zoom_after_fit = rt.state.canvas.camera.zoom;
    let mut expected = rt.state.canvas.clone();
    expected.fit(rt.state.canvas_size());

    right_click(rt, (canvas[0] + 30.0, canvas[1] + 30.0));
    let reopened = rt.find("context-menu").is_some();
    let dismiss_clicked = click(rt, "context-dismiss").is_ok();
    let dismissed = rt.state.canvas.context_menu.is_none() && rt.find("context-menu").is_none();

    Ok(json!({
        "opened": opened,
        "insideCanvas": inside,
        "items": items,
        "add": { "clicked": add_clicked, "before": add_before, "after": add_after, "kind": added_kind, "closed": closed_after_add, "spellNodes": spell_nodes_after_add },
        "delete": delete,
        "fit": { "clicked": fit_clicked, "zoomBefore": zoom_before_fit, "zoomAfter": zoom_after_fit, "expectedZoom": expected.camera.zoom },
        "dismiss": { "reopened": reopened, "clicked": dismiss_clicked, "dismissed": dismissed },
    }))
}

fn probe(desktop: PathBuf, compact: PathBuf) -> Result<serde_json::Value, String> {
    let registry = magicklala_nodes::default_registry();
    let registry_kinds: Vec<String> = registry.specs().into_iter().map(|spec| spec.kind).collect();
    let mut rt = Runtime::new(StudioApp::demo(), studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let default_nodes = rt.state.canvas.nodes.len();
    let default_wires = rt.state.canvas.wires.len();
    let titles: Vec<String> = rt.state.canvas.nodes.iter().map(|card| card.title.clone()).collect();
    let kinds: Vec<String> = rt.state.spell.nodes.iter().map(|node| node.kind.clone()).collect();
    let registry_validation = match SpellValidator::new(registry.specs()).validate(&rt.state.spell) {
        Ok(()) => vec![],
        Err(errors) => errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
    };
    let palette_kinds: Vec<String> =
        rt.state.specs.iter().map(|spec| spec.kind.clone()).filter(|kind| rt.find(&format!("palette-{}", kind.replace('/', "-"))).is_some()).collect();
    let app_kinds: Vec<String> = rt.state.specs.iter().map(|spec| spec.kind.clone()).collect();

    let palette_kind = "flow/branch";
    let nodes_before = rt.state.canvas.nodes.len();
    click(&mut rt, "palette-flow-branch")?;
    let nodes_after = rt.state.canvas.nodes.len();
    let spawned_kind = rt.state.canvas.nodes.last().map(|card| card.kind.clone()).unwrap_or_default();

    let dragged = "search";
    let before = rt.state.canvas.card(&dragged.into()).map(|card| rt.state.canvas.camera.world_to_screen(card.position)).ok_or("search node missing")?;
    let pointer_delta = (64.0_f32, 36.0_f32);
    drag(&mut rt, &format!("node-{dragged}-header"), pointer_delta)?;
    let after = rt.state.canvas.card(&dragged.into()).map(|card| rt.state.canvas.camera.world_to_screen(card.position)).ok_or("search node missing")?;

    let spawned_id = rt.state.canvas.nodes.last().map(|card| card.id.clone()).ok_or("spawned node missing")?;
    rt.update(|app, _| {
        app.canvas.select(Some(spawned_id.clone()));
        app.canvas.remove_selected_node();
        app.sync_spell();
    });
    rt.frame(0.0);

    click(&mut rt, "validate")?;
    let validation = rt.state.validation.clone();
    if let ValidationState::Invalid(errors) = &validation {
        eprintln!("validation errors: {errors:?}");
    }

    click(&mut rt, "run-spell")?;
    let events: Vec<String> = rt.state.trace.iter().map(|line| line.kind.to_owned()).collect();
    let succeeded = rt.state.canvas.nodes.iter().filter(|card| card.status == ExecutionStatus::Succeeded).count();
    let active_wires = rt.state.canvas.wires.iter().filter(|wire| wire.active).count();

    let theme_before = rt.state.theme.label();
    click(&mut rt, "theme-toggle")?;
    let theme_after = rt.state.theme.label();
    if rt.state.theme != Theme::Light || !rt.state.canvas.light {
        return Err("theme toggle did not reach the canvas".to_owned());
    }

    let mut menu_rt = sized_runtime((1280.0, 800.0), Theme::Dark);
    let menu = context_menu(&mut menu_rt)?;

    let toolbar: Vec<serde_json::Value> =
        [(960.0, 640.0), (1280.0, 800.0)].into_iter().flat_map(|size| [Theme::Dark, Theme::Light].map(|theme| toolbar_bounds(size, theme))).collect();

    let mut renders = Vec::new();
    for (path, size) in [(desktop, (1280.0, 800.0)), (compact, (960.0, 640.0))] {
        for theme in [Theme::Light, Theme::Dark] {
            let target = if theme == Theme::Dark { path.clone() } else { path.with_extension("light.png") };
            render_headless_snapshot_themed(&target, size, theme).map_err(|error| error.to_string())?;
            let bits = entropy(&target)?;
            if bits < 4.0 {
                return Err(format!("{} looks blank: {bits:.2} bits per byte", target.display()));
            }
            renders.push(json!({ "path": target.display().to_string(), "width": size.0, "height": size.1, "theme": theme.label(), "entropy": bits }));
        }
    }

    Ok(json!({
        "registry": { "kinds": registry_kinds, "appKinds": app_kinds, "paletteKinds": palette_kinds },
        "defaultSpell": { "nodes": default_nodes, "wires": default_wires, "titles": titles, "kinds": kinds, "registryErrors": registry_validation },
        "spawn": { "paletteKind": palette_kind, "nodesBefore": nodes_before, "nodesAfter": nodes_after, "spawnedKind": spawned_kind },
        "drag": { "node": dragged, "before": before, "after": after, "pointerDelta": [pointer_delta.0, pointer_delta.1] },
        "validate": { "status": validation.label() },
        "run": { "events": events, "succeededNodes": succeeded, "activeWires": active_wires },
        "theme": { "before": theme_before, "after": theme_after },
        "toolbar": toolbar,
        "contextMenu": menu,
        "renders": renders,
    }))
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).map(PathBuf::from);
    let (Some(desktop), Some(compact)) = (args.next(), args.next()) else {
        eprintln!("usage: studio_probe <desktop.png> <compact.png>");
        return ExitCode::from(2);
    };
    match probe(desktop, compact) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("studio probe failed: {error}");
            ExitCode::FAILURE
        }
    }
}
