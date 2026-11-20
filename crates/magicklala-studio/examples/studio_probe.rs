use std::path::PathBuf;
use std::process::ExitCode;

use magicklala_canvas::ExecutionStatus;
use magicklala_studio::{StudioApp, Theme, ValidationState, render_headless_snapshot_sized, studio_view};
use rune::{Env, Runtime};
use serde_json::json;

fn click(rt: &mut Runtime<StudioApp>, key: &str) -> Result<(), String> {
    rt.frame(0.0);
    let (x, y) = rt.find(key).ok_or_else(|| format!("no element keyed {key}"))?;
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    Ok(())
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

fn probe(desktop: PathBuf, compact: PathBuf) -> Result<serde_json::Value, String> {
    let mut rt = Runtime::new(StudioApp::demo(), studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let default_nodes = rt.state.canvas.nodes.len();
    let default_wires = rt.state.canvas.wires.len();
    let titles: Vec<String> = rt.state.canvas.nodes.iter().map(|card| card.title.clone()).collect();

    let palette_kind = "memory/store";
    let nodes_before = rt.state.canvas.nodes.len();
    click(&mut rt, "palette-memory-store")?;
    let nodes_after = rt.state.canvas.nodes.len();
    let spawned_kind = rt.state.canvas.nodes.last().map(|card| card.kind.clone()).unwrap_or_default();

    let dragged = "search";
    let before = rt.state.canvas.card(&dragged.into()).map(|card| rt.state.canvas.camera.world_to_screen(card.position)).ok_or("search node missing")?;
    let pointer_delta = (64.0_f32, 36.0_f32);
    drag(&mut rt, &format!("node-{dragged}-header"), pointer_delta)?;
    let after = rt.state.canvas.card(&dragged.into()).map(|card| rt.state.canvas.camera.world_to_screen(card.position)).ok_or("search node missing")?;

    click(&mut rt, "validate")?;
    let validation = rt.state.validation.clone();
    if let ValidationState::Invalid(errors) = &validation {
        eprintln!("validation errors: {errors:?}");
    }

    let spawned_id = rt.state.canvas.nodes.last().map(|card| card.id.clone()).ok_or("spawned node missing")?;
    rt.update(|app, _| {
        app.canvas.select(Some(spawned_id.clone()));
        app.canvas.remove_selected_node();
        app.sync_spell();
    });
    rt.frame(0.0);

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

    let mut renders = Vec::new();
    for (path, size) in [(desktop, (1280.0, 800.0)), (compact, (960.0, 640.0))] {
        render_headless_snapshot_sized(&path, size).map_err(|error| error.to_string())?;
        let bits = entropy(&path)?;
        if bits < 4.0 {
            return Err(format!("{} looks blank: {bits:.2} bits per byte", path.display()));
        }
        renders.push(json!({ "path": path.display().to_string(), "width": size.0, "height": size.1, "entropy": bits }));
    }

    Ok(json!({
        "defaultSpell": { "nodes": default_nodes, "wires": default_wires, "titles": titles },
        "spawn": { "paletteKind": palette_kind, "nodesBefore": nodes_before, "nodesAfter": nodes_after, "spawnedKind": spawned_kind },
        "drag": { "node": dragged, "before": before, "after": after, "pointerDelta": [pointer_delta.0, pointer_delta.1] },
        "validate": { "status": validation.label() },
        "run": { "events": events, "succeededNodes": succeeded, "activeWires": active_wires },
        "theme": { "before": theme_before, "after": theme_after },
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
