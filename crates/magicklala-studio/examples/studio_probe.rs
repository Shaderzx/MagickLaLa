use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use magicklala_canvas::{ExecutionStatus, SocketRef, category_accent};
use magicklala_graph::{NodeCatalog, NodeId, NodeInstance, SocketType, SpellGraph, SpellId, SpellValidator, Wire, WireId};
use magicklala_store::{NewProject, NewSpell, Repository, SqliteStore, StoreError};
use magicklala_studio::{
    LeftTab, PALETTE_CATEGORY_ORDER, PROMPT_EDITOR_PADDING, Release, RightTab, StudioApp, StudioConfig, StudioStore, Theme, ValidationState,
    palette_categories, prompt_font, registry_specs, render_headless_snapshot_state, render_headless_snapshot_themed, studio_text_system, studio_view,
};
use rune::{Env, Runtime};
use serde_json::json;

const TOOLBAR_KEYS: [&str; 11] =
    ["toolbar-left", "validation-pill", "validate", "run-spell", "save", "publish", "theme-toggle", "zoom-out", "zoom-readout", "zoom-in", "zoom-fit"];
const CONTEXT_MENU_ITEMS: [&str; 5] = ["context-duplicate-node", "context-add-ai-generate_text", "context-delete-node", "context-fit-view", "context-dismiss"];

const SETTLE: Duration = Duration::from_secs(120);

fn settle(rt: &mut Runtime<StudioApp>) -> Result<(), String> {
    rt.update(|app, _| {
        app.wait_background(SETTLE);
    });
    rt.frame(0.0);
    if !rt.state.is_running() { Ok(()) } else { Err("background run did not finish".to_owned()) }
}

fn click(rt: &mut Runtime<StudioApp>, key: &str) -> Result<(), String> {
    click_at(rt, key, 0.0)
}

fn click_at(rt: &mut Runtime<StudioApp>, key: &str, seconds: f64) -> Result<(), String> {
    rt.frame(seconds);
    let (x, y) = rt.find(key).ok_or_else(|| format!("no element keyed {key}"))?;
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(seconds);
    Ok(())
}

struct Clock(f64);

impl Clock {
    fn tick(&mut self) -> f64 {
        self.0 += 0.5;
        self.0
    }
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
    let mut rt = Runtime::with_text(app, studio_view, size, Env::new(true, 1.0), studio_text_system());
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
    rt.update(|app, _| {
        app.canvas.mode = magicklala_canvas::WorkbenchMode::Grimoire;
    });
    rt.frame(0.0);
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
    let on_card = match target {
        Some(local) => {
            right_click(rt, (canvas[0] + local[0], canvas[1] + local[1]));
            let items: Vec<String> = CONTEXT_MENU_ITEMS.iter().filter(|key| rt.find(key).is_some()).map(|key| (*key).to_owned()).collect();
            let before = rt.state.canvas.nodes.len();
            let clicked = click(rt, "context-duplicate-node").is_ok();
            let fresh = rt.state.canvas.nodes.iter().skip(before).map(|card| card.kind.clone()).collect::<Vec<_>>();
            json!({ "items": items, "duplicateClicked": clicked, "duplicated": fresh, "closed": rt.state.canvas.context_menu.is_none() })
        }
        None => json!({ "items": [], "duplicateClicked": false, "duplicated": [], "closed": false }),
    };
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
        "onCard": on_card,
        "delete": delete,
        "fit": { "clicked": fit_clicked, "zoomBefore": zoom_before_fit, "zoomAfter": zoom_after_fit, "expectedZoom": expected.camera.zoom },
        "dismiss": { "reopened": reopened, "clicked": dismiss_clicked, "dismissed": dismissed },
    }))
}

fn run_report(app: &StudioApp) -> serde_json::Value {
    let report = app.last_run.clone();
    let failed: Vec<String> = app.canvas.nodes.iter().filter(|card| card.status == ExecutionStatus::Failed).map(|card| card.id.0.clone()).collect();
    json!({
        "status": report.as_ref().map(|report| report.outcome.label()),
        "events": app.trace.iter().map(|line| line.kind).collect::<Vec<_>>(),
        "failedNodes": failed,
        "replies": report.as_ref().map(|report| report.replies.clone()).unwrap_or_default(),
        "error": report.and_then(|report| report.error),
        "validation": app.validation.label(),
    })
}

fn example(name: &str) -> Result<SpellGraph, String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name);
    let text = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{name}: {error}"))
}

fn runtime_for(spell: SpellGraph) -> Runtime<StudioApp> {
    let mut rt = Runtime::new(StudioApp::with_spell(spell, registry_specs()), studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn preserve(name: &str) -> Result<serde_json::Value, String> {
    let mut rt = runtime_for(example(name)?);
    let canvas_wires = rt.state.canvas.wires.len();
    rt.update(|app, _| app.sync_spell());
    click(&mut rt, "validate")?;
    if let ValidationState::Invalid(errors) = &rt.state.validation {
        eprintln!("{name} validation errors: {errors:?}");
    }
    let configs: serde_json::Map<String, serde_json::Value> = rt.state.spell.nodes.iter().map(|node| (node.id.0.clone(), node.config.clone())).collect();
    Ok(json!({
        "file": name,
        "canvasWires": canvas_wires,
        "validation": rt.state.validation.label(),
        "wireIds": rt.state.spell.wires.iter().map(|wire| wire.id.0.clone()).collect::<Vec<_>>(),
        "configs": configs,
    }))
}

fn edit_script() -> Result<serde_json::Value, String> {
    let mut rt = runtime_for(example("webhook_router.json")?);
    let node = NodeId::from("classify");
    let script = "#{ route: \"orders\", message: \"Order \" + body[\"id\"] + \" edited\" }".to_owned();
    let edited = script.clone();
    rt.update(move |app, _| {
        app.canvas.select(Some(node.clone()));
        if let Some(edit) = app.prompts.get_mut(&node) {
            *edit = rune::TextEdit::new(&edited);
        }
    });
    click(&mut rt, "validate")?;
    let config = rt.state.spell.nodes.iter().find(|node| node.id.0 == "classify").map(|node| node.config.clone()).unwrap_or_default();
    Ok(json!({ "node": "classify", "script": script, "config": config }))
}

fn echo_spell() -> SpellGraph {
    let mut spell = SpellGraph::new("Echo Agent");
    let place =
        |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position: [x, 0.0], config };
    spell.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("echo", "text/template", 280.0, json!({ "template": "Echo: {{content}}" })),
        place("reply", "io/send_message", 560.0, serde_json::Value::Null),
    ];
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    spell.wires = vec![
        wire("w1", ("trigger", "out"), ("reply", "in")),
        wire("w2", ("trigger", "content"), ("echo", "content")),
        wire("w3", ("echo", "text"), ("reply", "content")),
    ];
    spell
}

fn tasting_spell() -> SpellGraph {
    let mut spell = echo_spell();
    spell.name = "Tasting Agent".to_owned();
    if let Some(node) = spell.nodes.iter_mut().find(|node| node.id.0 == "echo") {
        node.config = json!({ "template": "You asked: {{content}}" });
    }
    spell
}

fn type_chat(rt: &mut Runtime<StudioApp>, message: &str) -> Result<String, String> {
    click(rt, "tab-playground")?;
    let text = message.to_owned();
    rt.update(move |app, _| {
        app.chat_input = rune::TextEdit::new(&text);
        app.send_chat();
    });
    settle(rt)?;
    rt.state.chat.last().filter(|reply| !reply.from_user).map(|reply| reply.text.clone()).ok_or_else(|| "no chat reply".to_owned())
}

fn execution() -> Result<serde_json::Value, String> {
    let mut webhook = runtime_for(example("webhook_router.json")?);
    webhook.update(|app, _| app.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } }));
    click(&mut webhook, "run-spell")?;
    settle(&mut webhook)?;
    let mut broken = StudioApp::with_spell(example("webhook_router.json")?, registry_specs());
    broken.spell.nodes.push(NodeInstance {
        id: NodeId::from("mystery"),
        kind: "nope/unknown".to_owned(),
        position: [0.0, 400.0],
        config: serde_json::Value::Null,
    });
    broken.canvas = magicklala_canvas::CanvasState::from_graph(&broken.spell, &broken.specs);
    let mut invalid = Runtime::new(broken, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    click(&mut invalid, "run-spell")?;
    settle(&mut invalid)?;
    Ok(json!({ "webhook": run_report(&webhook.state), "invalid": run_report(&invalid.state) }))
}

fn chat() -> Result<serde_json::Value, String> {
    let message = "hello studio";
    let mut echo = runtime_for(echo_spell());
    let echo_reply = type_chat(&mut echo, message)?;
    let mut failing = Runtime::new(StudioApp::from_registry(), studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    let failing_reply = type_chat(&mut failing, message)?;
    let failing_error = failing.state.last_run.as_ref().and_then(|report| report.error.clone());
    Ok(json!({ "message": message, "echoReply": echo_reply, "failingReply": failing_reply, "failingError": failing_error }))
}

fn publish(db_path: &std::path::Path) -> Result<serde_json::Value, String> {
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    click(&mut rt, "publish")?;
    if let Some(error) = &rt.state.store_error {
        return Err(format!("publish failed: {error}"));
    }
    let version = rt.state.releases.last().map(|release| release.version).ok_or("publish created no release")?;
    let graph = serde_json::to_value(&rt.state.last_published.as_ref().ok_or("no published release")?.graph).map_err(|error| error.to_string())?;
    drop(rt);
    let reopened = StudioApp::open(&config).map_err(|error| error.to_string())?;
    let spell = serde_json::to_value(&reopened.spell).map_err(|error| error.to_string())?;
    Ok(json!({
        "version": version,
        "graph": graph,
        "reopened": { "versions": reopened.releases.iter().map(|release| release.version).collect::<Vec<_>>(), "spell": spell },
    }))
}

fn delay_spell(ms: u64) -> SpellGraph {
    let mut spell = SpellGraph::new("Slow Agent");
    let place =
        |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position: [x, 0.0], config };
    spell.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("wait", "time/delay", 280.0, json!({ "ms": ms })),
        place("reply", "io/send_message", 560.0, serde_json::Value::Null),
    ];
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    spell.wires = vec![
        wire("w1", ("trigger", "out"), ("wait", "in")),
        wire("w2", ("wait", "out"), ("reply", "in")),
        wire("w3", ("trigger", "content"), ("reply", "content")),
    ];
    spell
}

fn toolbar_run_buttons(rt: &mut Runtime<StudioApp>, seconds: f64) -> serde_json::Value {
    rt.frame(seconds);
    json!({ "cancel": rt.find("cancel-run").is_some(), "run": rt.find("run-spell").is_some() })
}

fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn delay_started(app: &StudioApp) -> bool {
    app.trace_events.iter().any(|event| matches!(event, magicklala_graph::TraceEvent::NodeStarted { node_id, .. } if node_id.0 == "wait"))
}

fn delay_status(app: &StudioApp) -> &'static str {
    app.canvas.card(&NodeId::from("wait")).map_or("missing", |card| card.status.label())
}

fn slow_runtime(ms: u64, run_timeout: Duration) -> Runtime<StudioApp> {
    let mut app = StudioApp::with_spell(delay_spell(ms), registry_specs());
    app.run_timeout = run_timeout;
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn cancel_run(delay_ms: u64, deadline_ms: u64) -> Result<serde_json::Value, String> {
    let mut clock = Clock(0.0);
    let mut rt = slow_runtime(delay_ms, Duration::from_secs(600));
    let begun = Instant::now();
    click_at(&mut rt, "run-spell", clock.tick())?;
    let start_ms = elapsed_ms(begun);
    let running_after_start = rt.state.is_running();
    let toolbar_while_running = toolbar_run_buttons(&mut rt, clock.tick());
    rt.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    let live_events: Vec<&str> = rt.state.trace.iter().map(|line| line.kind).collect();
    let delay_status_while_running = delay_status(&rt.state);
    let cancel_begun = Instant::now();
    click_at(&mut rt, "cancel-run", clock.tick())?;
    let cancel_ms = elapsed_ms(cancel_begun);
    let running_after_cancel = rt.state.is_running();
    let background_stopped = rt.update_if(|app, _| app.wait_stopped(SETTLE)) || rt.state.background_tasks() == 0;
    let run = json!({
        "runningAfterStart": running_after_start,
        "startMs": start_ms,
        "toolbarWhileRunning": toolbar_while_running,
        "liveEvents": live_events,
        "delayStatusWhileRunning": delay_status_while_running,
        "cancelMs": cancel_ms,
        "runningAfterCancel": running_after_cancel,
        "outcome": rt.state.last_run.as_ref().map(|report| report.outcome.label()),
        "lastTrace": rt.state.trace.last().map(|line| line.kind),
        "backgroundStopped": background_stopped && rt.state.background_tasks() == 0,
        "totalMs": elapsed_ms(begun),
        "toolbarAfterCancel": toolbar_run_buttons(&mut rt, clock.tick()),
    });

    let message = "are you still there";
    let mut chat_clock = Clock(0.0);
    let mut chat = slow_runtime(delay_ms, Duration::from_secs(600));
    click_at(&mut chat, "tab-playground", chat_clock.tick())?;
    let text = message.to_owned();
    let send_begun = Instant::now();
    chat.update(move |app, _| {
        app.chat_input = rune::TextEdit::new(&text);
        app.send_chat();
    });
    let send_ms = elapsed_ms(send_begun);
    let running_after_send = chat.state.is_running();
    let last_bubble = chat.state.chat.last().map(|bubble| json!({ "fromUser": bubble.from_user, "text": bubble.text }));
    let input_cleared = chat.state.chat_input.text.is_empty();
    chat.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    click_at(&mut chat, "cancel-run", chat_clock.tick())?;
    let chat_report = json!({
        "message": message,
        "runningAfterSend": running_after_send,
        "sendMs": send_ms,
        "lastBubble": last_bubble,
        "inputCleared": input_cleared,
        "runNumber": chat.state.runs,
        "replyAfterCancel": chat.state.chat.last().map(|bubble| json!({ "fromUser": bubble.from_user, "text": bubble.text })),
    });

    let mut timed = slow_runtime(delay_ms, Duration::from_millis(deadline_ms));
    let timed_begun = Instant::now();
    click(&mut timed, "run-spell")?;
    settle(&mut timed)?;
    let deadline = json!({
        "timeoutMs": u64::try_from(timed.state.run_timeout.as_millis()).unwrap_or(u64::MAX),
        "outcome": timed.state.last_run.as_ref().map(|report| report.outcome.label()),
        "lastTrace": timed.state.trace.last().map(|line| line.kind),
        "error": timed.state.last_run.as_ref().and_then(|report| report.error.clone()),
        "running": timed.state.is_running(),
        "totalMs": elapsed_ms(timed_begun),
    });

    let delay = rt.state.spell.nodes.iter().find(|node| node.kind == "time/delay").map(|node| json!({ "kind": node.kind, "ms": node.config["ms"] }));
    Ok(json!({ "delayNode": delay, "run": run, "chat": chat_report, "deadline": deadline }))
}

fn publish_renamed(db_path: &Path, renamed: &str) -> Result<serde_json::Value, String> {
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut rt = Runtime::new(StudioApp::open(&config).map_err(|error| error.to_string())?, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    let released = |rt: &Runtime<StudioApp>| -> Result<serde_json::Value, String> {
        if let Some(error) = &rt.state.store_error {
            return Err(format!("publish failed: {error}"));
        }
        let release = rt.state.last_published.as_ref().ok_or("publish created no release")?;
        Ok(json!({ "version": release.version, "spellId": release.spell_id, "name": release.graph.name }))
    };
    click(&mut rt, "publish")?;
    let first = released(&rt)?;
    let name = renamed.to_owned();
    rt.update(move |app, _| app.spell.name = name);
    click(&mut rt, "publish")?;
    let second = released(&rt)?;
    drop(rt);
    let mut reopened = Runtime::new(StudioApp::open(&config).map_err(|error| error.to_string())?, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    let reopened_report = json!({
        "versions": reopened.state.releases.iter().map(|release| release.version).collect::<Vec<_>>(),
        "spellName": reopened.state.spell.name,
        "spellRecordId": reopened.state.spell_record_id(),
    });
    type_prompt(&mut reopened, &NodeId::from("llm"), "v3 prompt for {{content}}")?;
    reopened.update(|app, _| app.sync_spell());
    reopened.frame(0.0);
    click(&mut reopened, "publish")?;
    let third = released(&reopened)?;
    Ok(json!({ "first": first, "second": second, "reopened": reopened_report, "third": third }))
}

fn palette() -> serde_json::Value {
    let specs = registry_specs();
    let accents: serde_json::Map<String, serde_json::Value> = PALETTE_CATEGORY_ORDER
        .iter()
        .map(|category| {
            let color = category_accent(category);
            ((*category).to_owned(), json!([color.r, color.g, color.b, color.a]))
        })
        .collect();
    json!({ "categories": palette_categories(&specs), "order": PALETTE_CATEGORY_ORDER, "accents": accents })
}

fn inspector(size: (f32, f32)) -> Result<serde_json::Value, String> {
    let mut rt = sized_runtime(size, Theme::Light);
    if rt.find("prompt-unplug").is_some() {
        click(&mut rt, "prompt-unplug")?;
        rt.frame(0.0);
    }
    let [card_x, card_y, card_w, card_h] = rect(&rt, "prompt-editor").ok_or("prompt editor missing")?;
    let [field_x, field_y, field_w, _] = rect(&rt, "prompt-field").ok_or("prompt field missing")?;
    let panel = rect(&rt, "inspector").map(|[_, y, _, h]| y + h);
    let selected = rt.state.canvas.selection.clone().ok_or("no node selected")?;
    let prompt = rt.state.prompts.get(&selected).map(|edit| edit.text.clone()).ok_or("selected node has no prompt")?;
    Ok(json!({
        "width": size.0,
        "height": size.1,
        "editorBottom": card_y + card_h,
        "panelBottom": panel,
        "editorHeight": card_h,
        "card": { "left": card_x, "right": card_x + card_w, "top": card_y, "bottom": card_y + card_h },
        "field": { "left": field_x, "right": field_x + field_w, "top": field_y },
        "promptLines": prompt.lines().count(),
        "lineHeight": prompt_font().line_px(),
        "padding": PROMPT_EDITOR_PADDING,
    }))
}

fn probe(desktop: PathBuf, compact: PathBuf, db_path: PathBuf) -> Result<serde_json::Value, String> {
    let registry = magicklala_nodes::default_registry();
    let registry_kinds: Vec<String> = registry.specs().into_iter().map(|spec| spec.kind).collect();
    let registry_categories = palette_categories(&registry_specs());
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

    rt.update(|app, _| app.canvas.mode = magicklala_canvas::WorkbenchMode::Grimoire);
    rt.frame(0.0);
    let dragged = "search";
    let before = rt.state.canvas.card(&dragged.into()).map(|card| rt.state.canvas.camera.world_to_screen(card.position)).ok_or("search node missing")?;
    let pointer_delta = (64.0_f32, 36.0_f32);
    drag(&mut rt, &format!("node-{dragged}-header"), pointer_delta)?;
    let after = rt.state.canvas.card(&dragged.into()).map(|card| rt.state.canvas.camera.world_to_screen(card.position)).ok_or("search node missing")?;
    rt.update(|app, _| app.canvas.mode = magicklala_canvas::WorkbenchMode::Cauldron);
    rt.frame(0.0);

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
    settle(&mut rt)?;
    let run = run_report(&rt.state);
    let succeeded = rt.state.canvas.nodes.iter().filter(|card| card.status == ExecutionStatus::Succeeded).count();
    let active_wires = rt.state.canvas.wires.iter().filter(|wire| wire.active).count();

    let theme_before = rt.state.theme.label();
    click(&mut rt, "theme-toggle")?;
    let theme_after = rt.state.theme.label();
    if rt.state.theme != Theme::Dark || rt.state.canvas.light {
        return Err("theme toggle did not reach the canvas".to_owned());
    }

    let preserved = vec![preserve("webhook_router.json")?, preserve("parallel_research.json")?];
    let edit = edit_script()?;
    let execution = execution()?;
    let chat = chat()?;
    let published = publish(&db_path)?;

    let mut menu_rt = sized_runtime((1280.0, 800.0), Theme::Light);
    let menu = context_menu(&mut menu_rt)?;

    let toolbar: Vec<serde_json::Value> =
        [(960.0, 640.0), (1280.0, 800.0)].into_iter().flat_map(|size| [Theme::Light, Theme::Dark].map(|theme| toolbar_bounds(size, theme))).collect();

    let mut renders = Vec::new();
    for (path, size) in [(desktop, (1280.0, 800.0)), (compact, (960.0, 640.0))] {
        for theme in [Theme::Light, Theme::Dark] {
            let target = if theme == Theme::Dark { path.clone() } else { path.with_extension("light.png") };
            render_headless_snapshot_themed(&target, size, theme).map_err(|error| error.to_string())?;
            let bits = entropy(&target)?;
            if bits < 1.0 {
                return Err(format!("{} looks blank: {bits:.2} bits per byte", target.display()));
            }
            renders.push(json!({ "path": target.display().to_string(), "width": size.0, "height": size.1, "theme": theme.label(), "entropy": bits }));
        }
    }

    Ok(json!({
        "registry": { "kinds": registry_kinds, "appKinds": app_kinds, "paletteKinds": palette_kinds, "categories": registry_categories },
        "defaultSpell": { "nodes": default_nodes, "wires": default_wires, "titles": titles, "kinds": kinds, "registryErrors": registry_validation },
        "spawn": { "paletteKind": palette_kind, "nodesBefore": nodes_before, "nodesAfter": nodes_after, "spawnedKind": spawned_kind },
        "drag": { "node": dragged, "before": before, "after": after, "pointerDelta": [pointer_delta.0, pointer_delta.1] },
        "validate": { "status": validation.label() },
        "run": { "status": run["status"], "events": run["events"], "failedNodes": run["failedNodes"], "replies": run["replies"], "error": run["error"], "validation": run["validation"], "succeededNodes": succeeded, "activeWires": active_wires },
        "preserve": preserved,
        "edit": edit,
        "execution": execution,
        "chat": chat,
        "publish": published,
        "palette": palette(),
        "inspector": inspector((960.0, 640.0))?,
        "inspectorDesktop": inspector((1280.0, 800.0))?,
        "theme": { "before": theme_before, "after": theme_after },
        "toolbar": toolbar,
        "contextMenu": menu,
        "renders": renders,
    }))
}

fn studio_wire(id: &str, from: (&str, &str), to: (&str, &str)) -> Wire {
    Wire { id: WireId::from(id), from_node: NodeId::from(from.0), from_socket: from.1.to_owned(), to_node: NodeId::from(to.0), to_socket: to.1.to_owned() }
}

fn studio_node(id: &str, kind: &str, x: f32, config: serde_json::Value) -> NodeInstance {
    NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position: [x, 0.0], config }
}

fn child_spell(template: &str) -> SpellGraph {
    let mut spell = SpellGraph::new("Child Agent");
    spell.nodes = vec![
        studio_node("child_start", "event/on_start", 0.0, serde_json::Value::Null),
        studio_node("child_words", "text/template", 280.0, json!({ "template": template })),
        studio_node("child_send", "io/send_message", 560.0, serde_json::Value::Null),
    ];
    spell.wires = vec![
        studio_wire("cw_flow", ("child_start", "out"), ("child_send", "in")),
        studio_wire("cw_topic", ("child_start", "payload"), ("child_words", "topic")),
        studio_wire("cw_content", ("child_words", "text"), ("child_send", "content")),
    ];
    spell
}

fn parent_spell() -> SpellGraph {
    let mut spell = SpellGraph::new("Parent Agent");
    spell.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("words", "text/template", 560.0, json!({ "template": "parent heard {{result}}" })),
        studio_node("send", "io/send_message", 840.0, serde_json::Value::Null),
    ];
    spell.wires = vec![studio_wire("w_content", ("words", "text"), ("send", "content"))];
    spell
}

fn connect(rt: &mut Runtime<StudioApp>, from: (&str, &str), to: (&str, &str)) -> Result<(), String> {
    let (from, to) = (SocketRef::new(from.0, from.1), SocketRef::new(to.0, to.1));
    rt.state.canvas.connect_sockets(&from, &to).map(drop).map_err(|error| format!("cannot wire {from:?} to {to:?}: {error:?}"))?;
    rt.update(|app, _| app.sync_spell());
    Ok(())
}

fn validation_of(rt: &mut Runtime<StudioApp>) -> Result<serde_json::Value, String> {
    click(rt, "validate")?;
    let errors = match &rt.state.validation {
        ValidationState::Invalid(errors) => errors.clone(),
        _ => Vec::new(),
    };
    Ok(json!({ "validation": rt.state.validation.label(), "errors": errors, "subgraphs": rt.state.spell.subgraphs }))
}

fn editor_caption(rt: &Runtime<StudioApp>) -> Option<String> {
    let [_, editor_top, _, _] = rect(rt, "prompt-editor")?;
    let [panel_left, _, panel_width, _] = rect(rt, "inspector")?;
    rt.elements()
        .into_iter()
        .filter(|element| element.rect[0] >= panel_left && element.rect[0] < panel_left + panel_width && element.rect[1] + element.rect[3] <= editor_top + 0.5)
        .filter_map(|element| element.text.map(|text| (element.rect[1], text)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, text)| text)
}

fn type_prompt(rt: &mut Runtime<StudioApp>, node: &NodeId, text: &str) -> Result<(), String> {
    let (selected, typed) = (node.clone(), text.to_owned());
    rt.update(move |app, _| {
        app.canvas.select(Some(selected.clone()));
        if let Some(edit) = app.prompts.get_mut(&selected) {
            *edit = rune::TextEdit::new(&typed);
        }
    });
    rt.frame(0.0);
    let held = rt.state.prompts.get(node).map(|edit| edit.text.clone()).unwrap_or_default();
    if held == text && rt.find("prompt-field").is_some() { Ok(()) } else { Err(format!("inspector field holds {held:?} instead of {text:?}")) }
}

fn type_release_version(rt: &mut Runtime<StudioApp>, call: &NodeId, version: &str) -> Result<(), String> {
    let (selected, typed) = (call.clone(), version.to_owned());
    rt.update(move |app, _| {
        app.canvas.select(Some(selected.clone()));
        app.versions.insert(selected.clone(), rune::TextEdit::new(&typed));
    });
    rt.frame(0.0);
    let held = rt.state.versions.get(call).map(|edit| edit.text.clone()).unwrap_or_default();
    if held == version && rt.find("release-version-field").is_some() {
        Ok(())
    } else {
        Err(format!("release version field holds {held:?} instead of {version:?}"))
    }
}

fn caption_above(rt: &Runtime<StudioApp>, key: &str) -> Option<String> {
    let [_, field_top, _, _] = rect(rt, key)?;
    let [panel_left, _, panel_width, _] = rect(rt, "inspector")?;
    rt.elements()
        .into_iter()
        .filter(|element| element.rect[0] >= panel_left && element.rect[0] < panel_left + panel_width && element.rect[1] + element.rect[3] <= field_top + 0.5)
        .filter_map(|element| element.text.map(|text| (element.rect[1], text)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, text)| text)
}

fn run_once(rt: &mut Runtime<StudioApp>) -> Result<serde_json::Value, String> {
    click(rt, "run-spell")?;
    settle(rt)?;
    Ok(run_report(&rt.state))
}

fn published_then_saved_child(db_path: &Path) -> Result<(SpellGraph, serde_json::Value), String> {
    let mut app = StudioApp::with_spell(child_spell("v1 saw {{topic}}"), registry_specs());
    app.store = Some(StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?);
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "publish")?;
    let published = rt.state.releases.last().map(|release| release.version);
    type_prompt(&mut rt, &NodeId::from("child_words"), "draft saw {{topic}}")?;
    let save_button = rt.find("save").is_some();
    click(&mut rt, "save")?;
    let report = json!({
        "published": published,
        "saveButton": save_button,
        "status": rt.state.draft_status,
        "releases": rt.state.releases.iter().map(|release| release.version).collect::<Vec<_>>(),
        "storeError": rt.state.store_error,
    });
    Ok((rt.state.spell.clone(), report))
}

fn subgraph(db_path: &Path) -> Result<serde_json::Value, String> {
    let (child, child_save) = published_then_saved_child(db_path)?;
    let mut app = StudioApp::with_spell(parent_spell(), registry_specs());
    app.store = Some(StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?);
    let content = app.run_payload["content"].as_str().unwrap_or_default().to_owned();
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "palette-flow-subgraph")?;
    let call = rt
        .state
        .canvas
        .nodes
        .last()
        .filter(|card| card.kind == "flow/subgraph")
        .map(|card| card.id.clone())
        .ok_or("palette did not add a flow/subgraph node")?;
    let selected = call.clone();
    rt.update(move |app, _| app.canvas.select(Some(selected)));
    rt.frame(0.0);
    let placed = json!({
        "kind": "flow/subgraph",
        "editor": rt.find("prompt-field").is_some(),
        "caption": editor_caption(&rt),
        "versionField": rt.find("release-version-field").is_some(),
        "versionCaption": caption_above(&rt, "release-version-field"),
    });
    let child_id = child.id.0.to_string();
    type_prompt(&mut rt, &call, &child_id)?;
    let call_name = call.0.clone();
    connect(&mut rt, ("trigger", "out"), (&call_name, "in"))?;
    connect(&mut rt, ("trigger", "content"), (&call_name, "payload"))?;
    connect(&mut rt, (&call_name, "out"), ("send", "in"))?;
    if let Some(words) = rt.state.canvas.card_mut(&NodeId::from("words")) {
        words.ensure_socket("result", SocketType::Any, false);
    }
    connect(&mut rt, (&call_name, "result"), ("words", "result"))?;
    let mut pointed = validation_of(&mut rt)?;
    pointed["config"] = rt.state.spell.node(&call).map(|node| node.config.clone()).unwrap_or_default();
    let run = run_once(&mut rt)?;
    type_release_version(&mut rt, &call, "1")?;
    let mut pinned = validation_of(&mut rt)?;
    pinned["config"] = rt.state.spell.node(&call).map(|node| node.config.clone()).unwrap_or_default();
    pinned["run"] = run_once(&mut rt)?;
    type_release_version(&mut rt, &call, "zero")?;
    let bad_version = validation_of(&mut rt)?;
    type_release_version(&mut rt, &call, "")?;
    let unpinned = validation_of(&mut rt)?;
    let missing_id = SpellGraph::new("never published").id.0.to_string();
    type_prompt(&mut rt, &call, &missing_id)?;
    let mut missing = validation_of(&mut rt)?;
    missing["spellId"] = json!(missing_id);
    let doomed = call.clone();
    rt.update(move |app, _| {
        app.canvas.select(Some(doomed));
        app.delete_selected_node();
    });
    rt.frame(0.0);
    let mut deleted = validation_of(&mut rt)?;
    deleted["nodes"] = json!(rt.state.spell.nodes.iter().map(|node| node.id.0.clone()).collect::<Vec<_>>());
    Ok(json!({
        "content": content,
        "childSpellId": child_id,
        "childSave": child_save,
        "callNode": call.0,
        "placed": placed,
        "pointed": pointed,
        "run": run,
        "pinned": pinned,
        "badVersion": bad_version,
        "unpinned": unpinned,
        "missing": missing,
        "deleted": deleted,
    }))
}

fn api_shaped_parent(child: SpellId) -> SpellGraph {
    let mut spell = SpellGraph::new("API Parent");
    spell.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("call", "flow/subgraph", 280.0, json!({})),
        studio_node("words", "text/template", 560.0, json!({ "template": "parent heard {{result}}" })),
        studio_node("send", "io/send_message", 840.0, serde_json::Value::Null),
    ];
    spell.wires = vec![
        studio_wire("w_call", ("trigger", "out"), ("call", "in")),
        studio_wire("w_payload", ("trigger", "content"), ("call", "payload")),
        studio_wire("w_send", ("call", "out"), ("send", "in")),
        studio_wire("w_result", ("call", "result"), ("words", "result")),
        studio_wire("w_content", ("words", "text"), ("send", "content")),
    ];
    spell.subgraphs = vec![magicklala_graph::SubgraphRef { node_id: NodeId::from("call"), spell_id: child, release_version: Some(1) }];
    spell
}

fn set_child_template(child: &mut SpellGraph, template: &str) {
    if let Some(words) = child.nodes.iter_mut().find(|node| node.id.0 == "child_words") {
        words.config = json!({ "template": template });
    }
}

fn edited_run(rt: &mut Runtime<StudioApp>, call: &NodeId, version: &str) -> Result<serde_json::Value, String> {
    type_release_version(rt, call, version)?;
    let mut edited = validation_of(rt)?;
    edited["run"] = run_once(rt)?;
    Ok(edited)
}

fn float_pinned_run(db_path: &Path, child: SpellId) -> Result<serde_json::Value, String> {
    let mut parent = api_shaped_parent(child);
    if let Some(call) = parent.nodes.iter_mut().find(|node| node.id.0 == "call") {
        call.config = json!({ "spell_id": child.0.to_string(), "release_version": 1.0 });
    }
    parent.subgraphs.clear();
    let mut app = StudioApp::with_spell(parent, registry_specs());
    app.store = Some(StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?);
    let call = NodeId::from("call");
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let version_text = rt.state.versions.get(&call).map(|edit| edit.text.clone()).unwrap_or_default();
    let mut pinned = validation_of(&mut rt)?;
    pinned["config"] = rt.state.configs.get(&call).cloned().unwrap_or_default();
    pinned["versionText"] = json!(version_text);
    pinned["run"] = run_once(&mut rt)?;
    Ok(pinned)
}

fn subgraph_ref_edit(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut child = child_spell("v1 saw {{topic}}");
    let mut child_store = StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?;
    child_store.publish(&child).map_err(|error| error.to_string())?;
    set_child_template(&mut child, "v2 saw {{topic}}");
    child_store.publish(&child).map_err(|error| error.to_string())?;
    set_child_template(&mut child, "draft saw {{topic}}");
    child_store.save_draft(&child).map_err(|error| error.to_string())?;
    let mut app = StudioApp::with_spell(api_shaped_parent(child.id), registry_specs());
    app.store = Some(StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?);
    let content = app.run_payload["content"].as_str().unwrap_or_default().to_owned();
    let call = NodeId::from("call");
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let selected = call.clone();
    rt.update(move |app, _| app.canvas.select(Some(selected)));
    rt.frame(0.0);
    let version_text = rt.state.versions.get(&call).map(|edit| edit.text.clone()).unwrap_or_default();
    let mut saved = validation_of(&mut rt)?;
    saved["versionText"] = json!(version_text);
    saved["config"] = rt.state.configs.get(&call).cloned().unwrap_or_default();
    saved["run"] = run_once(&mut rt)?;
    let second = edited_run(&mut rt, &call, "2")?;
    type_release_version(&mut rt, &call, "zero")?;
    let bad_version = validation_of(&mut rt)?;
    let cleared = edited_run(&mut rt, &call, "")?;
    let float_pinned = float_pinned_run(db_path, child.id)?;
    Ok(json!({
        "content": content,
        "childSpellId": child.id.0.to_string(),
        "callNode": call.0,
        "saved": saved,
        "second": second,
        "badVersion": bad_version,
        "cleared": cleared,
        "floatPinned": float_pinned,
    }))
}
fn studio_at(db_path: &Path) -> Result<Runtime<StudioApp>, String> {
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut rt = Runtime::new(StudioApp::open(&config).map_err(|error| error.to_string())?, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    Ok(rt)
}

fn prompt_of(app: &StudioApp, node: &NodeId) -> serde_json::Value {
    app.spell.node(node).and_then(|node| node.config.get("prompt").cloned()).unwrap_or_default()
}

fn release_versions(app: &StudioApp) -> Vec<u32> {
    app.releases.iter().map(|release| release.version).collect()
}

fn saved(rt: &mut Runtime<StudioApp>) -> Result<serde_json::Value, String> {
    click(rt, "save")?;
    Ok(json!({ "status": rt.state.draft_status, "storeError": rt.state.store_error }))
}

fn reopened(app: &StudioApp, node: &NodeId) -> serde_json::Value {
    json!({
        "prompt": prompt_of(app, node),
        "releases": release_versions(app),
        "activeRelease": app.root_subgraph().1,
        "recordId": app.spell_record_id(),
    })
}

fn draft_reopen(db_path: &Path) -> Result<serde_json::Value, String> {
    let node = NodeId::from("llm");
    let typed = ["first saved draft for {{content}}", "draft saved after release 1 for {{content}}", "second edit after release 1 for {{content}}"];
    let mut rt = studio_at(db_path)?;
    type_prompt(&mut rt, &node, typed[0])?;
    let first_save = saved(&mut rt)?;
    drop(rt);
    let mut rt = studio_at(db_path)?;
    let first_reopen = reopened(&rt.state, &node);
    click(&mut rt, "publish")?;
    let published = json!({ "version": rt.state.last_published.as_ref().map(|release| release.version), "storeError": rt.state.store_error });
    type_prompt(&mut rt, &node, typed[1])?;
    let saved_after_release = saved(&mut rt)?;
    type_prompt(&mut rt, &node, typed[2])?;
    rt.update(|app, _| app.sync_spell());
    rt.frame(0.0);
    let edited = json!({ "prompt": prompt_of(&rt.state, &node), "status": rt.state.draft_status });
    let saved_again = saved(&mut rt)?;
    drop(rt);
    let mut rt = studio_at(db_path)?;
    let second_reopen = reopened(&rt.state, &node);
    let mut resaved = saved(&mut rt)?;
    resaved["prompt"] = prompt_of(&rt.state, &node);
    Ok(json!({
        "node": node.0,
        "typed": { "first": typed[0], "between": typed[1], "afterRelease": typed[2] },
        "firstSave": first_save,
        "firstReopen": first_reopen,
        "published": published,
        "savedAfterRelease": saved_after_release,
        "editedAfterSave": edited,
        "savedAgain": saved_again,
        "secondReopen": second_reopen,
        "resaved": resaved,
    }))
}

fn save_guards(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut memory =
        Runtime::new(StudioApp::open(&StudioConfig::default()).map_err(|error| error.to_string())?, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    memory.frame(0.0);
    click(&mut memory, "save")?;
    let (status, save_error) = (memory.state.draft_status.clone(), memory.state.store_error.clone());
    click(&mut memory, "publish")?;
    let no_db = json!({ "status": status, "saveError": save_error, "publishError": memory.state.store_error, "releases": release_versions(&memory.state) });
    let mut app = StudioApp::with_spell(api_shaped_parent(SpellGraph::new("unsaved child").id), registry_specs());
    app.store = Some(StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?);
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let pinned = saved(&mut rt)?;
    type_release_version(&mut rt, &NodeId::from("call"), "zero")?;
    let zero = saved(&mut rt)?;
    Ok(json!({ "noDb": no_db, "pinned": pinned, "zero": zero }))
}

fn calling_spell(name: &str, trigger: &str, callee: SpellId) -> SpellGraph {
    let mut spell = SpellGraph::new(name);
    spell.nodes = vec![studio_node("trigger", trigger, 0.0, serde_json::Value::Null), studio_node("call", "flow/subgraph", 280.0, json!({}))];
    spell.wires = vec![studio_wire("w_call", ("trigger", "out"), ("call", "in"))];
    spell.subgraphs = vec![magicklala_graph::SubgraphRef { node_id: NodeId::from("call"), spell_id: callee, release_version: None }];
    spell
}

fn release_identity(db_path: &Path) -> Result<serde_json::Value, String> {
    let a_draft_reply = "from A draft";
    let mut a_draft = SpellGraph::new("Release A");
    a_draft.nodes = vec![
        studio_node("start", "event/on_start", 0.0, serde_json::Value::Null),
        studio_node("send", "io/send_message", 280.0, json!({ "content": a_draft_reply })),
    ];
    a_draft.wires = vec![studio_wire("w_flow", ("start", "out"), ("send", "in"))];
    let b_draft = calling_spell("Release B", "event/on_start", a_draft.id);
    let mut a_release = calling_spell("Release A", "event/on_message", b_draft.id);
    a_release.id = a_draft.id;
    StudioStore::open(Some(db_path)).and_then(|mut store| store.save_draft(&b_draft)).map_err(|error| error.to_string())?;
    let mut a_store = StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?;
    a_store.save_draft(&a_draft).map_err(|error| error.to_string())?;
    a_store.publish(&a_release).map_err(|error| error.to_string())?;
    drop(a_store);
    let mut rt = studio_at(db_path)?;
    let opened = json!({
        "spellId": rt.state.spell.id,
        "recordId": rt.state.spell_record_id(),
        "activeRelease": rt.state.root_subgraph().1,
        "releases": release_versions(&rt.state),
    });
    let checked = validation_of(&mut rt)?;
    let validation = json!({ "validation": checked["validation"], "errors": checked["errors"] });
    let run = run_once(&mut rt)?;
    click(&mut rt, "publish")?;
    let published = json!({
        "version": rt.state.last_published.as_ref().map(|release| release.version),
        "storeError": rt.state.store_error,
        "activeRelease": rt.state.root_subgraph().1,
        "releases": release_versions(&rt.state),
    });
    Ok(json!({ "aSpellId": a_draft.id, "aDraftReply": a_draft_reply, "opened": opened, "validation": validation, "run": run, "published": published }))
}
fn trace_entry(event: &magicklala_graph::TraceEvent) -> Option<serde_json::Value> {
    use magicklala_graph::TraceEvent;
    let (kind, node) = match event {
        TraceEvent::NodeStarted { node_id, .. } => ("NodeStarted", node_id),
        TraceEvent::NodeFinished { node_id, .. } => ("NodeFinished", node_id),
        TraceEvent::NodeFailed { node_id, .. } => ("NodeFailed", node_id),
        _ => return None,
    };
    let path: Vec<&str> = event.subgraph_path().iter().map(|step| step.0.as_str()).collect();
    Some(json!({ "kind": kind, "path": path, "node": node.0 }))
}

fn subgraph_collision(db_path: &Path) -> Result<serde_json::Value, String> {
    let child_reply = "child reply";
    let mut child = SpellGraph::new("Collision Child");
    child.nodes = vec![
        studio_node("start", "event/on_start", 0.0, serde_json::Value::Null),
        studio_node("send", "io/send_message", 280.0, json!({ "content": child_reply })),
    ];
    child.wires = vec![studio_wire("w_flow", ("start", "out"), ("send", "in"))];
    StudioStore::open(Some(db_path)).and_then(|mut store| store.publish(&child)).map_err(|error| error.to_string())?;
    let mut parent = SpellGraph::new("Collision Parent");
    parent.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("gate", "flow/branch", 280.0, json!({ "condition": true })),
        studio_node("call", "flow/subgraph", 560.0, json!({ "spell_id": child.id.0.to_string() })),
        studio_node("call/send", "io/send_message", 840.0, json!({ "content": "root node on the branch not taken" })),
    ];
    parent.wires = vec![
        studio_wire("w_gate", ("trigger", "out"), ("gate", "in")),
        studio_wire("w_true", ("gate", "true"), ("call", "in")),
        studio_wire("w_false", ("gate", "false"), ("call/send", "in")),
    ];
    parent.subgraphs = vec![magicklala_graph::SubgraphRef { node_id: NodeId::from("call"), spell_id: child.id, release_version: None }];
    let mut app = StudioApp::with_spell(parent, registry_specs());
    app.store = Some(StudioStore::open(Some(db_path)).map_err(|error| error.to_string())?);
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let run = run_once(&mut rt)?;
    let statuses: serde_json::Map<String, serde_json::Value> =
        rt.state.canvas.nodes.iter().map(|card| (card.id.0.clone(), json!(format!("{:?}", card.status)))).collect();
    let trace: Vec<serde_json::Value> = rt.state.trace_events.iter().filter_map(trace_entry).collect();
    Ok(json!({ "childReply": child_reply, "run": run, "statuses": statuses, "trace": trace }))
}

fn shown_status(rt: &Runtime<StudioApp>) -> serde_json::Value {
    let Some(status) = rt.elements().into_iter().find(|element| element.key.as_deref() == Some("studio-status") && !element.exiting) else {
        return serde_json::Value::Null;
    };
    let [x, y, w, h] = status.rect;
    let within = |[left, top, width, height]: [f32; 4]| x >= left - 0.5 && y >= top - 0.5 && x + w <= left + width + 0.5 && y + h <= top + height + 0.5;
    json!({
        "text": status.text,
        "insideTrace": rect(rt, "trace-strip").is_some_and(within),
        "onScreen": w > 0.0 && h > 0.0 && rect(rt, "studio").is_some_and(within),
    })
}

fn status_view(rt: &Runtime<StudioApp>) -> serde_json::Value {
    json!({
        "leftTab": format!("{:?}", rt.state.left_tab),
        "status": rt.state.draft_status,
        "storeError": rt.state.store_error,
        "shown": shown_status(rt),
    })
}

fn headless(app: StudioApp) -> Runtime<StudioApp> {
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn memory_db(forms: &[String]) -> Result<serde_json::Value, String> {
    let mut reports = Vec::new();
    for form in forms {
        let config = StudioConfig { db_path: Some(PathBuf::from(form)), ..StudioConfig::default() };
        let mut rt = headless(StudioApp::open(&config).map_err(|error| format!("{form}: {error}"))?);
        let persistent = rt.state.store.as_ref().map(StudioStore::is_persistent);
        let edition = format!("edition {} for {{{{content}}}}", rt.state.releases.len() + 1);
        type_prompt(&mut rt, &NodeId::from("llm"), &edition)?;
        rt.update(|app, _| app.sync_spell());
        click(&mut rt, "save")?;
        let (status, save_error) = (rt.state.draft_status.clone(), rt.state.store_error.clone());
        click(&mut rt, "publish")?;
        reports.push(json!({
            "db": form,
            "persistent": persistent,
            "status": status,
            "saveError": save_error,
            "publishError": rt.state.store_error,
            "releases": release_versions(&rt.state),
        }));
    }
    Ok(json!({ "forms": reports }))
}

fn visible_status(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut memory = headless(StudioApp::open(&StudioConfig::default()).map_err(|error| error.to_string())?);
    click(&mut memory, "save")?;
    let no_db = status_view(&memory);
    let mut rt = studio_at(db_path)?;
    click(&mut rt, "save")?;
    let saved = status_view(&rt);
    click(&mut rt, "publish")?;
    let mut published = status_view(&rt);
    published["version"] = json!(rt.state.last_published.as_ref().map(|release| release.version));
    click(&mut rt, "save")?;
    let mut resaved = status_view(&rt);
    resaved["live"] = json!(rt.state.active_release_version);
    let _ = click(&mut rt, "publish");
    resaved["releasesAfterPublish"] = json!(release_versions(&rt.state));
    type_prompt(&mut rt, &NodeId::from("llm"), "edited after the save for {{content}}")?;
    rt.update(|app, _| app.sync_spell());
    rt.frame(0.0);
    let edited = status_view(&rt);
    Ok(json!({ "noDb": no_db, "saved": saved, "published": published, "resaved": resaved, "edited": edited }))
}

fn unopenable_db(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut rt = headless(StudioApp::launch(&StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() }));
    let persistent = rt.state.store.as_ref().map(StudioStore::is_persistent);
    click(&mut rt, "save")?;
    let (status, save_error) = (rt.state.draft_status.clone(), rt.state.store_error.clone());
    click(&mut rt, "publish")?;
    Ok(json!({ "persistent": persistent, "status": status, "saveError": save_error, "publishError": rt.state.store_error, "shown": shown_status(&rt) }))
}

fn reply_spell(name: &str, id: SpellId, reply: &str) -> SpellGraph {
    let mut spell = SpellGraph::new(name);
    spell.id = id;
    spell.nodes =
        vec![studio_node("start", "event/on_start", 0.0, serde_json::Value::Null), studio_node("send", "io/send_message", 280.0, json!({ "content": reply }))];
    spell.wires = vec![studio_wire("w_flow", ("start", "out"), ("send", "in"))];
    spell
}

fn project_scope(db_path: &Path) -> Result<serde_json::Value, String> {
    let (one_reply, two_reply) = ("from project 1", "from project 2");
    let shared = SpellGraph::new("shared child").id;
    let parent = calling_spell("parent two", "event/on_message", shared);
    let url = format!("sqlite://{}", db_path.display());
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    runtime
        .block_on(async {
            let store = SqliteStore::connect(&url).await?;
            store.migrate().await?;
            let project = |name: &str| NewProject { name: name.to_owned(), description: String::new() };
            let one = store.create_project(project("Project 1")).await?;
            let two = store.create_project(project("Project 2")).await?;
            let spell = |project_id, name: &str, graph: SpellGraph| NewSpell { project_id, name: name.to_owned(), graph };
            store.create_spell(spell(one.id, "child one", reply_spell("child one", shared, one_reply))).await?;
            store.create_spell(spell(two.id, "child two", reply_spell("child two", shared, two_reply))).await?;
            let record = store.create_spell(spell(two.id, "parent two", parent.clone())).await?;
            store.save_spell_graph(record.id, parent.clone()).await?;
            Ok::<_, StoreError>(())
        })
        .map_err(|error| error.to_string())?;
    drop(runtime);
    let mut rt = studio_at(db_path)?;
    let opened = json!({ "recordId": rt.state.spell_record_id(), "spellId": rt.state.spell.id });
    let checked = validation_of(&mut rt)?;
    let validation = json!({ "validation": checked["validation"], "errors": checked["errors"] });
    let run = run_once(&mut rt)?;
    Ok(json!({ "sharedId": shared, "replies": { "one": one_reply, "two": two_reply }, "opened": opened, "validation": validation, "run": run }))
}

fn out_of_band_release(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut a = SpellGraph::new("Release A");
    a.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("send", "io/send_message", 280.0, json!({ "content": "from A" })),
    ];
    a.wires = vec![studio_wire("w_flow", ("trigger", "out"), ("send", "in"))];
    StudioStore::open(Some(db_path)).and_then(|mut store| store.publish(&a)).map_err(|error| error.to_string())?;
    let mut rt = studio_at(db_path)?;
    let record = rt.state.spell_record_id().ok_or("Studio did not open the A record")?;
    let opened = json!({ "recordId": record, "activeRelease": rt.state.root_subgraph().1, "releases": release_versions(&rt.state) });
    let outside = StudioStore::open(Some(db_path)).and_then(|mut store| store.publish(&a)).map_err(|error| error.to_string())?;
    click(&mut rt, "palette-flow-subgraph")?;
    let call = rt
        .state
        .canvas
        .nodes
        .last()
        .filter(|card| card.kind == "flow/subgraph")
        .map(|card| card.id.clone())
        .ok_or("palette did not add a flow/subgraph node")?;
    type_prompt(&mut rt, &call, &record.0.to_string())?;
    type_release_version(&mut rt, &call, &outside.version.to_string())?;
    connect(&mut rt, ("trigger", "out"), (&call.0, "in"))?;
    let edited = json!({ "subgraphs": rt.state.spell.subgraphs });
    click(&mut rt, "publish")?;
    let published = json!({
        "version": rt.state.last_published.as_ref().map(|release| release.version),
        "storeError": rt.state.store_error,
        "validation": rt.state.validation.label(),
        "releases": release_versions(&rt.state),
    });
    Ok(json!({ "recordId": record, "callNode": call.0, "opened": opened, "outside": { "version": outside.version }, "edited": edited, "published": published }))
}
fn added_subgraph_call(rt: &mut Runtime<StudioApp>, callee: &str) -> Result<NodeId, String> {
    click(rt, "palette-flow-subgraph")?;
    let call = rt
        .state
        .canvas
        .nodes
        .last()
        .filter(|card| card.kind == "flow/subgraph")
        .map(|card| card.id.clone())
        .ok_or("palette did not add a flow/subgraph node")?;
    type_prompt(rt, &call, callee)?;
    connect(rt, ("trigger", "out"), (&call.0, "in"))?;
    Ok(call)
}

fn foreign_child(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut rt = studio_at(db_path)?;
    let opened_project = rt.state.store.as_ref().and_then(StudioStore::project_id);
    let url = format!("sqlite://{}", db_path.display());
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    let child = runtime
        .block_on(async {
            let store = SqliteStore::connect(&url).await?;
            store.migrate().await?;
            let other = store.create_project(NewProject { name: "Other".to_owned(), description: String::new() }).await?;
            let graph = reply_spell("foreign child", SpellGraph::new("foreign child").id, "from Other");
            store.create_spell(NewSpell { project_id: other.id, name: "foreign child".to_owned(), graph }).await
        })
        .map_err(|error| error.to_string())?;
    drop(runtime);
    let call = added_subgraph_call(&mut rt, &child.id.0.to_string())?;
    let edited = json!({ "subgraphs": rt.state.spell.subgraphs });
    let checked = validation_of(&mut rt)?;
    let validation = json!({ "validation": checked["validation"], "errors": checked["errors"] });
    click(&mut rt, "publish")?;
    let published = json!({
        "version": rt.state.last_published.as_ref().map(|release| release.version),
        "storeError": rt.state.store_error,
        "releases": release_versions(&rt.state),
    });
    Ok(json!({
        "openedProject": opened_project,
        "childRecordId": child.id,
        "callNode": call.0,
        "edited": edited,
        "validation": validation,
        "published": published,
    }))
}

fn edited_llm(rt: &mut Runtime<StudioApp>, prompt: &str) -> Result<serde_json::Value, String> {
    type_prompt(rt, &NodeId::from("llm"), prompt)?;
    rt.update(|app, _| app.sync_spell());
    rt.frame(0.0);
    let mut view = status_view(rt);
    view["openError"] = json!(rt.state.open_error);
    Ok(view)
}

fn edit_clears_store_error(db_path: &Path, unopenable: &Path) -> Result<serde_json::Value, String> {
    let mut rt = studio_at(db_path)?;
    added_subgraph_call(&mut rt, &SpellGraph::new("missing child").id.0.to_string())?;
    click(&mut rt, "publish")?;
    let refused = status_view(&rt);
    let refused_edited = edited_llm(&mut rt, "edited after the refused publish for {{content}}")?;
    let mut blocked = headless(StudioApp::launch(&StudioConfig { db_path: Some(unopenable.to_path_buf()), ..StudioConfig::default() }));
    click(&mut blocked, "save")?;
    let unopenable_saved = status_view(&blocked);
    let unopenable_edited = edited_llm(&mut blocked, "edited after the refused save for {{content}}")?;
    click(&mut blocked, "save")?;
    let unopenable_saved_again = status_view(&blocked);
    Ok(json!({
        "refused": refused,
        "refusedEdited": refused_edited,
        "unopenable": unopenable_saved,
        "unopenableEdited": unopenable_edited,
        "unopenableSavedAgain": unopenable_saved_again,
    }))
}
fn capture_surfaces(out_dir: &Path) -> Result<serde_json::Value, String> {
    std::fs::create_dir_all(out_dir).map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    let mut save = |name: &str, app: StudioApp| -> Result<(), String> {
        let target = out_dir.join(name);
        render_headless_snapshot_state(&target, app).map_err(|error| error.to_string())?;
        files.push(target.display().to_string());
        Ok(())
    };

    let base_desktop = || {
        let mut app = StudioApp::from_registry();
        app.size = (1280.0, 800.0);
        let viewport = app.canvas_size();
        app.canvas.fit(viewport);
        app
    };

    save("01-canvas-light-1280x800.png", base_desktop())?;

    let mut dark = base_desktop();
    dark.toggle_theme();
    save("02-canvas-dark-1280x800.png", dark)?;

    let mut compact = StudioApp::from_registry();
    compact.size = (960.0, 640.0);
    let viewport = compact.canvas_size();
    compact.canvas.fit(viewport);
    save("03-canvas-compact-960x640.png", compact)?;

    let mut agents = base_desktop();
    agents.sync_spell();
    agents.left_tab = LeftTab::Agents;
    agents.validation = ValidationState::Valid;
    agents.canvas.validated = true;
    agents.releases = vec![Release { version: 1, nodes: 5 }, Release { version: 2, nodes: 5 }];
    agents.release_graphs.insert(1, agents.spell.clone());
    agents.release_graphs.insert(2, agents.spell.clone());
    agents.active_release_version = Some(2);
    agents.last_published = serde_json::from_value(json!({
        "id": "00000000-0000-0000-0000-000000000002",
        "spell_id": "00000000-0000-0000-0000-000000000001",
        "version": 2,
        "notes": "",
        "graph": agents.brew_graph(),
        "content_hash": "",
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .ok();
    agents.canvas.release_badge = Some("v2".to_owned());
    agents.canvas.brew_summary = Some("Published v2 · 4 ingredients".to_owned());
    save("04-agents-releases-1280x800.png", agents)?;

    let mut agents_empty = base_desktop();
    agents_empty.left_tab = LeftTab::Agents;
    save("05-agents-empty-1280x800.png", agents_empty)?;

    let mut vars = base_desktop();
    vars.right_tab = RightTab::Variables;
    save("06-variables-1280x800.png", vars)?;

    let tasted = |dark: bool, messages: &[&str]| -> Result<StudioApp, String> {
        let mut showcase = tasting_spell();
        if let Some(node) = showcase.nodes.iter_mut().find(|node| node.id.0 == "echo") {
            node.config = json!({ "template": "Noted: {{content}}" });
        }
        let mut rt = runtime_for(showcase);
        if dark {
            rt.update(|app, _| app.toggle_theme());
        }
        for message in messages {
            type_chat(&mut rt, message)?;
        }
        rt.update(|app, _| app.fit_view());
        rt.frame(0.0);
        Ok(std::mem::replace(&mut rt.state, StudioApp::from_registry()))
    };
    save("07-playground-1280x800.png", tasted(false, &["What did Finance ask for Q3?", "Who signs off the Q3 budget?"])?)?;

    let mut executed = StudioApp::with_spell(example("webhook_router.json")?, registry_specs());
    executed.size = (1280.0, 800.0);
    executed.canvas.mode = magicklala_canvas::WorkbenchMode::Grimoire;
    executed.right_tab = RightTab::Trace;
    executed.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } });
    executed.validate();
    executed.run_spell();
    executed.wait_background(SETTLE);
    let webhook_trace = executed.trace.clone();
    let webhook_runs = executed.runs;
    let webhook_validation = executed.validation.clone();
    let webhook_summary = executed.canvas.brew_summary.clone();
    let webhook_validated = executed.canvas.validated;
    let webhook_nodes: Vec<(NodeId, ExecutionStatus, Option<u64>, Option<usize>)> =
        executed.canvas.nodes.iter().map(|card| (card.id.clone(), card.status, card.duration_ms, card.step_order)).collect();
    let webhook_wires: Vec<(WireId, bool)> = executed.canvas.wires.iter().map(|wire| (wire.id.clone(), wire.active)).collect();
    let viewport = executed.canvas_size();
    executed.canvas.fit(viewport);
    save("08-execution-trace-1280x800.png", executed)?;

    let mut menu = base_desktop();
    let picked = menu.canvas.selection.clone();
    let menu_plan = magicklala_canvas::brew_plan(&menu.canvas, menu.canvas_size());
    menu.canvas.context_card = picked.as_ref().and_then(|id| menu_plan.card(id)).map(|card| card.rect);
    menu.canvas.context_menu = Some([24.0, 456.0]);
    save("09-context-menu-1280x800.png", menu)?;

    let mut empty = StudioApp::with_spell(SpellGraph::new("New Potion"), registry_specs());
    empty.size = (1280.0, 800.0);
    empty.canvas.select(None);
    save("10-empty-canvas-1280x800.png", empty)?;

    let build_invalid = |dark: bool| -> StudioApp {
        let mut invalid = base_desktop();
        if dark {
            invalid.toggle_theme();
        }
        invalid.canvas.wires.retain(|wire| !(wire.to_node.0 == "recall" && wire.to_socket == "query"));
        invalid.sync_spell();
        invalid.canvas.select(Some(NodeId::from("recall")));
        invalid.left_tab = LeftTab::Agents;
        invalid.runs = 0;
        invalid.validate();
        let viewport = invalid.canvas_size();
        invalid.canvas.fit(viewport);
        invalid
    };
    save("11-error-state-1280x800.png", build_invalid(false))?;

    save("12-dark-secondary-1280x800.png", tasted(true, &["Summarize Q3 blockers from memory."])?)?;

    let mut filtered = base_desktop();
    filtered.palette_query = rune::TextEdit::new("search");
    save("13-palette-filter-1280x800.png", filtered)?;

    let apply_webhook_run = |app: &mut StudioApp| {
        app.trace = webhook_trace.clone();
        app.runs = webhook_runs;
        app.validation = webhook_validation.clone();
        app.canvas.brew_summary = webhook_summary.clone();
        app.canvas.validated = webhook_validated;
        for card in &mut app.canvas.nodes {
            if let Some((_, status, duration, step)) = webhook_nodes.iter().find(|(id, _, _, _)| *id == card.id) {
                card.status = *status;
                card.duration_ms = *duration;
                card.step_order = *step;
            }
        }
        for wire in &mut app.canvas.wires {
            if let Some((_, active)) = webhook_wires.iter().find(|(id, _)| *id == wire.id) {
                wire.active = *active;
            }
        }
    };

    let mut dark_grimoire = StudioApp::with_spell(example("webhook_router.json")?, registry_specs());
    dark_grimoire.size = (1280.0, 800.0);
    dark_grimoire.toggle_theme();
    dark_grimoire.canvas.mode = magicklala_canvas::WorkbenchMode::Grimoire;
    dark_grimoire.right_tab = RightTab::Trace;
    apply_webhook_run(&mut dark_grimoire);
    let viewport = dark_grimoire.canvas_size();
    dark_grimoire.canvas.fit(viewport);
    save("14-dark-grimoire-1280x800.png", dark_grimoire)?;

    save("15-dark-error-1280x800.png", build_invalid(true))?;

    let mut branch_cauldron = StudioApp::with_spell(example("webhook_router.json")?, registry_specs());
    branch_cauldron.size = (1280.0, 800.0);
    branch_cauldron.canvas.mode = magicklala_canvas::WorkbenchMode::Cauldron;
    branch_cauldron.right_tab = RightTab::Trace;
    apply_webhook_run(&mut branch_cauldron);
    let viewport = branch_cauldron.canvas_size();
    branch_cauldron.canvas.fit(viewport);
    save("16-branching-cauldron-1280x800.png", branch_cauldron)?;

    let mut crowded = StudioApp::with_spell(dozen_ingredient_spell(), registry_specs());
    crowded.size = (960.0, 640.0);
    save("17-dozen-ingredients-960x640.png", crowded)?;

    let mut aside = base_desktop();
    aside.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall")));
    aside.undo_banner = None;
    save("18-set-aside-1280x800.png", aside)?;

    let shelf = |spell: SpellGraph, size: (f32, f32), dark: bool, pantry: bool, brew: bool| -> Result<StudioApp, String> {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        if dark {
            rt.update(|app, _| app.toggle_theme());
        }
        if pantry {
            click(&mut rt, "expand-nodes")?;
        }
        if brew {
            click(&mut rt, "run-spell")?;
            live_finish(&mut rt)?;
        }
        rt.frame(3.0);
        Ok(std::mem::replace(&mut rt.state, StudioApp::from_registry()))
    };
    save("19-eight-bottles-1280x800.png", shelf(many_bottle_spell(8), (1280.0, 800.0), false, false, true)?)?;
    save("20-eight-bottles-dark-960x640.png", shelf(many_bottle_spell(8), (960.0, 640.0), true, false, false)?)?;
    save("21-crowded-kitchen-960x640.png", shelf(crowded_kitchen(), (960.0, 640.0), false, true, false)?)?;
    save("22-crowded-kitchen-dark-1280x800.png", shelf(crowded_kitchen(), (1280.0, 800.0), true, false, true)?)?;

    Ok(json!({ "surfaces": files }))
}

fn number(raw: Option<&String>, name: &str) -> Result<u64, String> {
    raw.ok_or_else(|| format!("missing {name}"))?.parse().map_err(|error| format!("{name}: {error}"))
}

fn hex_report(spell: SpellGraph) -> Result<serde_json::Value, String> {
    let mut rt = headless(StudioApp::with_spell(spell, registry_specs()));
    click(&mut rt, "validate")?;
    let flagged: Vec<serde_json::Value> = rt
        .state
        .canvas
        .nodes
        .iter()
        .filter(|card| card.status == ExecutionStatus::Failed)
        .map(|card| json!({ "node": card.id.0, "sockets": card.inputs.iter().filter(|socket| socket.flagged).map(|socket| socket.name.clone()).collect::<Vec<_>>() }))
        .collect();
    let log: Vec<String> = rt.state.trace.iter().filter(|line| line.kind == "RunRejected").map(|line| line.detail.clone()).collect();
    Ok(json!({ "validation": rt.state.validation.label(), "flagged": flagged, "plaque": rt.state.canvas.brew_summary, "log": log }))
}

fn hex_mend(spell: SpellGraph, broken: &NodeId) -> Result<serde_json::Value, String> {
    let mut rt = headless(StudioApp::with_spell(spell.clone(), registry_specs()));
    click(&mut rt, "validate")?;
    rt.update(|app, _| app.canvas.select(Some(broken.clone())));
    click_at(&mut rt, "inspector-fix", 1.0)?;
    let added: Vec<serde_json::Value> = rt
        .state
        .spell
        .wires
        .iter()
        .filter(|wire| {
            !spell.wires.iter().any(|before| {
                before.from_node == wire.from_node
                    && before.from_socket == wire.from_socket
                    && before.to_node == wire.to_node
                    && before.to_socket == wire.to_socket
            })
        })
        .map(|wire| json!({ "from_node": wire.from_node.0, "from_socket": wire.from_socket, "to_node": wire.to_node.0, "to_socket": wire.to_socket }))
        .collect();
    Ok(json!({ "validation": rt.state.validation.label(), "added": added }))
}

fn hex_pinpoint() -> Result<serde_json::Value, String> {
    let demo = StudioApp::from_registry().spell.clone();
    let mut missing_query = demo.clone();
    let cut = missing_query.wires.iter().position(|wire| wire.to_socket == "query").ok_or("demo spell has no query wire")?;
    let removed = missing_query.wires.remove(cut);
    let broken_title = registry_specs()
        .iter()
        .find(|spec| missing_query.nodes.iter().any(|node| node.id == removed.to_node && node.kind == spec.kind))
        .map(|spec| spec.title.clone());
    let mend = hex_mend(missing_query.clone(), &removed.to_node)?;
    let mut twin_ids = demo.clone();
    let reply = twin_ids.nodes.iter().find(|node| node.kind == "io/send_message").ok_or("demo spell has no reply")?.clone();
    let flow_into_reply = twin_ids.wires.iter().find(|wire| wire.to_node == reply.id && wire.to_socket == "in").ok_or("reply has no flow wire")?.clone();
    let twin = NodeId::from(format!("{}_support", reply.id.0).as_str());
    twin_ids.nodes.push(NodeInstance {
        id: twin.clone(),
        kind: reply.kind.clone(),
        position: [reply.position[0], reply.position[1] + 180.0],
        config: reply.config.clone(),
    });
    twin_ids.wires.push(Wire { id: WireId::from("twin_flow"), to_node: twin.clone(), ..flow_into_reply });
    Ok(json!({
        "missingQuery": {
            "removed": { "node": removed.to_node.0, "socket": removed.to_socket, "from_node": removed.from_node.0, "from_socket": removed.from_socket, "title": broken_title },
            "report": hex_report(missing_query)?,
            "mend": mend,
        },
        "twinIds": { "twin": twin.0, "lookalike": reply.id.0, "report": hex_report(twin_ids)? },
    }))
}

fn dozen_ingredient_spell() -> SpellGraph {
    let specs = registry_specs();
    let flow_in = |spec: &magicklala_graph::NodeSpec| spec.inputs.iter().find(|socket| socket.ty == SocketType::Flow).map(|socket| socket.name.clone());
    let flow_out = |spec: &magicklala_graph::NodeSpec| spec.outputs.iter().find(|socket| socket.ty == SocketType::Flow).map(|socket| socket.name.clone());
    let mut spell = SpellGraph::new("Dozen Ingredients");
    spell.nodes.push(NodeInstance { id: NodeId::from("start"), kind: "event/on_message".to_owned(), position: [0.0, 0.0], config: serde_json::Value::Null });
    let mut previous = (NodeId::from("start"), "out".to_owned());
    let chain: Vec<&magicklala_graph::NodeSpec> = specs
        .iter()
        .filter(|spec| !spec.is_trigger && !spec.kind.starts_with("io/") && spec.kind != "flow/subgraph" && flow_in(spec).is_some() && flow_out(spec).is_some())
        .take(11)
        .collect();
    for (index, spec) in chain.iter().enumerate() {
        let id = NodeId::from(format!("step_{index}").as_str());
        spell.nodes.push(NodeInstance {
            id: id.clone(),
            kind: spec.kind.clone(),
            position: [220.0 * (index + 1) as f32, 0.0],
            config: serde_json::Value::Null,
        });
        spell.wires.push(Wire {
            id: WireId::from(format!("chain_{index}").as_str()),
            from_node: previous.0.clone(),
            from_socket: previous.1.clone(),
            to_node: id.clone(),
            to_socket: flow_in(spec).unwrap_or_default(),
        });
        previous = (id, flow_out(spec).unwrap_or_default());
    }
    spell.nodes.push(NodeInstance { id: NodeId::from("finish"), kind: "io/send_message".to_owned(), position: [2800.0, 0.0], config: serde_json::Value::Null });
    spell.wires.push(Wire {
        id: WireId::from("chain_end"),
        from_node: previous.0,
        from_socket: previous.1,
        to_node: NodeId::from("finish"),
        to_socket: "in".to_owned(),
    });
    spell
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] - 0.5 && b[0] < a[0] + a[2] - 0.5 && a[1] < b[1] + b[3] - 0.5 && b[1] < a[1] + a[3] - 0.5
}

fn pot_measure(rt: &Runtime<StudioApp>, ids: &[String]) -> serde_json::Value {
    let canvas = rt.presented("canvas").map(|(rect, _)| rect);
    let placed: Vec<(String, [f32; 4])> = ids.iter().filter_map(|id| rt.presented(&format!("node-{id}")).map(|(rect, _)| (id.clone(), rect))).collect();
    let missing: Vec<&String> = ids.iter().filter(|id| !placed.iter().any(|(placed_id, _)| placed_id == *id)).collect();
    let mut overlaps = Vec::new();
    for (index, (a_id, a)) in placed.iter().enumerate() {
        for (b_id, b) in placed.iter().skip(index + 1) {
            if rects_overlap(*a, *b) {
                overlaps.push(json!([a_id, b_id]));
            }
        }
    }
    let outside: Vec<&String> = placed
        .iter()
        .filter(|(_, rect)| {
            canvas
                .is_some_and(|c| rect[0] < c[0] - 0.5 || rect[1] < c[1] - 0.5 || rect[0] + rect[2] > c[0] + c[2] + 0.5 || rect[1] + rect[3] > c[1] + c[3] + 0.5)
        })
        .map(|(id, _)| id)
        .collect();
    let elements = rt.elements();
    let roots: Vec<usize> = elements
        .iter()
        .enumerate()
        .filter(|(_, element)| element.key.as_deref().is_some_and(|key| ids.iter().any(|id| key == format!("node-{id}"))))
        .map(|(index, _)| index)
        .collect();
    let under = |mut index: usize| loop {
        if roots.contains(&index) {
            return true;
        }
        match elements[index].parent {
            Some(parent) => index = parent,
            None => return false,
        }
    };
    let smallest_line = elements
        .iter()
        .enumerate()
        .filter(|(index, element)| !element.exiting && element.text.as_deref().is_some_and(|text| !text.trim().is_empty()) && under(*index))
        .map(|(_, element)| element.rect[3])
        .fold(f32::INFINITY, f32::min);
    json!({ "zoom": rt.state.canvas.active_camera().zoom, "missing": missing, "overlaps": overlaps, "outside": outside, "smallestLine": smallest_line })
}

fn pot_layout() -> Result<serde_json::Value, String> {
    let spells = vec![
        ("support_agent", StudioApp::from_registry().spell.clone()),
        ("webhook_router", example("webhook_router.json")?),
        ("parallel_research", example("parallel_research.json")?),
        ("dozen_ingredients", dozen_ingredient_spell()),
    ];
    let mut cases = Vec::new();
    for (name, spell) in spells {
        let ids: Vec<String> = spell.nodes.iter().map(|node| node.id.0.clone()).collect();
        for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
            for pantry_open in [false, true] {
                let mut app = StudioApp::with_spell(spell.clone(), registry_specs());
                app.size = size;
                app.left_tab = LeftTab::Palette;
                app.left_rail_pinned = pantry_open;
                let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
                let mut clock = 0.0;
                let mut step = |rt: &mut Runtime<StudioApp>, key: Option<&str>| -> Result<serde_json::Value, String> {
                    clock += 0.5;
                    let at = clock;
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match key {
                        Some(key) => click_at(rt, key, at).map(|_| ()),
                        None => {
                            rt.frame(at);
                            Ok(())
                        }
                    }));
                    Ok(match outcome {
                        Ok(Ok(())) => {
                            let mut measured = pot_measure(rt, &ids);
                            measured["panicked"] = json!(false);
                            measured
                        }
                        Ok(Err(error)) => return Err(error),
                        Err(_) => json!({ "panicked": true }),
                    })
                };
                let opened = step(&mut rt, None)?;
                let mut zooms = vec![opened.clone()];
                for _ in 0..8 {
                    let shot = step(&mut rt, Some("zoom-out"))?;
                    let stop = shot["panicked"] == json!(true);
                    zooms.push(shot);
                    if stop {
                        break;
                    }
                }
                for _ in 0..12 {
                    let shot = step(&mut rt, Some("zoom-in"))?;
                    let stop = shot["panicked"] == json!(true);
                    zooms.push(shot);
                    if stop {
                        break;
                    }
                }
                cases.push(json!({ "spell": name, "size": [size.0, size.1], "pantryOpen": pantry_open, "nodes": ids, "opened": opened, "zooms": zooms }));
            }
        }
    }
    Ok(json!({ "cases": cases }))
}

fn wire_list(app: &StudioApp) -> Vec<serde_json::Value> {
    app.spell.wires.iter().map(|wire| json!([wire.from_node.0, wire.from_socket, wire.to_node.0, wire.to_socket])).collect()
}

fn spec_sockets(kind: &str) -> serde_json::Value {
    let specs = registry_specs();
    let Some(spec) = specs.iter().find(|spec| spec.kind == kind) else { return json!(null) };
    let side = |sockets: &[magicklala_graph::SocketSpec]| -> Vec<serde_json::Value> {
        sockets.iter().map(|socket| json!({ "name": socket.name, "ty": format!("{:?}", socket.ty), "required": socket.required })).collect()
    };
    json!({ "inputs": side(&spec.inputs), "outputs": side(&spec.outputs) })
}

fn node_of_kind(app: &StudioApp, kind: &str) -> Option<String> {
    app.spell.nodes.iter().rev().find(|node| node.kind == kind).map(|node| node.id.0.clone())
}

fn toss() -> Result<serde_json::Value, String> {
    let recipe = ["event/on_message", "ai/generate_text", "io/send_message"];
    let mut built = headless(StudioApp::with_spell(SpellGraph::new("Tossed"), registry_specs()));
    let mut clock = 0.0;
    for kind in recipe {
        let title = registry_specs().iter().find(|spec| spec.kind == kind).map(|spec| spec.title.clone()).ok_or("unknown kind")?;
        built.update(|app, _| app.palette_query = rune::TextEdit::new(&title));
        clock += 0.5;
        click_at(&mut built, &format!("palette-{}", kind.replace('/', "-")), clock)?;
    }
    built.update(|app, _| app.palette_query = rune::TextEdit::new(""));
    click_at(&mut built, "validate", clock + 1.0)?;
    let built_report = json!({
        "ids": recipe.iter().map(|kind| node_of_kind(&built.state, kind)).collect::<Vec<_>>(),
        "wires": wire_list(&built.state),
        "validation": built.state.validation.label(),
        "specs": recipe.iter().map(|kind| (kind.to_string(), spec_sockets(kind))).collect::<serde_json::Map<_, _>>(),
    });

    let mut rt = headless(StudioApp::from_registry());
    let positions_before: Vec<[f32; 2]> = rt.state.canvas.nodes.iter().map(|card| card.position).collect();
    let pantry_drop = pantry_toss(&mut rt, "ai/generate_text", "Generate Text")?;
    let mut full_rt = headless(StudioApp::from_registry());
    let pantry_full = pantry_toss(&mut full_rt, "var/get", "Get Variable")?;

    let mut aside_rt = headless(StudioApp::from_registry());
    let target = "recall";
    let header = format!("node-{target}-header");
    let start = aside_rt.find(&header).ok_or("no recall vial")?;
    let canvas_rect = aside_rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    let away = [canvas_rect[0] + 40.0, canvas_rect[1] + canvas_rect[3] - 30.0];
    let wires_before = aside_rt.state.spell.wires.iter().filter(|wire| wire.from_node.0 == target || wire.to_node.0 == target).count();
    drag(&mut aside_rt, &header, (away[0] - start.0, away[1] - start.1))?;
    let wires_aside = aside_rt.state.spell.wires.iter().filter(|wire| wire.from_node.0 == target || wire.to_node.0 == target).count();
    let role_aside =
        magicklala_canvas::brew_plan(&aside_rt.state.canvas, aside_rt.state.canvas_size()).card(&NodeId::from(target)).map(|card| format!("{:?}", card.role));
    let back_from = aside_rt.find(&header).ok_or("vial vanished")?;
    let pot_now = aside_rt.presented("cauldron-center").map(|(rect, _)| [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5]).ok_or("no cauldron")?;
    drag(&mut aside_rt, &header, (pot_now[0] - back_from.0, pot_now[1] - back_from.1))?;
    let wires_back = aside_rt.state.spell.wires.iter().filter(|wire| wire.from_node.0 == target || wire.to_node.0 == target).count();
    let role_back =
        magicklala_canvas::brew_plan(&aside_rt.state.canvas, aside_rt.state.canvas_size()).card(&NodeId::from(target)).map(|card| format!("{:?}", card.role));
    let positions_after: Vec<[f32; 2]> = aside_rt.state.canvas.nodes.iter().map(|card| card.position).collect();
    let set_aside = json!({
        "node": target,
        "wiresBefore": wires_before,
        "wiresAside": wires_aside,
        "roleAside": role_aside,
        "wiresBack": wires_back,
        "roleBack": role_back,
        "positionsBefore": positions_before,
        "positionsAfter": positions_after,
    });
    Ok(json!({ "built": built_report, "pantryDrop": pantry_drop, "pantryFull": pantry_full, "setAside": set_aside }))
}

fn empty_recipe() -> Result<serde_json::Value, String> {
    let mut rt = runtime_for(SpellGraph::new("Fresh Potion"));
    rt.frame(1.0);
    let texts: Vec<String> = text_lines_under(&rt, "inspector").into_iter().map(|(line, _)| line).collect();
    Ok(json!({
        "texts": texts,
        "placeholder": rt.find("inspector-empty").is_some(),
        "sections": rt.find("recipe-overview").is_some(),
    }))
}
fn list_truth() -> Result<serde_json::Value, String> {
    let mut imported = router_spell();
    if let Some(node) = imported.nodes.iter_mut().find(|node| node.id.0 == "switch") {
        node.config = json!({ "cases": ["north,west", "south"] });
    }
    let mut kept = runtime_for(imported);
    let checked = validation_of(&mut kept)?;
    let kept_cases = kept.state.spell.nodes.iter().find(|node| node.id.0 == "switch").and_then(|node| node.config.get("cases").cloned());
    let shown = kept.state.prompts.get(&NodeId::from("switch")).map(|edit| edit.text.clone());
    let kept_outputs = outputs_of(&kept.state, "switch");

    let mut captions = serde_json::Map::new();
    let mut lists = SpellGraph::new("Lists");
    lists.nodes = vec![
        studio_node("switch", "flow/switch", 0.0, serde_json::Value::Null),
        studio_node("split", "flow/split", 0.0, serde_json::Value::Null),
        studio_node("pick", "json/destructure", 0.0, serde_json::Value::Null),
    ];
    let mut captioned = runtime_for(lists);
    for id in ["switch", "split", "pick"] {
        captioned.update(move |app, _| app.canvas.select(Some(NodeId::from(id))));
        captioned.frame(0.0);
        captions.insert(id.to_owned(), json!(editor_caption(&captioned)));
    }

    let mut blank = runtime_for(router_spell());
    type_prompt(&mut blank, &NodeId::from("switch"), ",")?;
    let blank_check = validation_of(&mut blank)?;

    let mut only_split = SpellGraph::new("Split");
    only_split.nodes = vec![studio_node("fork", "flow/split", 0.0, serde_json::Value::Null)];
    let mut split = runtime_for(only_split);
    let split_id = split.state.canvas.nodes.first().map(|card| card.id.clone()).ok_or("no split")?;
    let split_default = split.state.prompts.get(&split_id).map(|edit| edit.text.clone());
    type_prompt(&mut split, &split_id, "left, right")?;
    split.update(|app, _| app.sync_spell());
    let split_outputs = outputs_of(&split.state, &split_id.0);

    let mut stale = runtime_for(router_spell());
    type_prompt(&mut stale, &NodeId::from("switch"), "refund, billing")?;
    stale.update(|app, _| app.sync_spell());
    connect(&mut stale, ("switch", "billing"), ("refund_reply", "in"))?;
    type_prompt(&mut stale, &NodeId::from("switch"), "refund")?;
    stale.update(|app, _| app.sync_spell());
    let stale_check = validation_of(&mut stale)?;

    Ok(json!({
        "kept": { "check": checked["validation"], "cases": kept_cases, "shown": shown, "outputs": kept_outputs },
        "captions": captions,
        "blank": { "check": blank_check["validation"], "errors": blank_check["errors"] },
        "split": { "default": split_default, "outputs": split_outputs },
        "stale": { "check": stale_check["validation"], "errors": stale_check["errors"], "outputs": outputs_of(&stale.state, "switch") },
    }))
}
fn check_save_truth(db_path: &Path) -> Result<serde_json::Value, String> {
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    let mut draft = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    draft.update(|app, _| {
        app.new_empty_spell();
        app.add_variable();
    });
    draft.frame(0.0);
    let name = draft.state.spell.name.clone();
    click(&mut draft, "save")?;
    let saved = draft.state.saved_potions.iter().any(|(saved, _)| saved == &name);
    draft.update(|app, _| {
        app.remove_variable_at(0);
        app.sync_spell();
        app.request_open_sample("Research Agent");
    });
    let asked = draft.state.pending_switch.is_some();

    let mut literal = SpellGraph::new("Literal");
    literal.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("echo", "text/template", 280.0, json!({ "template": "{{thing..name}} {{content}}" })),
        studio_node("reply", "io/send_message", 560.0, serde_json::Value::Null),
    ];
    literal.wires = vec![
        studio_wire("w1", ("trigger", "out"), ("reply", "in")),
        studio_wire("w2", ("trigger", "content"), ("echo", "content")),
        studio_wire("w3", ("echo", "text"), ("reply", "content")),
    ];
    let mut literal_rt = runtime_for(literal);
    let literal_check = validation_of(&mut literal_rt)?;

    let mut broken = echo_spell();
    broken.nodes[1] = studio_node("echo", "code/rhai_script", 280.0, json!({ "script": "throw \"boom\"" }));
    broken.wires[2] = studio_wire("w3", ("echo", "result"), ("reply", "content"));
    let mut failing = runtime_for(broken);
    click(&mut failing, "run-spell")?;
    live_finish(&mut failing)?;
    let before = format!("{:?}", failing.state.canvas.last_brew);
    click(&mut failing, "validate")?;
    let failed_cards = failing.state.canvas.nodes.iter().filter(|card| card.status == ExecutionStatus::Failed).count();
    Ok(json!({
        "draft": { "saved": saved, "askedAfterEmptying": asked },
        "literal": { "check": literal_check["validation"], "errors": literal_check["errors"] },
        "failedBrew": { "before": before, "after": format!("{:?}", failing.state.canvas.last_brew), "failedCards": failed_cards, "stale": failing.state.trace_stale },
    }))
}
fn one_step_undo() -> Result<serde_json::Value, String> {
    let cmd_z = |rt: &mut Runtime<StudioApp>| {
        rt.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
        rt.frame(0.0);
    };
    let mut empty = runtime_for(SpellGraph::new("Empty Pot"));
    empty.on_key = Some(magicklala_studio::studio_key);
    empty.frame(0.0);
    let tossed = pantry_toss(&mut empty, "io/send_message", "Send")?;
    let toss_entries = empty.state.undo_stack.len();
    cmd_z(&mut empty);
    let toss = json!({ "added": tossed["nodesAfter"], "entries": toss_entries, "afterUndo": empty.state.canvas.nodes.len() });

    let mut twin = runtime_for(tasting_spell());
    twin.on_key = Some(magicklala_studio::studio_key);
    twin.update(|app, _| {
        app.canvas.select(Some(NodeId::from("echo")));
        app.duplicate_selected_node();
    });
    twin.frame(0.0);
    let twin_nodes = twin.state.canvas.nodes.len();
    let twin_entries = twin.state.undo_stack.len();
    cmd_z(&mut twin);
    let duplicate = json!({ "after": twin_nodes, "entries": twin_entries, "afterUndo": twin.state.canvas.nodes.len() });

    let mut back = runtime_for(tasting_spell());
    back.on_key = Some(magicklala_studio::studio_key);
    back.update(|app, _| {
        app.canvas.select(Some(NodeId::from("echo")));
        app.set_aside_selected();
        app.sync_edits();
    });
    back.frame(0.0);
    back.update(|app, _| {
        app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("echo")));
        app.sync_edits();
    });
    back.frame(0.0);
    let poured = !back.state.canvas.is_shelved(&NodeId::from("echo"));
    cmd_z(&mut back);
    let pour = json!({ "poured": poured, "shelvedAfterUndo": back.state.canvas.is_shelved(&NodeId::from("echo")) });
    Ok(json!({ "toss": toss, "duplicate": duplicate, "pour": pour }))
}
fn wire_return() -> Result<serde_json::Value, String> {
    let records = |app: &StudioApp| {
        app.spell
            .wires
            .iter()
            .map(|wire| format!("{}:{}.{}>{}.{}", wire.id.0, wire.from_node.0, wire.from_socket, wire.to_node.0, wire.to_socket))
            .collect::<Vec<_>>()
    };
    let spell = StudioApp::from_registry().spell.clone();
    let middles: Vec<NodeId> = spell
        .nodes
        .iter()
        .filter(|node| spell.wires.iter().any(|wire| wire.to_node == node.id) && spell.wires.iter().any(|wire| wire.from_node == node.id))
        .map(|node| node.id.clone())
        .take(2)
        .collect();
    let [first, second] = <[NodeId; 2]>::try_from(middles).map_err(|_| "need two middle steps")?;
    let mut rt = runtime_for(spell);
    rt.update(|app, _| app.sync_spell());
    let original = records(&rt.state);
    for id in [&first, &second] {
        let id = id.clone();
        rt.update(move |app, _| {
            app.toss(magicklala_canvas::Toss::Aside(id.clone()));
            app.sync_edits();
        });
        rt.frame(0.0);
    }
    for id in [&first, &second] {
        let id = id.clone();
        rt.update(move |app, _| {
            app.toss(magicklala_canvas::Toss::IntoPot(id.clone()));
            app.sync_edits();
        });
        rt.frame(0.0);
    }
    let poured = records(&rt.state);

    let mut undone = runtime_for(StudioApp::from_registry().spell.clone());
    undone.on_key = Some(magicklala_studio::studio_key);
    undone.update(|app, _| app.sync_spell());
    let before_undo = records(&undone.state);
    let aside = first.clone();
    undone.update(move |app, _| {
        app.toss(magicklala_canvas::Toss::Aside(aside.clone()));
        app.sync_edits();
    });
    undone.frame(0.0);
    undone.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
    undone.frame(0.0);
    undone.update(|app, _| app.sync_spell());
    Ok(json!({
        "pouredMatches": poured == original,
        "original": original,
        "poured": poured,
        "undoMatches": records(&undone.state) == before_undo,
        "undone": records(&undone.state),
    }))
}
fn pantry_whole() -> Result<serde_json::Value, String> {
    let mut report = serde_json::Map::new();
    for size in [(960.0_f32, 640.0_f32), (1280.0, 800.0)] {
        let mut app = StudioApp::with_spell(crowded_kitchen(), registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        if rt.find("palette-event-on_variable_changed").is_none() {
            click(&mut rt, "expand-nodes")?;
        }
        rt.frame(3.0);
        rt.frame(6.0);
        let mut measure = studio_text_system();
        let mut cut: Vec<String> = Vec::new();
        let mut seen = 0;
        for spec in rt.state.specs.clone() {
            let key = format!("palette-{}", spec.kind.replace('/', "-"));
            let Some((_, rect)) = text_lines_under(&rt, &key).into_iter().find(|(line, _)| line.replace('\u{a0}', " ") == spec.title) else { continue };
            seen += 1;
            let (natural, line_h) = measure.measure(&spec.title, rune::TextStyle::new(12.0, 550.0), 1.0);
            let lines = (rect[3] / line_h).round().max(1.0);
            let row = layout_rect(&rt, &key).unwrap_or(rect);
            let room = row[0] + row[2] - 7.0 - 22.0 - 4.0;
            if natural > rect[2] * lines + 2.0 || rect[0] + rect[2] > room + 0.5 {
                cut.push(spec.title.clone());
            }
        }
        report.insert(format!("{}", size.0), json!({ "seen": seen, "cut": cut }));
    }
    Ok(serde_json::Value::Object(report))
}
fn empty_start() -> Result<serde_json::Value, String> {
    let mut report = serde_json::Map::new();
    for (name, size, potions) in [("wide-pantry", (1280.0_f32, 800.0_f32), false), ("wide-potions", (1280.0, 800.0), true), ("compact", (960.0, 640.0), false)]
    {
        let mut app = StudioApp::with_spell(SpellGraph::new("New Potion"), registry_specs());
        app.size = size;
        if potions {
            app.left_tab = LeftTab::Agents;
        }
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(1.0);
        rt.frame(3.0);
        let triggers: Vec<String> = rt.state.specs.iter().filter(|spec| spec.is_trigger).map(|spec| spec.kind.replace('/', "-")).collect();
        let starters = triggers.iter().filter(|kind| rt.find(&format!("starter-{kind}")).is_some()).count();
        let glowing = triggers.iter().filter(|kind| rt.find(&format!("pantry-glow-{kind}")).is_some()).count();
        let pantry_rows = triggers.iter().filter(|kind| rt.find(&format!("palette-{kind}")).is_some()).count();
        report.insert(
            name.to_owned(),
            json!({
                "inertHeading": all_texts(&rt).iter().any(|line| line == "Start a new potion"),
                "offered": starters.max(glowing),
                "glowing": glowing,
                "starters": starters,
                "pantryRows": pantry_rows,
            }),
        );
    }
    Ok(serde_json::Value::Object(report))
}
fn downhill() -> Result<serde_json::Value, String> {
    let scenes: Vec<(&str, SpellGraph, (f32, f32))> = vec![
        ("support@1280", StudioApp::from_registry().spell.clone(), (1280.0, 800.0)),
        ("support@960", StudioApp::from_registry().spell.clone(), (960.0, 640.0)),
        ("webhook@1280", example("webhook_router.json")?, (1280.0, 800.0)),
        ("webhook@960", example("webhook_router.json")?, (960.0, 640.0)),
        ("parallel@960", example("parallel_research.json")?, (960.0, 640.0)),
        ("two@1280", many_bottle_spell(2), (1280.0, 800.0)),
        ("three@960", many_bottle_spell(3), (960.0, 640.0)),
    ];
    let mut uphill: Vec<String> = Vec::new();
    let mut misordered: Vec<String> = Vec::new();
    let mut cut_kinds: Vec<String> = Vec::new();
    for (name, spell, size) in scenes {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(1.0);
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        let tap = magicklala_canvas::cauldron::spout_point(&plan);
        let bottles: Vec<&magicklala_canvas::BrewCard> = plan.with_role(magicklala_canvas::BrewRole::Bottle).collect();
        for card in &bottles {
            let neck = magicklala_canvas::cauldron::bottle_neck(card, bottles.len());
            if neck[1] < tap[1] - 2.0 {
                uphill.push(format!("{name} {} {:.0}", rt.state.canvas.display_title(&card.id), tap[1] - neck[1]));
            }
        }
        let mut measure = studio_text_system();
        for card in plan.with_role(magicklala_canvas::BrewRole::Ingredient) {
            let Some(model) = rt.state.canvas.card(&card.id) else { continue };
            let Some((_, rect)) = text_lines_under(&rt, &format!("node-{}", card.id.0)).into_iter().find(|(line, _)| line == &model.kind) else { continue };
            let (natural, _) = measure.measure(&model.kind, rune::TextStyle::new(11.5_f32.max(10.5 / plan.scale), 500.0).mono(), 1.0);
            if natural * plan.scale > rect[2] + 2.0 {
                cut_kinds.push(format!("{name} {}", model.kind));
            }
        }
        let mut by_height: Vec<&&magicklala_canvas::BrewCard> = bottles.iter().collect();
        by_height.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
        let mut row = 0;
        let mut row_top = by_height.first().map_or(0.0, |card| card.rect[1]);
        let mut reading: Vec<(i32, f32, NodeId)> = Vec::new();
        for card in by_height {
            if card.rect[1] - row_top > card.rect[3] * 0.5 {
                row += 1;
                row_top = card.rect[1];
            }
            reading.push((row, card.rect[0], card.id.clone()));
        }
        reading.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let read: Vec<String> = reading.iter().map(|(_, _, id)| rt.state.canvas.display_title(id)).collect();
        let listed: Vec<String> = bottles.iter().map(|card| rt.state.canvas.display_title(&card.id)).collect();
        if bottles.len() > 1 && read != listed {
            misordered.push(format!("{name}: {} vs {}", read.join(","), listed.join(",")));
        }
    }
    Ok(json!({ "uphill": uphill, "misordered": misordered, "cutKinds": cut_kinds }))
}
fn failed_stub() -> Result<serde_json::Value, String> {
    let mut upward: Vec<String> = Vec::new();
    let mut crowding: Vec<String> = Vec::new();
    for size in [(960.0_f32, 640.0_f32), (1280.0, 800.0)] {
        let mut app = StudioApp::with_spell(example("parallel_research.json")?, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        click(&mut rt, "run-spell")?;
        live_finish(&mut rt)?;
        rt.frame(2.0);
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        let paths = magicklala_canvas::cauldron::pour_paths(&rt.state.canvas, &plan);
        let failed: Vec<NodeId> = rt.state.canvas.nodes.iter().filter(|card| card.status == ExecutionStatus::Failed).map(|card| card.id.clone()).collect();
        for (id, points) in paths.iter().filter(|(id, _)| failed.contains(id)) {
            let stub = magicklala_canvas::cauldron::hexed_stub(points);
            let end = stub[stub.len() - 1];
            let before = stub[stub.len().saturating_sub(4)];
            let heading_down = stub.len() < 2 || end[1] - before[1] >= (end[0] - before[0]).abs() || stub.len() >= points.len() * 80 / 100;
            if !heading_down {
                upward.push(format!("{}@{}", id.0, size.0));
            }
            for (other, line) in paths.iter().filter(|(other, _)| other != id && !failed.contains(other)) {
                let near = line.iter().map(|point| (point[0] - end[0]).hypot(point[1] - end[1])).fold(f32::MAX, f32::min);
                if near < 8.0 * plan.scale {
                    crowding.push(format!("{}@{} near {}", id.0, size.0, other.0));
                }
            }
        }
    }
    Ok(json!({ "upward": upward, "crowding": crowding }))
}
fn bound_settings() -> Result<serde_json::Value, String> {
    use magicklala_graph::{Value, VariableDef};
    let bound_delay = |pause: &str| {
        let mut spell = delay_spell(0);
        if let Some(node) = spell.nodes.iter_mut().find(|node| node.id.0 == "wait") {
            node.config = json!({ "ms": format!("{{{{{pause}}}}}") });
        }
        spell.variables = vec![VariableDef { name: "pause".to_owned(), ty: SocketType::Int, default: Some(Value::Int(4000)), exposed: true }];
        spell
    };
    let mut quick = runtime_for(bound_delay("pause"));
    quick.update(|app, _| {
        if let Some(edit) = app.var_default_edits.first_mut() {
            *edit = rune::TextEdit::new("1800");
        }
        app.sync_spell();
    });
    let check = validation_of(&mut quick)?;
    let started = Instant::now();
    click(&mut quick, "run-spell")?;
    live_finish(&mut quick)?;
    let quick_ms = started.elapsed().as_millis() as u64;
    let quick_outcome = quick.state.last_run.as_ref().map(|report| report.outcome.label());

    let mut unknown = runtime_for(bound_delay("nope"));
    let unknown_check = validation_of(&mut unknown)?;

    let demo = runtime_for(StudioApp::from_registry().spell.clone());
    let hints: Vec<(String, String)> =
        demo.state.spell.variables.iter().map(|variable| (variable.name.clone(), demo.state.variable_binding_hint(&variable.name).0)).collect();

    let mut dial = runtime_for(StudioApp::from_registry().spell.clone());
    let llm = dial.state.canvas.nodes.iter().find(|card| card.kind == "ai/generate_text").map(|card| card.id.clone()).ok_or("no llm")?;
    let picked = llm.clone();
    dial.update(move |app, _| app.canvas.select(Some(picked.clone())));
    let shown = dial.state.selected_temperature();
    dial.update(|app, _| app.adjust_temperature(0.1));
    let after_variable = dial
        .state
        .spell
        .variables
        .iter()
        .find(|variable| variable.name == "temperature")
        .and_then(|variable| variable.default.clone())
        .map(|value| format!("{value:?}"));
    let after_config = dial.state.spell.nodes.iter().find(|node| node.id == llm).and_then(|node| node.config.get("temperature").cloned());
    Ok(json!({
        "quick": { "check": check["validation"], "outcome": quick_outcome, "waitedForVariable": quick_ms >= 1700 },
        "unknown": { "check": unknown_check["validation"], "errors": unknown_check["errors"] },
        "hints": hints,
        "dial": { "shown": shown, "variable": after_variable, "config": after_config },
    }))
}
fn twin_routes() -> Result<serde_json::Value, String> {
    let mut spell = router_spell();
    if let Some(node) = spell.nodes.iter_mut().find(|node| node.id.0 == "switch") {
        node.config = json!({ "cases": ["billing"] });
    }
    spell.nodes.push(studio_node("billing_copy", "io/send_message", 840.0, serde_json::Value::Null));
    spell.wires.push(studio_wire("w6", ("switch", "billing"), ("refund_reply", "in")));
    spell.wires.push(studio_wire("w7", ("switch", "billing"), ("billing_copy", "in")));
    spell.wires.push(studio_wire("w8", ("refund_text", "text"), ("billing_copy", "content")));
    let rt = runtime_for(spell);
    let names: Vec<String> = ["refund_reply", "billing_copy", "other_reply"].iter().map(|id| rt.state.canvas.display_title(&NodeId::from(*id))).collect();
    Ok(json!({ "names": names }))
}
fn small_truths() -> Result<serde_json::Value, String> {
    let mut search = runtime_for(tasting_spell());
    search.update(|app, _| app.palette_query = rune::TextEdit::new("send"));
    search.frame(1.0);
    let clear = layout_rect(&search, "palette-inline-clear").map(|rect| [rect[2].round(), rect[3].round()]);

    let mut clash = runtime_for(router_spell());
    type_prompt(&mut clash, &NodeId::from("switch"), "refund, default")?;
    clash.update(|app, _| app.sync_spell());
    let clash_check = validation_of(&mut clash)?;

    let cmd_z = |rt: &mut Runtime<StudioApp>| {
        rt.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
        rt.frame(0.0);
    };
    let mut moved = runtime_for(tasting_spell());
    moved.on_key = Some(magicklala_studio::studio_key);
    click(&mut moved, "mode-grimoire")?;
    let before = moved.state.canvas.nodes.len();
    moved.update(|app, _| {
        let _ = app.spawn("text/template");
        app.sync_edits();
    });
    let added = moved.state.canvas.nodes.len() == before + 1;
    moved.frame(0.0);
    let target = [900.0_f32, 420.0_f32];
    moved.update(move |app, _| {
        app.canvas.move_node(&NodeId::from("reply"), target);
        app.sync_edits();
    });
    moved.frame(0.0);
    cmd_z(&mut moved);
    let reply_at = moved.state.canvas.card(&NodeId::from("reply")).map(|card| card.position);
    Ok(json!({
        "clear": clear,
        "clash": { "check": clash_check["validation"], "errors": clash_check["errors"] },
        "undoKeepsMove": { "added": added, "addUndone": moved.state.canvas.nodes.len() == before, "replyStays": reply_at == Some(target) },
    }))
}
fn card_notes() -> Result<serde_json::Value, String> {
    let support = runtime_for(StudioApp::from_registry().spell.clone());
    let support_lines = |id: &str| text_lines_under(&support, &format!("node-{id}")).into_iter().map(|(line, _)| line).collect::<Vec<_>>();
    let router = runtime_for(example("webhook_router.json")?);
    let router_switch = node_of_kind(&router.state, "flow/switch").ok_or("no switch")?;
    let switch_lines: Vec<String> = text_lines_under(&router, &format!("node-{router_switch}")).into_iter().map(|(line, _)| line).collect();
    let mut grimoire = runtime_for(StudioApp::from_registry().spell.clone());
    click(&mut grimoire, "mode-grimoire")?;
    grimoire.frame(1.0);
    let grimoire_llm: Vec<String> = text_lines_under(&grimoire, "node-llm").into_iter().map(|(line, _)| line).collect();
    Ok(json!({
        "llm": support_lines("llm"),
        "recall": support_lines("recall"),
        "switch": switch_lines,
        "grimoireShowsNote": grimoire_llm.iter().any(|line| line == "gpt-4o-mini"),
    }))
}
fn var_undo() -> Result<serde_json::Value, String> {
    let cmd_z = |rt: &mut Runtime<StudioApp>| {
        rt.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
        rt.frame(0.0);
    };
    let default_of = |app: &StudioApp, name: &str| {
        app.spell.variables.iter().find(|variable| variable.name == name).and_then(|variable| variable.default.clone()).map(|value| format!("{value:?}"))
    };
    let recall_limit = |app: &StudioApp| app.spell.nodes.iter().find(|node| node.id.0 == "recall").and_then(|node| node.config.get("limit").cloned());

    let mut dial = runtime_for(StudioApp::from_registry().spell.clone());
    dial.on_key = Some(magicklala_studio::studio_key);
    dial.update(|app, _| {
        app.canvas.select(Some(NodeId::from("llm")));
        app.adjust_temperature(0.1);
    });
    dial.frame(0.0);
    let dial_after = default_of(&dial.state, "temperature");
    cmd_z(&mut dial);
    let dial_undone = default_of(&dial.state, "temperature");

    let mut typed = runtime_for(StudioApp::from_registry().spell.clone());
    typed.on_key = Some(magicklala_studio::studio_key);
    for text in ["1", "12"] {
        typed.update(move |app, _| {
            if let Some(edit) = app.var_default_edits.first_mut() {
                *edit = rune::TextEdit::new(text);
            }
            app.sync_edits();
        });
        typed.frame(0.0);
    }
    let typed_after = default_of(&typed.state, "max_memories");
    cmd_z(&mut typed);
    let typed_undone = default_of(&typed.state, "max_memories");

    let mut renamed = runtime_for(StudioApp::from_registry().spell.clone());
    renamed.on_key = Some(magicklala_studio::studio_key);
    for text in ["r", "recall_count"] {
        renamed.update(move |app, _| {
            if let Some(edit) = app.var_name_edits.first_mut() {
                *edit = rune::TextEdit::new(text);
            }
            app.sync_edits();
        });
        renamed.frame(0.0);
    }
    let rename_after = json!({
        "name": renamed.state.spell.variables.first().map(|variable| variable.name.clone()),
        "limit": recall_limit(&renamed.state),
        "hint": renamed.state.variable_binding_hint("recall_count").0,
    });
    let rename_check = validation_of(&mut renamed)?;
    cmd_z(&mut renamed);
    let rename_undone = json!({ "name": renamed.state.spell.variables.first().map(|variable| variable.name.clone()), "limit": recall_limit(&renamed.state) });
    Ok(json!({
        "dial": { "after": dial_after, "undone": dial_undone },
        "typed": { "after": typed_after, "undone": typed_undone },
        "rename": { "after": rename_after, "check": rename_check["validation"], "undone": rename_undone },
    }))
}
fn bound_looks(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut demo = runtime_for(StudioApp::from_registry().spell.clone());
    demo.update(|app, _| {
        app.canvas.select(Some(NodeId::from("llm")));
        app.right_tab = RightTab::Inspector;
    });
    demo.frame(1.0);
    let tile = all_texts(&demo).into_iter().filter(|line| line.starts_with("Temperature")).collect::<Vec<_>>();
    let mut measure = studio_text_system();
    let tile_cut: Vec<String> = text_lines_under(&demo, "inspector")
        .into_iter()
        .filter(|(line, _)| line.starts_with("Temperature"))
        .filter(|(line, rect)| {
            let (natural, line_h) = measure.measure(line, rune::TextStyle::new(10.5, 650.0), 1.0);
            natural > rect[2] * (rect[3] / line_h).round().max(1.0) + 2.0
        })
        .map(|(line, _)| line)
        .collect();
    let recall_line = demo.state.canvas.card(&NodeId::from("recall")).map(|card| demo.state.canvas.card_line(card).to_owned());

    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.load_graph(tasting_spell());
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "publish")?;
    rt.update(|app, _| app.left_tab = LeftTab::Agents);
    rt.frame(1.0);
    let row: Vec<String> = text_lines_under(&rt, "spell-tasting-agent").into_iter().map(|(line, _)| line).collect();
    Ok(json!({ "tile": tile, "tileCut": tile_cut, "recall": recall_line, "activeRow": row }))
}
fn redo_order() -> Result<serde_json::Value, String> {
    let press = |rt: &mut Runtime<StudioApp>, shift: bool| {
        rt.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, shift, ..Default::default() } });
        rt.frame(0.0);
    };
    let has_echo = |rt: &Runtime<StudioApp>| rt.state.canvas.card(&NodeId::from("echo")).is_some();
    let mut rt = runtime_for(tasting_spell());
    rt.on_key = Some(magicklala_studio::studio_key);
    rt.update(|app, _| {
        app.canvas.select(Some(NodeId::from("echo")));
        app.delete_selected_node();
        app.sync_edits();
    });
    rt.frame(0.0);
    let removed = !has_echo(&rt);
    press(&mut rt, false);
    let undone = has_echo(&rt);
    press(&mut rt, true);
    let redone = !has_echo(&rt);
    press(&mut rt, false);
    let undone_again = has_echo(&rt);
    rt.update(|app, _| {
        app.prompts.insert(NodeId::from("echo"), rune::TextEdit::new("Fresh: {{content}}"));
        app.sync_edits();
    });
    rt.frame(0.0);
    press(&mut rt, true);
    let fresh_kept = rt.state.prompts.get(&NodeId::from("echo")).map(|edit| edit.text.clone());
    Ok(
        json!({ "removed": removed, "undone": undone, "redone": redone, "undoneAgain": undone_again, "freshKept": fresh_kept, "echoAfterStaleRedo": has_echo(&rt) }),
    )
}
fn aside_undo_pour() -> Result<serde_json::Value, String> {
    let records = |app: &StudioApp| -> Vec<String> {
        app.spell
            .wires
            .iter()
            .map(|wire| format!("{}:{}.{}>{}.{}", wire.id.0, wire.from_node.0, wire.from_socket, wire.to_node.0, wire.to_socket))
            .collect()
    };
    let parked_of = |app: &StudioApp, id: &str| -> usize { app.spell.shelf.iter().find(|entry| entry.node.0 == id).map_or(0, |entry| entry.parked.len()) };
    let toss = |rt: &mut Runtime<StudioApp>, toss: magicklala_canvas::Toss| {
        rt.update(move |app, _| {
            app.toss(toss.clone());
            app.sync_edits();
        });
        rt.frame(0.0);
    };
    let mut rt = runtime_for(StudioApp::from_registry().spell.clone());
    rt.on_key = Some(magicklala_studio::studio_key);
    rt.update(|app, _| app.sync_spell());
    let original = records(&rt.state);
    toss(&mut rt, magicklala_canvas::Toss::Aside(NodeId::from("recall")));
    let parked_aside = parked_of(&rt.state, "recall");
    toss(&mut rt, magicklala_canvas::Toss::IntoPot(NodeId::from("recall")));
    let poured_matches = records(&rt.state) == original;
    rt.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
    rt.frame(0.0);
    rt.update(|app, _| app.sync_spell());
    let parked_undone = parked_of(&rt.state, "recall");
    let shelved_undone = rt.state.canvas.is_shelved(&NodeId::from("recall"));
    toss(&mut rt, magicklala_canvas::Toss::IntoPot(NodeId::from("recall")));
    let repoured = records(&rt.state);

    let spell = StudioApp::from_registry().spell.clone();
    let middles: Vec<NodeId> = spell
        .nodes
        .iter()
        .filter(|node| spell.wires.iter().any(|wire| wire.to_node == node.id) && spell.wires.iter().any(|wire| wire.from_node == node.id))
        .map(|node| node.id.clone())
        .take(2)
        .collect();
    let mut two = runtime_for(spell);
    two.update(|app, _| app.sync_spell());
    let two_original = records(&two.state);
    for id in &middles {
        toss(&mut two, magicklala_canvas::Toss::Aside(id.clone()));
    }
    let saved: SpellGraph = serde_json::from_str(&serde_json::to_string(&two.state.spell).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    let mut reloaded = runtime_for(StudioApp::from_registry().spell.clone());
    reloaded.update(move |app, _| app.load_graph(saved.clone()));
    reloaded.frame(0.0);
    for id in &middles {
        toss(&mut reloaded, magicklala_canvas::Toss::IntoPot(id.clone()));
    }
    let reloaded_records = records(&reloaded.state);
    Ok(json!({
        "parkedAside": parked_aside,
        "pouredMatches": poured_matches,
        "undone": { "shelved": shelved_undone, "parked": parked_undone },
        "repouredMatches": repoured == original,
        "repoured": repoured,
        "original": original,
        "reloadMatches": reloaded_records == two_original,
        "reloaded": reloaded_records,
        "twoOriginal": two_original,
    }))
}
fn picker_undo() -> Result<serde_json::Value, String> {
    let cmd_z = |rt: &mut Runtime<StudioApp>| {
        rt.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
        rt.frame(0.0);
    };
    let model_of = |app: &StudioApp| app.spell.nodes.iter().find(|node| node.id.0 == "llm").and_then(|node| node.config.get("model").cloned()).unwrap_or_default();
    let temperature_of = |app: &StudioApp| app.spell.variables.iter().find(|variable| variable.name == "temperature").and_then(|variable| variable.default.clone()).map(|value| format!("{value:?}"));
    let mut rt = runtime_for(StudioApp::from_registry().spell.clone());
    rt.on_key = Some(magicklala_studio::studio_key);
    let original = model_of(&rt.state);
    for model in ["gpt-4o", "gpt-4.1"] {
        rt.update(move |app, _| {
            app.canvas.select(Some(NodeId::from("llm")));
            app.select_model("openai", model);
        });
        rt.frame(0.0);
    }
    let mut models = vec![model_of(&rt.state)];
    for _ in 0..2 {
        cmd_z(&mut rt);
        models.push(model_of(&rt.state));
    }
    let mut dial = runtime_for(StudioApp::from_registry().spell.clone());
    dial.on_key = Some(magicklala_studio::studio_key);
    for _ in 0..2 {
        dial.update(|app, _| {
            app.canvas.select(Some(NodeId::from("llm")));
            app.adjust_temperature(0.1);
        });
        dial.frame(0.0);
    }
    let mut temperatures = vec![temperature_of(&dial.state)];
    for _ in 0..2 {
        cmd_z(&mut dial);
        temperatures.push(temperature_of(&dial.state));
    }
    Ok(json!({ "original": original, "models": models, "temperatures": temperatures }))
}
fn split_cleared() -> Result<serde_json::Value, String> {
    let mut spell = SpellGraph::new("Cleared Split");
    spell.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("fork", "flow/split", 260.0, json!({ "outputs": ["left", "right"] })),
        studio_node("first", "time/delay", 520.0, json!({ "ms": 10 })),
        studio_node("reply", "io/send_message", 780.0, serde_json::Value::Null),
    ];
    let link = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    spell.wires = vec![
        link("c1", ("trigger", "out"), ("fork", "in")),
        link("c2", ("fork", "left"), ("first", "in")),
        link("c3", ("first", "out"), ("reply", "in")),
        link("c4", ("trigger", "content"), ("reply", "content")),
    ];
    let mut rt = runtime_for(spell);
    let before = validation_of(&mut rt)?;
    rt.update(|app, _| {
        app.prompts.insert(NodeId::from("fork"), rune::TextEdit::new(""));
        app.sync_edits();
    });
    rt.frame(0.0);
    let outputs = rt.state.spell.nodes.iter().find(|node| node.id.0 == "fork").and_then(|node| node.config.get("outputs").cloned());
    let after = validation_of(&mut rt)?;
    let errors = match &rt.state.validation {
        ValidationState::Invalid(errors) => errors.clone(),
        _ => Vec::new(),
    };
    Ok(json!({ "before": before["validation"], "outputs": outputs, "after": after["validation"], "errors": errors }))
}
fn step_numbers() -> Result<serde_json::Value, String> {
    let mut spell = SpellGraph::new("Numbered Brew");
    spell.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("call", magicklala_graph::SUBGRAPH_KIND, 260.0, serde_json::Value::Null),
        studio_node("script", "code/rhai_script", 520.0, json!({ "script": "x" })),
        studio_node("reply", "io/send_message", 780.0, serde_json::Value::Null),
    ];
    let run = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4d01";
    let at = "2026-11-25T10:00:00Z";
    let brew = |status: &str| -> Result<Vec<magicklala_graph::TraceEvent>, String> {
        let summary = json!({ "run_id": run, "spell_id": spell_id_text(), "started_at": at, "finished_at": at, "status": status, "nodes_executed": 3, "outputs": {}, "error": "stopped" });
        [
            json!({ "event": "run_started", "run_id": run, "spell_id": spell_id_text(), "at": at }),
            json!({ "event": "node_finished", "run_id": run, "node_id": "trigger", "at": at, "duration_ms": 1 }),
            json!({ "event": "node_finished", "run_id": run, "node_id": "inner", "at": at, "duration_ms": 1, "subgraph_path": ["call"] }),
            json!({ "event": "node_finished", "run_id": run, "node_id": "inner_two", "at": at, "duration_ms": 1, "subgraph_path": ["call"] }),
            json!({ "event": "node_finished", "run_id": run, "node_id": "call", "at": at, "duration_ms": 2 }),
            json!({ "event": "node_failed", "run_id": run, "node_id": "script", "at": at, "error": "boom" }),
            json!({ "event": "run_finished", "run_id": run, "at": at, "summary": summary }),
        ]
        .into_iter()
        .map(|event| serde_json::from_value(event).map_err(|error| error.to_string()))
        .collect()
    };
    let mut report = serde_json::Map::new();
    for status in ["failed", "cancelled", "timed_out"] {
        let mut rt = runtime_for(spell.clone());
        let events = brew(status)?;
        rt.update(move |app, _| {
            app.pending_trace.extend(events.clone());
            while app.step_trace() {}
            app.right_tab = RightTab::Trace;
        });
        rt.frame(0.0);
        let badges: serde_json::Map<String, serde_json::Value> =
            rt.state.canvas.nodes.iter().map(|card| (card.id.0.clone(), json!(card.step_order))).collect();
        let mut steps = Vec::new();
        for index in 0..8 {
            let words = texts_under(&mut rt, &format!("trace-step-{index}"));
            if words.is_empty() {
                break;
            }
            steps.push(words);
        }
        report.insert(status.to_owned(), json!({ "badges": badges, "steps": steps, "header": texts_under(&mut rt, "inspector-trace").split(" Brewing sequence").next().unwrap_or_default().to_owned() }));
    }
    Ok(serde_json::Value::Object(report))
}

fn spell_id_text() -> String {
    "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4d02".to_owned()
}
fn router_spell() -> SpellGraph {
    let mut spell = SpellGraph::new("Router Agent");
    spell.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("switch", "flow/switch", 280.0, serde_json::Value::Null),
        studio_node("refund_text", "text/template", 560.0, json!({ "template": "Refunds desk" })),
        studio_node("other_text", "text/template", 560.0, json!({ "template": "Front desk" })),
        studio_node("refund_reply", "io/send_message", 840.0, serde_json::Value::Null),
        studio_node("other_reply", "io/send_message", 840.0, serde_json::Value::Null),
    ];
    spell.wires = vec![
        studio_wire("w1", ("trigger", "out"), ("switch", "in")),
        studio_wire("w2", ("trigger", "content"), ("switch", "value")),
        studio_wire("w3", ("switch", "default"), ("other_reply", "in")),
        studio_wire("w4", ("other_text", "text"), ("other_reply", "content")),
        studio_wire("w5", ("refund_text", "text"), ("refund_reply", "content")),
    ];
    spell
}

fn outputs_of(app: &StudioApp, id: &str) -> Vec<String> {
    app.canvas.card(&NodeId::from(id)).map(|card| card.outputs.iter().map(|socket| socket.name.clone()).collect()).unwrap_or_default()
}

fn pantry_settings() -> Result<serde_json::Value, String> {
    let mut rt = runtime_for(router_spell());
    let switch = NodeId::from("switch");
    rt.update(|app, _| app.canvas.select(Some(NodeId::from("switch"))));
    rt.frame(0.0);
    let field_shown = rt.find("prompt-field").is_some();
    type_prompt(&mut rt, &switch, "refund, billing")?;
    rt.update(|app, _| app.sync_spell());
    rt.frame(0.0);
    let typed_outputs = outputs_of(&rt.state, "switch");
    connect(&mut rt, ("switch", "refund"), ("refund_reply", "in"))?;
    let saved = rt.state.spell.nodes.iter().find(|node| node.id == switch).and_then(|node| node.config.get("cases").cloned());
    let check = validation_of(&mut rt)?;
    let refund_reply = type_chat(&mut rt, "refund")?;
    let other_reply = type_chat(&mut rt, "hello")?;
    let reopened = StudioApp::with_spell(rt.state.spell.clone(), registry_specs());
    let reopened_outputs = outputs_of(&reopened, "switch");
    let reopened_text = reopened.prompts.get(&switch).map(|edit| edit.text.clone());

    let mut trimmed = runtime_for(rt.state.spell.clone());
    type_prompt(&mut trimmed, &switch, "refund")?;
    trimmed.update(|app, _| app.sync_spell());
    let trimmed_outputs = outputs_of(&trimmed.state, "switch");

    let mut keys_spell = SpellGraph::new("Keys Agent");
    keys_spell.nodes = vec![studio_node("pick", "json/destructure", 0.0, serde_json::Value::Null)];
    let mut keys = runtime_for(keys_spell);
    let pick = NodeId::from("pick");
    keys.update(|app, _| app.canvas.select(Some(NodeId::from("pick"))));
    keys.frame(0.0);
    let keys_field = keys.find("prompt-field").is_some();
    type_prompt(&mut keys, &pick, "name, age")?;
    keys.update(|app, _| app.sync_spell());
    let key_outputs = outputs_of(&keys.state, "pick");
    let key_config = keys.state.spell.nodes.iter().find(|node| node.id == pick).and_then(|node| node.config.get("keys").cloned());
    Ok(json!({
        "fieldShown": field_shown,
        "typedOutputs": typed_outputs,
        "saved": saved,
        "check": check["validation"],
        "refundReply": refund_reply,
        "otherReply": other_reply,
        "reopenedOutputs": reopened_outputs,
        "reopenedText": reopened_text,
        "trimmedOutputs": trimmed_outputs,
        "keysField": keys_field,
        "keyOutputs": key_outputs,
        "keyConfig": key_config,
    }))
}
fn pantry_toss(rt: &mut Runtime<StudioApp>, kind: &str, query: &str) -> Result<serde_json::Value, String> {
    rt.update(|app, _| app.palette_query = rune::TextEdit::new(query));
    rt.frame(1.0);
    rt.frame(3.0);
    let pot = rt.presented("cauldron-center").map(|(rect, _)| [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5]).ok_or("no cauldron")?;
    let key = format!("palette-{}", kind.replace('/', "-"));
    let row = rt.find(&key).ok_or("no pantry row")?;
    let nodes_before = rt.state.spell.nodes.len();
    let wires_before = wire_list(&rt.state);
    drag(rt, &key, (pot[0] - row.0, pot[1] - row.1))?;
    let existing: Vec<String> = rt.state.spell.nodes.iter().take(nodes_before).map(|node| node.id.0.clone()).collect();
    let dropped = rt.state.spell.nodes.iter().find(|node| node.kind == kind && !existing.contains(&node.id.0)).map(|node| node.id.0.clone());
    let role = dropped.as_ref().and_then(|id| {
        magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size()).card(&NodeId::from(id.as_str())).map(|card| format!("{:?}", card.role))
    });
    Ok(json!({
        "kind": kind,
        "nodesBefore": nodes_before,
        "nodesAfter": rt.state.spell.nodes.len(),
        "id": dropped,
        "role": role,
        "wiresBefore": wires_before,
        "wiresAfter": wire_list(&rt.state),
        "ghostDuringDrag": rt.state.pantry_drag.is_some(),
    }))
}

fn visible_texts(rt: &Runtime<StudioApp>) -> Vec<String> {
    rt.elements().into_iter().filter(|element| !element.exiting).filter_map(|element| element.text).filter(|text| !text.trim().is_empty()).collect()
}

fn plain_words() -> Result<serde_json::Value, String> {
    let mut texts: Vec<String> = Vec::new();
    let mut empty = headless(StudioApp::with_spell(SpellGraph::new("New Potion"), registry_specs()));
    empty.frame(0.0);
    texts.extend(visible_texts(&empty));
    let mut broken = StudioApp::from_registry();
    broken.spell.wires.retain(|wire| wire.id.0 != "w2");
    broken.canvas = magicklala_canvas::CanvasState::from_graph(&broken.spell, &broken.specs);
    let mut hexed = headless(broken);
    click(&mut hexed, "validate")?;
    click(&mut hexed, "tab-agents")?;
    texts.extend(visible_texts(&hexed));
    let mut bare = headless(StudioApp::from_registry());
    bare.update(|app, _| app.canvas.select(None));
    bare.frame(0.0);
    texts.extend(visible_texts(&bare));
    let mut rt = headless(StudioApp::from_registry());
    for tab in ["tab-inspector", "tab-trace", "tab-variables", "tab-playground"] {
        click(&mut rt, tab)?;
        rt.frame(0.0);
        texts.extend(visible_texts(&rt));
    }
    let headings: Vec<String> = PALETTE_CATEGORY_ORDER
        .iter()
        .filter_map(|category| {
            rt.elements()
                .into_iter()
                .find(|element| element.key.as_deref() == Some(&format!("palette-heading-{category}")))
                .and_then(|element| element.label.or(element.text))
        })
        .collect();
    let mut tabs: Vec<String> = Vec::new();
    for key in ["tab-inspector", "tab-trace", "tab-variables", "tab-playground"] {
        let label = rt.elements().into_iter().find(|element| element.key.as_deref() == Some(key)).and_then(|element| element.label.or(element.text));
        tabs.push(label.unwrap_or_default());
    }
    texts.sort();
    texts.dedup();
    Ok(json!({ "texts": texts, "headings": headings, "tabs": tabs }))
}

fn texts_under(rt: &mut Runtime<StudioApp>, key: &str) -> String {
    rt.frame(0.0);
    let elements = rt.elements();
    let Some(root) = elements.iter().position(|element| element.key.as_deref() == Some(key) && !element.exiting) else { return String::new() };
    let inside = |mut index: usize| loop {
        if index == root {
            return true;
        }
        match elements[index].parent {
            Some(parent) => index = parent,
            None => return false,
        }
    };
    elements
        .iter()
        .enumerate()
        .filter(|(index, element)| !element.exiting && inside(*index))
        .filter_map(|(_, element)| element.text.clone())
        .collect::<Vec<_>>()
        .join(" ")
}

fn live_finish(rt: &mut Runtime<StudioApp>) -> Result<(), String> {
    let begun = Instant::now();
    while rt.state.is_running() {
        if begun.elapsed() > SETTLE {
            return Err("run never finished".to_owned());
        }
        rt.update(|app, _| {
            app.poll_background();
        });
        rt.frame(0.0);
        std::thread::sleep(Duration::from_millis(15));
    }
    rt.frame(0.0);
    Ok(())
}

fn brew_state(rt: &mut Runtime<StudioApp>) -> serde_json::Value {
    json!({
        "pill": texts_under(rt, "validation-pill"),
        "plaque": texts_under(rt, "cauldron-plaque"),
        "runningBrew": rt.state.canvas.running_brew,
        "outcome": rt.state.last_run.as_ref().map(|report| report.outcome.label()),
    })
}

fn brew_truth() -> Result<serde_json::Value, String> {
    let mut good = runtime_for(tasting_spell());
    click(&mut good, "run-spell")?;
    live_finish(&mut good)?;
    let succeeded = brew_state(&mut good);

    let mut bad = runtime_for(StudioApp::from_registry().spell.clone());
    click(&mut bad, "run-spell")?;
    live_finish(&mut bad)?;
    let mut failed = brew_state(&mut bad);
    bad.update(|app, _| app.trace_expanded = true);
    bad.frame(1.0);
    failed["hexedWords"] = json!(all_texts(&bad).into_iter().filter(|line| line.contains("Hexed")).collect::<Vec<_>>());
    failed["failedWords"] = json!(all_texts(&bad).into_iter().filter(|line| line.contains("Failed")).count());

    let mut clock = Clock(0.0);
    let mut slow = slow_runtime(5_000, Duration::from_secs(600));
    click_at(&mut slow, "run-spell", clock.tick())?;
    slow.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    slow.frame(clock.tick());
    let running = brew_state(&mut slow);
    click_at(&mut slow, "cancel-run", clock.tick())?;
    live_finish(&mut slow)?;
    let mut cancelled = brew_state(&mut slow);
    slow.update(|app, _| app.trace_expanded = true);
    slow.frame(clock.tick());
    cancelled["logWords"] = json!(all_texts(&slow).into_iter().filter(|line| ["Quenched", "Cancelled"].contains(&line.as_str())).collect::<Vec<_>>());
    let mut timed = slow_runtime(5_000, Duration::from_millis(300));
    click(&mut timed, "run-spell")?;
    live_finish(&mut timed)?;
    timed.update(|app, _| app.trace_expanded = true);
    timed.frame(1.0);
    let timed_words: Vec<String> = all_texts(&timed).into_iter().filter(|line| ["Boiled over", "Timed out"].contains(&line.as_str())).collect();
    Ok(json!({ "succeeded": succeeded, "failed": failed, "running": running, "cancelled": cancelled, "timedWords": timed_words }))
}

fn switch_mid_brew() -> Result<serde_json::Value, String> {
    let mut clock = Clock(0.0);
    let mut rt = slow_runtime(1_200, Duration::from_secs(600));
    click_at(&mut rt, "tab-playground", clock.tick())?;
    rt.update(|app, _| {
        app.chat_input = rune::TextEdit::new("still brewing?");
        app.send_chat();
    });
    rt.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    let running_before = rt.state.is_running();
    rt.update(|app, _| {
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    let switched_at = Instant::now();
    while switched_at.elapsed() < Duration::from_millis(2_400) {
        rt.update(|app, _| {
            app.poll_background();
        });
        rt.frame(clock.tick());
        std::thread::sleep(Duration::from_millis(20));
    }
    let stopped = rt.update_if(|app, _| app.wait_stopped(SETTLE)) || rt.state.background_tasks() == 0;
    Ok(json!({
        "runningBefore": running_before,
        "spell": rt.state.spell.name,
        "running": rt.state.is_running(),
        "chat": rt.state.chat.iter().map(|message| message.text.clone()).collect::<Vec<_>>(),
        "trace": rt.state.trace.len(),
        "lastRun": rt.state.last_run.as_ref().map(|report| report.outcome.label()),
        "runningBrew": rt.state.canvas.running_brew,
        "lastBrew": rt.state.canvas.last_brew.map(|status| format!("{status:?}")),
        "busyCards": rt.state.canvas.nodes.iter().filter(|card| card.status != ExecutionStatus::Idle).count(),
        "activeWires": rt.state.canvas.wires.iter().filter(|wire| wire.active).count(),
        "backgroundStopped": stopped && rt.state.background_tasks() == 0,
    }))
}

fn variable_defaults() -> Result<serde_json::Value, String> {
    use magicklala_graph::{Value, VariableDef};
    let mut spell = echo_spell();
    spell.variables = vec![
        VariableDef { name: "nickname".to_owned(), ty: SocketType::String, default: None, exposed: true },
        VariableDef { name: "blank".to_owned(), ty: SocketType::String, default: Some(Value::String(String::new())), exposed: true },
        VariableDef { name: "settings".to_owned(), ty: SocketType::Json, default: Some(Value::Json(json!({ "tone": "warm", "depth": 2 }))), exposed: true },
        VariableDef { name: "retries".to_owned(), ty: SocketType::Int, default: Some(Value::Int(3)), exposed: true },
    ];
    let original = serde_json::to_value(&spell.variables).map_err(|error| error.to_string())?;
    let mut rt = runtime_for(spell);
    click(&mut rt, "tab-variables")?;
    rt.update(|app, _| {
        app.sync_spell();
    });
    rt.frame(0.0);
    let opened = serde_json::to_value(&rt.state.spell.variables).map_err(|error| error.to_string())?;
    let dirty = rt.state.dirty;
    rt.update(|app, _| {
        if let Some(edit) = app.var_default_edits.get_mut(2) {
            *edit = rune::TextEdit::new(r#"{"tone":"brisk"}"#);
        }
        app.sync_spell();
    });
    let edited = serde_json::to_value(&rt.state.spell.variables).map_err(|error| error.to_string())?;
    Ok(json!({ "original": original, "opened": opened, "dirtyAfterOpen": dirty, "edited": edited }))
}

fn publish_plaque(db_path: &Path) -> Result<serde_json::Value, String> {
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.load_graph(tasting_spell());
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "publish")?;
    let published = texts_under(&mut rt, "cauldron-plaque");
    click(&mut rt, "run-spell")?;
    live_finish(&mut rt)?;
    let brewed = json!({ "plaque": texts_under(&mut rt, "cauldron-plaque"), "pill": texts_under(&mut rt, "validation-pill") });
    let canvas_rect = rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    let start = rt.find("node-echo-header").ok_or("no echo vial")?;
    drag(&mut rt, "node-echo-header", (canvas_rect[0] + 40.0 - start.0, canvas_rect[1] + canvas_rect[3] - 30.0 - start.1))?;
    let edited = json!({
        "plaque": texts_under(&mut rt, "cauldron-plaque"),
        "pill": texts_under(&mut rt, "validation-pill"),
        "litWires": rt.state.canvas.wires.iter().filter(|wire| wire.active).count(),
        "markedCards": rt.state.canvas.nodes.iter().filter(|card| card.status != ExecutionStatus::Idle || card.duration_ms.is_some()).count(),
    });
    Ok(json!({ "published": published, "brewed": brewed, "edited": edited }))
}

fn shelf_seals(db_path: &Path) -> Result<serde_json::Value, String> {
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.load_graph(tasting_spell());
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "publish")?;
    rt.update(|app, _| {
        app.left_tab = LeftTab::Agents;
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    rt.frame(1.0);
    rt.frame(2.0);
    let elsewhere = texts_under(&mut rt, "spell-tasting-agent");
    let research = texts_under(&mut rt, "spell-research-agent");
    Ok(json!({ "openSpell": rt.state.spell.name, "publishedRow": elsewhere, "draftRow": research }))
}
fn wires_of(app: &StudioApp, id: &str) -> usize {
    app.spell.wires.iter().filter(|wire| wire.from_node.0 == id || wire.to_node.0 == id).count()
}

fn role_of(app: &StudioApp, id: &str) -> Option<String> {
    magicklala_canvas::brew_plan(&app.canvas, app.canvas_size()).card(&NodeId::from(id)).map(|card| format!("{:?}", card.role))
}

fn toss_safety() -> Result<serde_json::Value, String> {
    let mut rt = headless(StudioApp::from_registry());
    let header = "node-search-header";
    let before = wires_of(&rt.state, "search");
    drag(&mut rt, header, (40.0, 0.0))?;
    let nudge = json!({ "before": before, "after": wires_of(&rt.state, "search"), "role": role_of(&rt.state, "search") });

    let start = rt.find(header).ok_or("no search vial")?;
    let canvas_rect = rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    drag(&mut rt, header, (canvas_rect[0] + 40.0 - start.0, canvas_rect[1] + canvas_rect[3] - 30.0 - start.1))?;
    let aside_wires = wires_of(&rt.state, "search");
    let aside_role = role_of(&rt.state, "search");
    let undo_shown = rt.find("undo-action").is_some();
    if undo_shown {
        click(&mut rt, "undo-action")?;
    }
    let undo = json!({ "asideWires": aside_wires, "asideRole": aside_role, "undoShown": undo_shown, "restoredWires": wires_of(&rt.state, "search"), "restoredRole": role_of(&rt.state, "search") });

    let start = rt.find("node-recall-header").ok_or("no recall vial")?;
    drag(&mut rt, "node-recall-header", (canvas_rect[0] + 40.0 - start.0, canvas_rect[1] + canvas_rect[3] - 30.0 - start.1))?;
    let stashed = rt.state.aside_wires.len();
    rt.update(|app, _| {
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    let leak = json!({ "stashedBefore": stashed, "stashedAfterSwitch": rt.state.aside_wires.len() });

    let mut menu_rt = headless(StudioApp::from_registry());
    click(&mut menu_rt, "node-search-header")?;
    let menu_canvas = menu_rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    right_click(&mut menu_rt, (menu_canvas[0] + menu_canvas[2] - 40.0, menu_canvas[1] + 140.0));
    let empty_items: Vec<String> = CONTEXT_MENU_ITEMS.iter().filter(|key| menu_rt.find(key).is_some()).map(|key| (*key).to_owned()).collect();
    click(&mut menu_rt, "context-dismiss")?;
    let card_at = menu_rt.find("node-llm-header").ok_or("no llm vial")?;
    right_click(&mut menu_rt, card_at);
    let card_items: Vec<String> = CONTEXT_MENU_ITEMS.iter().filter(|key| menu_rt.find(key).is_some()).map(|key| (*key).to_owned()).collect();
    let overlap = match (rect(&menu_rt, "context-menu"), rect(&menu_rt, "node-llm")) {
        (Some(m), Some(c)) => m[0] < c[0] + c[2] && c[0] < m[0] + m[2] && m[1] < c[1] + c[3] && c[1] < m[1] + m[3],
        _ => true,
    };
    let menu = json!({ "emptyItems": empty_items, "cardItems": card_items, "coversCard": overlap });

    let mut clock = Clock(0.0);
    let mut slow = slow_runtime(1_500, Duration::from_secs(600));
    click_at(&mut slow, "run-spell", clock.tick())?;
    slow.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    let wait_before = wires_of(&slow.state, "wait");
    let start = slow.find("node-wait-header").ok_or("no wait vial")?;
    let slow_canvas = slow.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    drag(&mut slow, "node-wait-header", (slow_canvas[0] + 40.0 - start.0, slow_canvas[1] + slow_canvas[3] - 30.0 - start.1))?;
    let while_running = json!({ "before": wait_before, "after": wires_of(&slow.state, "wait"), "running": slow.state.is_running() });
    slow.update(|app, _| app.cancel_run());
    Ok(json!({ "nudge": nudge, "undo": undo, "leak": leak, "menu": menu, "whileRunning": while_running }))
}

fn feeders_of(app: &StudioApp, id: &str) -> Vec<String> {
    app.spell.wires.iter().filter(|wire| wire.to_node.0 == id).map(|wire| format!("{}.{}->{}", wire.from_node.0, wire.from_socket, wire.to_socket)).collect()
}

fn contrast(ink: rune::Color, ground: rune::Color) -> f64 {
    let over = |i: f32, g: f32| f64::from(i * ink.a + g * (1.0 - ink.a));
    let lum = |r: f64, g: f64, b: f64| 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let x = lum(over(ink.r, ground.r), over(ink.g, ground.g), over(ink.b, ground.b));
    let y = lum(f64::from(ground.r), f64::from(ground.g), f64::from(ground.b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn recipe_honesty() -> Result<serde_json::Value, String> {
    let mut rt = headless(StudioApp::from_registry());
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    rt.update(|app, _| {
        app.spawn("memory/add");
    });
    let added = node_of_kind(&rt.state, "memory/add").ok_or("memory/add not added")?;
    let memory = json!({ "feeders": feeders_of(&rt.state, &added), "replyFeeders": feeders_of(&rt.state, "reply"), "id": added });

    let mut research = headless(StudioApp::from_registry());
    research.update(|app, _| {
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    click(&mut research, "validate")?;
    let research_check = research.state.validation.label();

    let mut mistyped_spell = SpellGraph::new("Mistyped");
    let place =
        |id: &str, kind: &str, x: f32| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position: [x, 0.0], config: serde_json::Value::Null };
    mistyped_spell.nodes = vec![
        place("trigger", "event/on_message", 0.0),
        place("recall", "memory/search", 260.0),
        place("llm", "ai/generate_text", 520.0),
        place("reply", "io/send_message", 780.0),
    ];
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    mistyped_spell.wires = vec![
        wire("w1", ("trigger", "out"), ("recall", "in")),
        wire("w2", ("trigger", "content"), ("recall", "query")),
        wire("w3", ("recall", "out"), ("llm", "in")),
        wire("w4", ("recall", "results"), ("llm", "prompt")),
        wire("w5", ("llm", "out"), ("reply", "in")),
        wire("w6", ("llm", "text"), ("reply", "content")),
    ];
    let mut mistyped = runtime_for(mistyped_spell);
    click(&mut mistyped, "validate")?;
    let mistyped_report = json!({ "card": texts_under(&mut mistyped, "node-llm"), "plaque": texts_under(&mut mistyped, "cauldron-plaque") });

    let mut empty = headless(StudioApp::with_spell(SpellGraph::new("New Potion"), registry_specs()));
    empty.update(|app, _| app.left_tab = LeftTab::Agents);
    empty.frame(1.0);
    empty.frame(2.0);
    click(&mut empty, "starter-event-on_message")?;
    let first = node_of_kind(&empty.state, "event/on_message").ok_or("no trigger added")?;
    let lone = role_of(&empty.state, &first);

    let mut side_spell = SpellGraph::new("Remember Everything");
    side_spell.nodes = vec![place("trigger", "event/on_message", 0.0), place("keep", "memory/add", 280.0)];
    side_spell.wires = vec![wire("w1", ("trigger", "out"), ("keep", "in")), wire("w2", ("trigger", "content"), ("keep", "content"))];
    let side = headless(StudioApp::with_spell(side_spell, registry_specs()));
    let side_plan = magicklala_canvas::brew_plan(&side.state.canvas, side.state.canvas_size());
    let side_effect = json!({ "role": role_of(&side.state, "keep"), "ghost": side_plan.ghost_bottle.is_some() });

    let mut shades = Vec::new();
    for light in [true, false] {
        let mut blank = StudioApp::with_spell(SpellGraph::new("New Potion"), registry_specs());
        if !light {
            blank.toggle_theme();
        }
        let mut rt = headless(blank);
        rt.frame(0.0);
        let elements = rt.elements();
        let brew = elements.iter().position(|element| element.key.as_deref() == Some("run-spell")).ok_or("no brew button")?;
        let fill = elements[brew].fill;
        let ink = elements
            .iter()
            .find(|element| element.parent == Some(brew) && element.text.is_some())
            .and_then(|element| element.text_color)
            .ok_or("no brew caption")?;
        let palette = magicklala_canvas::HearthPalette::for_theme(light);
        shades.push(json!({ "light": light, "disabledText": contrast(ink, fill), "ghostOutline": contrast(magicklala_canvas::hearth::ghost_ink(light), palette.paper) }));
    }
    Ok(
        json!({ "memory": memory, "researchCheck": research_check, "mistyped": mistyped_report, "loneFirst": lone, "sideEffect": side_effect, "shades": shades }),
    )
}

fn potion_colours() -> Result<serde_json::Value, String> {
    let hexed = |c: rune::Color| format!("{:.3},{:.3},{:.3}", c.r, c.g, c.b);
    let mut out = serde_json::Map::new();
    for (name, spell) in [("tasting", tasting_spell()), ("webhook", example("webhook_router.json")?)] {
        let mut rt = runtime_for(spell);
        if name == "webhook" {
            rt.update(|app, _| app.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } }));
        }
        click(&mut rt, "run-spell")?;
        live_finish(&mut rt)?;
        let canvas = &rt.state.canvas;
        let plan = magicklala_canvas::brew_plan(canvas, rt.state.canvas_size());
        let last = plan
            .with_role(magicklala_canvas::BrewRole::Ingredient)
            .filter_map(|placed| canvas.card(&placed.id))
            .filter(|card| card.status == ExecutionStatus::Succeeded)
            .last()
            .map(|card| (card.title.clone(), hexed(magicklala_canvas::hearth::category_tone(&card.category, canvas.light).liquid)));
        out.insert(name.to_owned(), json!({ "potion": magicklala_canvas::cauldron::potion_tint(canvas, &plan).map(hexed), "lastPoured": last }));
    }
    Ok(serde_json::Value::Object(out))
}

fn inside(outer: [f32; 4], inner: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5
        && inner[1] >= outer[1] - 0.5
        && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5
        && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
}

fn grimoire_place() -> Result<serde_json::Value, String> {
    let mut app = StudioApp::from_registry();
    app.size = (960.0, 640.0);
    app.left_rail_pinned = true;
    let mut rt = Runtime::new(app, studio_view, (960.0, 640.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|app, _| {
        app.spawn("ai/generate_text");
    });
    let fresh = rt.state.canvas.nodes.last().map(|card| card.id.0.clone()).ok_or("nothing spawned")?;
    click(&mut rt, "mode-grimoire")?;
    rt.frame(1.0);
    rt.frame(2.0);
    let canvas = rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    let ids: Vec<String> = rt.state.canvas.nodes.iter().map(|card| card.id.0.clone()).collect();
    let offscreen: Vec<String> = ids.iter().filter(|id| rt.presented(&format!("node-{id}")).is_none_or(|(rect, _)| !inside(canvas, rect))).cloned().collect();
    let world = |id: &str| rt.state.canvas.card(&NodeId::from(id)).map(|card| [card.position[0], card.position[1], card.size[0], card.size[1]]);
    let fresh_rect = world(&fresh).ok_or("fresh missing")?;
    let overlaps: Vec<String> =
        ids.iter().filter(|id| **id != fresh).filter(|id| world(id).is_some_and(|other| rects_overlap(fresh_rect, other))).cloned().collect();
    let llm_x = world("llm").map(|rect| rect[0]).unwrap_or_default();

    let mut webhook = StudioApp::with_spell(example("webhook_router.json")?, registry_specs());
    webhook.size = (960.0, 640.0);
    webhook.left_rail_pinned = true;
    webhook.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } });
    let mut hook = Runtime::new(webhook, studio_view, (960.0, 640.0), Env::new(true, 1.0));
    click(&mut hook, "run-spell")?;
    live_finish(&mut hook)?;
    let bottles: Vec<String> = magicklala_canvas::brew_plan(&hook.state.canvas, hook.state.canvas_size())
        .with_role(magicklala_canvas::BrewRole::Bottle)
        .map(|card| card.id.0.clone())
        .collect();
    let routes: Vec<serde_json::Value> = bottles
        .iter()
        .filter_map(|id| {
            let label = hook.presented(&format!("route-{id}")).map(|(rect, _)| rect)?;
            let card = hook.presented(&format!("node-{id}")).map(|(rect, _)| rect)?;
            Some(json!({ "bottle": id, "insideCard": inside(card, label) }))
        })
        .collect();
    Ok(
        json!({ "offscreen": offscreen, "fresh": fresh, "freshX": fresh_rect[0], "llmX": llm_x, "overlaps": overlaps, "routes": routes, "bottles": bottles.len() }),
    )
}

fn pour_shape() -> Result<serde_json::Value, String> {
    let rt = headless(StudioApp::from_registry());
    let pot = rt.presented("cauldron-center").map(|(rect, _)| rect).ok_or("no pot")?;
    let cards: Vec<serde_json::Value> = ["trigger", "recall", "search", "llm"]
        .iter()
        .filter_map(|id| {
            let card = rt.presented(&format!("node-{id}")).map(|(rect, _)| rect)?;
            let vial = rt.presented(&format!("vial-{id}")).map(|(rect, _)| rect);
            Some(json!({ "id": id, "card": card, "vial": vial }))
        })
        .collect();
    Ok(json!({ "pot": pot, "cards": cards }))
}

fn text_lines_under(rt: &Runtime<StudioApp>, root_key: &str) -> Vec<(String, [f32; 4])> {
    let elements = rt.elements();
    let Some(root) = elements.iter().position(|element| element.key.as_deref() == Some(root_key) && !element.exiting) else { return Vec::new() };
    let within = |mut index: usize| loop {
        if index == root {
            return true;
        }
        match elements[index].parent {
            Some(parent) => index = parent,
            None => return false,
        }
    };
    elements
        .iter()
        .enumerate()
        .filter(|(index, element)| !element.exiting && within(*index))
        .filter_map(|(_, element)| element.text.clone().map(|text| (text, element.rect)))
        .collect()
}

fn cut_titles(rt: &Runtime<StudioApp>, text: &mut rune::text::TextSystem) -> Vec<String> {
    let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
    rt.state
        .canvas
        .nodes
        .iter()
        .filter(|card| plan.card(&card.id).is_some_and(|placed| placed.role != magicklala_canvas::BrewRole::Bottle))
        .filter_map(|card| {
            let (_, rect) = text_lines_under(rt, &format!("node-{}", card.id.0)).into_iter().find(|(line, _)| line == &card.title)?;
            let (natural, line_h) = text.measure(&card.title, rune::TextStyle::new(13.0, 750.0), 1.0);
            let lines = (rect[3] / (line_h * plan.scale)).round().max(1.0);
            (natural * plan.scale > rect[2] * lines + 3.0).then(|| card.title.clone())
        })
        .collect()
}

fn cauldron_reads_cleanly() -> Result<serde_json::Value, String> {
    let mut text = magicklala_studio::studio_text_system();
    let mut cut = Vec::new();
    let mut crossings = Vec::new();
    for (name, spell, size) in [
        ("support", StudioApp::from_registry().spell.clone(), (1280.0_f32, 800.0_f32)),
        ("support", StudioApp::from_registry().spell.clone(), (960.0, 640.0)),
        ("dozen", dozen_ingredient_spell(), (960.0, 640.0)),
        ("dozen", dozen_ingredient_spell(), (1280.0, 800.0)),
    ] {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        cut.extend(cut_titles(&rt, &mut text).into_iter().map(|title| format!("{name}@{}: {title}", size.0)));
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        for (id, points) in magicklala_canvas::cauldron::pour_paths(&rt.state.canvas, &plan) {
            for other in plan.cards.iter().filter(|card| card.id != id) {
                let r = other.rect;
                if points.iter().any(|pt| pt[0] > r[0] + 2.0 && pt[0] < r[0] + r[2] - 2.0 && pt[1] > r[1] + 2.0 && pt[1] < r[1] + r[3] - 2.0) {
                    crossings.push(format!("{name}@{}: {} crosses {}", size.0, id.0, other.id.0));
                }
            }
        }
    }
    let demo = headless(StudioApp::from_registry());
    let last = magicklala_canvas::recipe_order(&demo.state.canvas)
        .into_iter()
        .filter_map(|id| demo.state.canvas.card(&id).cloned())
        .rfind(|card| card.kind != "io/send_message")
        .ok_or("no ingredient")?;
    let ready_broth = magicklala_canvas::hearth::broth_mid(
        magicklala_canvas::CauldronMood::Ready,
        true,
        magicklala_canvas::cauldron::potion_tint(&demo.state.canvas, &magicklala_canvas::brew_plan(&demo.state.canvas, demo.state.canvas_size())),
    );
    let last_liquid = magicklala_canvas::hearth::category_tone(&last.category, true).liquid;
    let mut roomy_app = StudioApp::from_registry();
    roomy_app.size = (1600.0, 1000.0);
    let mut roomy = Runtime::new(roomy_app, studio_view, (1600.0, 1000.0), Env::new(true, 1.0));
    roomy.frame(0.0);
    let pot_width = roomy.presented("cauldron-center").map(|(rect, _)| rect[2]).unwrap_or_default();

    let place =
        |id: &str, kind: &str, x: f32| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position: [x, 0.0], config: serde_json::Value::Null };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let mut chain = SpellGraph::new("Chain");
    chain.nodes = vec![
        place("trigger", "event/on_message", 0.0),
        place("a", "ai/generate_text", 1.0),
        place("b", "ai/generate_text", 2.0),
        place("c", "ai/generate_text", 3.0),
        place("reply", "io/send_message", 4.0),
    ];
    chain.wires = vec![
        wire("w1", ("trigger", "out"), ("a", "in")),
        wire("w2", ("a", "out"), ("b", "in")),
        wire("w3", ("b", "out"), ("c", "in")),
        wire("w4", ("c", "out"), ("reply", "in")),
        wire("w5", ("trigger", "content"), ("a", "prompt")),
        wire("w6", ("a", "text"), ("b", "prompt")),
        wire("w7", ("trigger", "content"), ("c", "prompt")),
        wire("w8", ("c", "text"), ("reply", "content")),
    ];
    let chain_app = StudioApp::with_spell(chain, registry_specs());
    let chain_plan = magicklala_canvas::brew_plan(&chain_app.canvas, chain_app.canvas_size());
    let steps: Vec<Option<usize>> =
        ["trigger", "a", "b", "c", "reply"].iter().map(|id| chain_plan.card(&NodeId::from(*id)).and_then(|card| card.recipe_step)).collect();

    let mut relay = SpellGraph::new("Relay");
    relay.nodes = vec![place("trigger", "event/on_message", 0.0), place("send", "io/send_message", 1.0), place("keep", "memory/add", 2.0)];
    relay.wires = vec![
        wire("w1", ("trigger", "out"), ("send", "in")),
        wire("w2", ("trigger", "content"), ("send", "content")),
        wire("w3", ("send", "out"), ("keep", "in")),
        wire("w4", ("trigger", "content"), ("keep", "content")),
    ];
    let relay_app = StudioApp::with_spell(relay, registry_specs());
    let relay_roles: serde_json::Map<String, serde_json::Value> =
        ["trigger", "send", "keep"].iter().map(|id| ((*id).to_owned(), json!(role_of(&relay_app, id)))).collect();

    let mut aside_rt = headless(StudioApp::from_registry());
    aside_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    aside_rt.frame(1.0);
    aside_rt.frame(3.0);
    let shelf = aside_rt.presented("aside-shelf").map(|(rect, _)| rect);
    let recall = aside_rt.presented("node-recall").map(|(rect, _)| rect);
    let aside_shelf = json!({ "shelf": shelf.is_some(), "holdsCard": matches!((shelf, recall), (Some(s), Some(c)) if inside(s, c)) });

    let mut hook = runtime_for(example("webhook_router.json")?);
    hook.frame(0.0);
    let bottle_titles: Vec<String> = magicklala_canvas::brew_plan(&hook.state.canvas, hook.state.canvas_size())
        .with_role(magicklala_canvas::BrewRole::Bottle)
        .filter_map(|card| text_lines_under(&hook, &format!("node-{}", card.id.0)).into_iter().next().map(|(line, _)| line))
        .collect();

    let mut grim = headless(StudioApp::from_registry());
    click(&mut grim, "mode-grimoire")?;
    grim.frame(1.0);
    let ids: Vec<String> = grim.state.canvas.nodes.iter().map(|card| card.id.0.clone()).collect();
    let grimoire_smallest = pot_measure(&grim, &ids)["smallestLine"].clone();

    Ok(json!({
        "cut": cut,
        "crossings": crossings,
        "readyBroth": format!("{:.3},{:.3},{:.3}", ready_broth.r, ready_broth.g, ready_broth.b),
        "lastLiquid": format!("{:.3},{:.3},{:.3}", last_liquid.r, last_liquid.g, last_liquid.b),
        "potWidth": pot_width,
        "chainSteps": steps,
        "relayRoles": relay_roles,
        "asideShelf": aside_shelf,
        "bottleTitles": bottle_titles,
        "grimoireSmallest": grimoire_smallest,
    }))
}

fn pot_body(rt: &Runtime<StudioApp>) -> Option<[f32; 4]> {
    let steaming = rt.state.is_running() || rt.state.canvas.last_brew.is_some();
    let share = if steaming { magicklala_canvas::POT_STEAM_TOP } else { magicklala_canvas::POT_RIM_TOP };
    layout_rect(rt, "hero-cauldron").map(|pot| [pot[0], pot[1] + pot[3] * share, pot[2], pot[3] * (1.0 - share)])
}

fn layout_rect(rt: &Runtime<StudioApp>, key: &str) -> Option<[f32; 4]> {
    rt.elements().iter().find(|element| element.key.as_deref() == Some(key) && !element.exiting).map(|element| element.rect)
}

fn rects_touch(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] - 0.5 && b[0] < a[0] + a[2] - 0.5 && a[1] < b[1] + b[3] - 0.5 && b[1] < a[1] + a[3] - 0.5
}

fn one_name_per_step() -> Result<serde_json::Value, String> {
    let mut title_misfits = Vec::new();
    for (name, spell) in
        [("support", StudioApp::from_registry().spell.clone()), ("dozen", dozen_ingredient_spell()), ("webhook", example("webhook_router.json")?)]
    {
        for size in [(960.0_f32, 640.0_f32), (1280.0, 800.0)] {
            let mut app = StudioApp::with_spell(spell.clone(), registry_specs());
            app.size = size;
            let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
            rt.frame(0.0);
            rt.frame(2.0);
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            for placed in plan.cards.iter().filter(|card| card.role != magicklala_canvas::BrewRole::Bottle) {
                let Some(card) = rt.state.canvas.card(&placed.id) else { continue };
                let Some(header) = layout_rect(&rt, &format!("node-{}-header", card.id.0)) else { continue };
                let vial = layout_rect(&rt, &format!("vial-{}", card.id.0));
                let lines: Vec<[f32; 4]> =
                    text_lines_under(&rt, &format!("node-{}", card.id.0)).into_iter().filter(|(line, _)| line == &card.title).map(|(_, rect)| rect).collect();
                let fits = !lines.is_empty() && lines.iter().all(|rect| inside(header, *rect) && vial.is_none_or(|vial| !rects_touch(vial, *rect)));
                if !fits {
                    title_misfits.push(format!("{name}@{}: {}", size.0, card.title));
                }
            }
        }
    }

    let names_in = |rt: &Runtime<StudioApp>, ids: &[&str]| -> serde_json::Map<String, serde_json::Value> {
        ids.iter()
            .filter_map(|id| {
                let lines = text_lines_under(rt, &format!("node-{id}"));
                let line = lines.into_iter().map(|(line, _)| line).find(|line| line.starts_with("Send"))?;
                Some(((*id).to_owned(), json!(line)))
            })
            .collect()
    };
    let bottles = ["order_reply", "support_reply", "ignored_reply"];
    let mut hook = runtime_for(example("webhook_router.json")?);
    hook.update(|app, _| app.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } }));
    click(&mut hook, "run-spell")?;
    live_finish(&mut hook)?;
    hook.frame(4.0);
    let cauldron_names = names_in(&hook, &bottles);
    let trace_names: serde_json::Map<String, serde_json::Value> = hook
        .state
        .trace
        .iter()
        .filter(|line| line.kind == "NodeFinished")
        .filter_map(|line| {
            let id = line.node.as_ref()?.0.clone();
            bottles.contains(&id.as_str()).then(|| (id, json!(line.detail.rsplit_once(" · ").map_or(line.detail.as_str(), |(name, _)| name))))
        })
        .collect();
    click(&mut hook, "mode-grimoire")?;
    hook.frame(5.0);
    hook.frame(7.0);
    let grimoire_names = names_in(&hook, &bottles);

    let place =
        |id: &str, kind: &str, x: f32| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position: [x, 0.0], config: serde_json::Value::Null };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let mut twins = SpellGraph::new("Twin Replies");
    twins.nodes = vec![place("trigger", "event/on_message", 0.0), place("first_reply", "io/send_message", 1.0), place("second_reply", "io/send_message", 2.0)];
    twins.wires = vec![
        wire("w1", ("trigger", "out"), ("first_reply", "in")),
        wire("w2", ("trigger", "content"), ("first_reply", "content")),
        wire("w3", ("trigger", "out"), ("second_reply", "in")),
        wire("w4", ("trigger", "content"), ("second_reply", "content")),
    ];
    let mut twin_rt = runtime_for(twins);
    twin_rt.frame(1.0);
    let twin_names = names_in(&twin_rt, &["first_reply", "second_reply"]);

    let mut brewed = headless(StudioApp::from_registry());
    click(&mut brewed, "run-spell")?;
    live_finish(&mut brewed)?;
    brewed.frame(4.0);
    let bobbin = layout_rect(&brewed, "trace-bobbin").ok_or("no log icon")?;
    let scroll = layout_rect(&brewed, "trace-scroll").ok_or("no log chips")?;
    let log_gap = scroll[1] - (bobbin[1] + bobbin[3]);
    let idle = headless(StudioApp::from_registry());
    let idle_bobbin = layout_rect(&idle, "trace-bobbin").ok_or("no idle log icon")?;
    let log_icons = [[bobbin[2], bobbin[3]], [idle_bobbin[2], idle_bobbin[3]]];

    let mut empty = headless(StudioApp::with_spell(SpellGraph::new("New Potion"), registry_specs()));
    empty.frame(1.0);
    let second_add = layout_rect(&empty, "empty-add-trigger").is_some();

    Ok(json!({
        "titleMisfits": title_misfits,
        "cauldronNames": cauldron_names,
        "grimoireNames": grimoire_names,
        "traceNames": trace_names,
        "twinNames": twin_names,
        "logGap": log_gap,
        "logIcons": log_icons,
        "empty": { "secondAdd": second_add },
    }))
}
fn all_texts(rt: &Runtime<StudioApp>) -> Vec<String> {
    rt.elements().iter().filter(|element| !element.exiting).filter_map(|element| element.text.clone()).collect()
}

fn compact_fit() -> Result<serde_json::Value, String> {
    let compact = |spell: SpellGraph| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = (960.0, 640.0);
        let mut rt = Runtime::new(app, studio_view, (960.0, 640.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let demo = StudioApp::from_registry().spell.clone();

    let mut vars = compact(demo.clone());
    vars.update(|app, _| app.right_tab = RightTab::Variables);
    vars.frame(1.0);
    let var_widths: Vec<f32> =
        (0..vars.state.spell.variables.len()).map(|index| layout_rect(&vars, &format!("var-label-{index}")).map_or(0.0, |rect| rect[2].round())).collect();

    let mut broken = demo.clone();
    let cut = broken.wires.iter().position(|wire| wire.to_socket == "query").ok_or("demo spell has no query wire")?;
    let removed = broken.wires.remove(cut);
    let mut hexed = compact(broken.clone());
    hexed.update(|app, _| app.canvas.select(Some(removed.to_node.clone())));
    click(&mut hexed, "validate")?;
    hexed.frame(2.0);
    let error_box = layout_rect(&hexed, "inspector-error").ok_or("no error box")?;
    let spilled: Vec<String> = ["inspector-fix", "inspector-open-grimoire"]
        .iter()
        .filter(|key| layout_rect(&hexed, key).is_none_or(|rect| !inside(error_box, rect)))
        .map(|key| (*key).to_owned())
        .collect();

    let mut rail = compact(demo.clone());
    click(&mut rail, "expand-nodes")?;
    let opened = rail.state.left_rail_pinned;
    rail.frame(2.0);
    let collapsed = click_at(&mut rail, "collapse-left-rail", 2.5).is_ok() && !rail.state.left_rail_pinned;

    let mut grim = headless(StudioApp::from_registry());
    click(&mut grim, "run-spell")?;
    live_finish(&mut grim)?;
    click(&mut grim, "mode-grimoire")?;
    grim.frame(3.0);
    let glyphs: Vec<String> = all_texts(&grim).into_iter().filter(|line| line.contains(['\u{2713}', '\u{25B4}', '\u{25BE}'])).collect();

    let mut hook = runtime_for(example("webhook_router.json")?);
    hook.update(|app, _| app.canvas.select(Some(NodeId::from("order_reply"))));
    hook.frame(1.0);
    let recipe_title = text_lines_under(&hook, "inspector-card").into_iter().map(|(line, _)| line).find(|line| line.starts_with("Send"));

    let stats_of = |rt: &Runtime<StudioApp>| text_lines_under(rt, "cauldron-plaque").into_iter().map(|(line, _)| line).find(|line| line.contains("ingredient"));
    let mut aside = headless(StudioApp::from_registry());
    aside.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    aside.frame(1.0);
    let place =
        |id: &str, kind: &str, x: f32| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position: [x, 0.0], config: serde_json::Value::Null };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let mut relay = SpellGraph::new("Relay");
    relay.nodes = vec![place("trigger", "event/on_message", 0.0), place("send", "io/send_message", 1.0), place("keep", "memory/add", 2.0)];
    relay.wires = vec![
        wire("w1", ("trigger", "out"), ("send", "in")),
        wire("w2", ("trigger", "content"), ("send", "content")),
        wire("w3", ("send", "out"), ("keep", "in")),
        wire("w4", ("trigger", "content"), ("keep", "content")),
    ];
    let mut relay_rt = runtime_for(relay);
    relay_rt.frame(1.0);

    let mut checked = headless(StudioApp::with_spell(broken, registry_specs()));
    click(&mut checked, "validate")?;
    checked.update(|app, _| {
        app.canvas.select(None);
        app.right_tab = RightTab::Trace;
    });
    checked.frame(2.0);
    let trace_texts = text_lines_under(&checked, "inspector").into_iter().map(|(line, _)| line).collect::<Vec<_>>();
    let mut texts = trace_texts.clone();
    texts.extend(all_texts(&grim));
    let singular_plurals: Vec<String> =
        texts.into_iter().filter(|line| line.contains("1 steps") || line.contains("1 events") || line.contains(" 1 issues")).collect();

    Ok(json!({
        "varLabelWidths": var_widths,
        "errorSpill": spilled,
        "rail": { "opened": opened, "collapsed": collapsed },
        "grimoireGlyphs": glyphs,
        "recipeTitle": recipe_title,
        "stats": { "aside": stats_of(&aside), "relay": stats_of(&relay_rt) },
        "checkHeader": trace_texts.iter().find(|line| line.starts_with("Brew #") || line.starts_with("Check")),
        "singularPlurals": singular_plurals,
    }))
}
fn polylines_cross(a: &[[f32; 2]], b: &[[f32; 2]]) -> bool {
    let side = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    a.windows(2).any(|s| {
        b.windows(2).any(|t| {
            let (d1, d2) = (side(s[0], s[1], t[0]), side(s[0], s[1], t[1]));
            let (d3, d4) = (side(t[0], t[1], s[0]), side(t[0], t[1], s[1]));
            d1 * d2 < 0.0 && d3 * d4 < 0.0
        })
    })
}

fn dozen_lanes() -> serde_json::Value {
    let mut app = StudioApp::with_spell(dozen_ingredient_spell(), registry_specs());
    app.size = (960.0, 640.0);
    let plan = magicklala_canvas::brew_plan(&app.canvas, app.canvas_size());
    let pours = magicklala_canvas::cauldron::pour_paths(&app.canvas, &plan);
    let ribbons = magicklala_canvas::cauldron::ribbon_paths(&plan);
    let crossing: Vec<String> =
        pours.iter().filter(|(_, points)| ribbons.iter().any(|ribbon| polylines_cross(points, ribbon))).map(|(id, _)| id.0.clone()).collect();
    let mut lanes: Vec<i64> = pours.iter().filter_map(|(_, points)| points.get(6).map(|point| (point[0] * 10.0).round() as i64)).collect();
    lanes.sort_unstable();
    lanes.dedup();
    json!({ "crossing": crossing, "strands": lanes.len(), "pours": pours.len() })
}

fn recipe_mending() -> Result<serde_json::Value, String> {
    let pulled = |delete: bool| -> Result<serde_json::Value, String> {
        let mut rt = headless(StudioApp::from_registry());
        let reply = node_of_kind(&rt.state, "io/send_message").ok_or("no reply")?;
        let model = NodeId::from(node_of_kind(&rt.state, "ai/generate_text").ok_or("no model")?.as_str());
        rt.update(move |app, _| {
            if delete {
                app.canvas.select(Some(model.clone()));
                app.delete_selected_node();
            } else {
                app.toss(magicklala_canvas::Toss::Aside(model.clone()));
            }
        });
        click(&mut rt, "validate")?;
        Ok(json!({ "check": rt.state.validation.label(), "contentFed": feeders_of(&rt.state, &reply).iter().any(|feed| feed.ends_with("->content")) }))
    };
    let spawned = |kinds: &[&str]| {
        let mut rt = headless(StudioApp::with_spell(SpellGraph::new("Shout"), registry_specs()));
        for kind in kinds {
            let kind = (*kind).to_owned();
            rt.update(move |app, _| {
                app.spawn(&kind);
            });
        }
        rt
    };
    let mut replace_rt = spawned(&["event/on_message", "io/send_message", "text/replace"]);
    let replace = NodeId::from(node_of_kind(&replace_rt.state, "text/replace").ok_or("no replace")?.as_str());
    replace_rt.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(replace.clone())));
    click(&mut replace_rt, "validate")?;
    let replace_reply = node_of_kind(&replace_rt.state, "io/send_message").ok_or("no reply")?;
    let replace_aside = json!({ "check": replace_rt.state.validation.label(), "replyFeeders": feeders_of(&replace_rt.state, &replace_reply) });

    let mut template_rt = spawned(&["event/on_message", "io/send_message", "text/template"]);
    click(&mut template_rt, "validate")?;
    let template = node_of_kind(&template_rt.state, "text/template").ok_or("no template")?;
    let template_reply = node_of_kind(&template_rt.state, "io/send_message").ok_or("no reply")?;
    let template_toss = json!({
        "check": template_rt.state.validation.label(),
        "role": role_of(&template_rt.state, &template),
        "templateFed": !feeders_of(&template_rt.state, &template).is_empty(),
        "replyFeeders": feeders_of(&template_rt.state, &template_reply),
        "template": template,
    });

    let mut hollow = StudioApp::from_registry().spell.clone();
    let reply = hollow.nodes.iter().find(|node| node.kind == "io/send_message").map(|node| node.id.0.clone()).ok_or("no reply")?;
    hollow.wires.retain(|wire| wire.to_node.0 != reply || wire.to_socket != "content");
    let mut hexed = runtime_for(hollow);
    click(&mut hexed, "validate")?;
    hexed.frame(2.0);
    let bottle_pill = text_lines_under(&hexed, &format!("node-{reply}")).into_iter().map(|(line, _)| line).find(|line| line.contains("content"));

    let mut brewed = headless(StudioApp::from_registry());
    click(&mut brewed, "run-spell")?;
    live_finish(&mut brewed)?;
    let plan = magicklala_canvas::brew_plan(&brewed.state.canvas, brewed.state.canvas_size());
    let brewed_reply = NodeId::from(node_of_kind(&brewed.state, "io/send_message").ok_or("no reply")?.as_str());
    let tint = |color: Option<rune::Color>| color.map(|c| format!("{:.3},{:.3},{:.3}", c.r, c.g, c.b));
    let tints = json!({
        "bottle": tint(magicklala_canvas::cauldron::bottle_tint(&brewed.state.canvas, &plan, &brewed_reply)),
        "potion": tint(magicklala_canvas::cauldron::potion_tint(&brewed.state.canvas, &plan)),
    });

    let mut clock = 0.0;
    let mut tick = || {
        clock += 0.5;
        clock
    };
    let mut slow = slow_runtime(1_500, Duration::from_secs(600));
    click_at(&mut slow, "run-spell", tick())?;
    slow.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    let nodes_before = slow.state.spell.nodes.len();
    let vars_before = slow.state.spell.variables.len();
    slow.update(|app, _| {
        app.canvas.select(Some(NodeId::from("wait")));
        app.delete_selected_node();
        app.add_variable();
        app.undo();
    });
    slow.frame(tick());
    let wait_running = slow.state.canvas.card(&NodeId::from("wait")).is_some_and(|card| card.status == ExecutionStatus::Running);
    let mid_brew = json!({
        "nodes": slow.state.spell.nodes.len() as i64 - nodes_before as i64,
        "vars": slow.state.spell.variables.len() as i64 - vars_before as i64,
        "stillSimmering": wait_running,
    });
    live_finish(&mut slow)?;

    let mut menu = headless(StudioApp::from_registry());
    let recall = NodeId::from("recall");
    let open_menu = |rt: &mut Runtime<StudioApp>, id: &NodeId| {
        let id = id.clone();
        rt.update(move |app, _| {
            app.canvas.select(Some(id.clone()));
            app.canvas.context_menu = Some([420.0, 300.0]);
        });
        rt.frame(1.0);
    };
    open_menu(&mut menu, &recall);
    let set_aside = click_at(&mut menu, "context-set-aside", 1.0).is_ok();
    let after_aside = role_of(&menu.state, "recall");
    open_menu(&mut menu, &recall);
    let pour_in = click_at(&mut menu, "context-pour-in", 1.5).is_ok();
    let after_pour = role_of(&menu.state, "recall");

    Ok(json!({
        "pullModel": { "aside": pulled(false)?, "deleted": pulled(true)? },
        "replaceAside": replace_aside,
        "templateToss": template_toss,
        "bottlePill": bottle_pill,
        "tints": tints,
        "midBrew": mid_brew,
        "dozenLanes": dozen_lanes(),
        "menu": { "setAside": set_aside, "afterAside": after_aside, "pourIn": pour_in, "afterPour": after_pour },
    }))
}
fn one_recipe(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config: serde_json::Value::Null,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };

    let mut spare = tasting_spell();
    spare.name = "Spare Agent".to_owned();
    spare.nodes.push(place("spare_reply", "io/send_message", 3.0));
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.load_graph(spare);
    let mut pub_rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    pub_rt.frame(0.0);
    click(&mut pub_rt, "validate")?;
    let spare_check = pub_rt.state.validation.label();
    let spare_role = role_of(&pub_rt.state, "spare_reply");
    click(&mut pub_rt, "save")?;
    click(&mut pub_rt, "publish")?;
    let release = pub_rt.state.last_published.as_ref().map(|release| release.graph.clone()).ok_or("nothing published")?;
    let release_valid = SpellValidator::new(registry_specs()).validate(&release).is_ok();
    let release_has_spare = release.nodes.iter().any(|node| node.id.0 == "spare_reply");
    let draft_keeps_spare = pub_rt.state.spell.nodes.iter().any(|node| node.id.0 == "spare_reply");

    pub_rt.update(|app, _| {
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    click(&mut pub_rt, "tab-agents")?;
    pub_rt.frame(1.0);
    click(&mut pub_rt, "spell-spare-agent")?;
    pub_rt.update(|app, _| app.confirm_switch());
    pub_rt.frame(2.0);
    let reopened = json!({ "name": pub_rt.state.spell.name, "live": pub_rt.state.active_release_version, "releases": pub_rt.state.releases.len() });

    let mut names = Vec::new();
    for _ in 0..2 {
        pub_rt.update(|app, _| {
            app.new_empty_spell();
            app.spawn("event/on_message");
        });
        click(&mut pub_rt, "save")?;
        names.push(json!({ "name": pub_rt.state.spell.name, "saved": pub_rt.state.store_error.is_none() }));
    }

    let mut hook = runtime_for(example("webhook_router.json")?);
    hook.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("route"))));
    let route_bridges = hook.state.spell.wires.iter().filter(|wire| wire.to_socket == "in" && wire.to_node.0.ends_with("_reply")).count();

    let mut join = SpellGraph::new("Join");
    join.nodes = vec![
        place("trigger", "event/on_message", 0.0),
        place("a", "time/delay", 1.0),
        place("b", "time/delay", 1.0),
        place("join", "flow/wait_for_all", 2.0),
        place("reply", "io/send_message", 3.0),
    ];
    join.wires = vec![
        wire("w1", ("trigger", "out"), ("a", "in")),
        wire("w2", ("trigger", "out"), ("b", "in")),
        wire("w3", ("a", "out"), ("join", "in")),
        wire("w4", ("b", "out"), ("join", "in")),
        wire("w5", ("join", "out"), ("reply", "in")),
        wire("w6", ("trigger", "content"), ("reply", "content")),
    ];
    let mut join_rt = runtime_for(join);
    let into_join = |app: &StudioApp| app.spell.wires.iter().filter(|wire| wire.to_node.0 == "join").count();
    let join_before = into_join(&join_rt.state);
    join_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("join"))));
    join_rt.update(|app, _| app.undo());
    let join_after = into_join(&join_rt.state);

    let mut dangling = StudioApp::from_registry().spell.clone();
    dangling.nodes.push(place("nap", "time/delay", 2.0));
    let trigger = dangling.nodes.iter().find(|node| node.kind == "event/on_message").map(|node| node.id.clone()).ok_or("no trigger")?;
    dangling.wires.push(Wire {
        id: WireId::from("nap_in"),
        from_node: trigger,
        from_socket: "out".to_owned(),
        to_node: NodeId::from("nap"),
        to_socket: "in".to_owned(),
    });
    dangling.nodes.push(place("loose_reply", "io/send_message", 4.0));
    let dangling_rt = runtime_for(dangling);
    let shelf = json!({ "wiredDelay": role_of(&dangling_rt.state, "nap"), "looseSend": role_of(&dangling_rt.state, "loose_reply") });

    let mut tray = headless(StudioApp::with_spell(SpellGraph::new("Tray"), registry_specs()));
    tray.update(|app, _| app.palette_query = rune::TextEdit::new("On Message"));
    tray.frame(1.0);
    tray.frame(3.0);
    let key = "palette-event-on_message";
    let row = tray.find(key).ok_or("no pantry row")?;
    let board = tray.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    drag(&mut tray, key, (board[0] + 60.0 - row.0, board[1] + board[3] - 30.0 - row.1))?;
    let dropped = tray.state.spell.nodes.first().map(|node| node.id.0.clone()).ok_or("nothing dropped")?;
    let tray_role = role_of(&tray.state, &dropped);
    click(&mut tray, "run-spell")?;
    let tray_brewed = tray.state.runs;

    let mut slow = slow_runtime(1_500, Duration::from_secs(600));
    click_at(&mut slow, "run-spell", 0.5)?;
    slow.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    slow.frame(1.0);
    let reply = slow.state.canvas.card(&NodeId::from("reply")).cloned().ok_or("no reply")?;
    let bottle_mood = format!("{:?}", magicklala_canvas::view::bottle_art_mood(&slow.state.canvas, &reply));
    let mut refused = String::new();
    slow.update(|app, _| {
        refused = app.publish().err().map(|error| error.to_string()).unwrap_or_default();
    });
    live_finish(&mut slow)?;

    Ok(json!({
        "spare": { "check": spare_check, "role": spare_role, "releaseValid": release_valid, "releaseHasSpare": release_has_spare, "draftKeepsSpare": draft_keeps_spare },
        "reopened": reopened,
        "newNames": names,
        "routeBridges": route_bridges,
        "join": { "before": join_before, "after": join_after },
        "shelf": shelf,
        "tray": { "role": tray_role, "brews": tray_brewed },
        "midBrew": { "bottle": bottle_mood, "publish": refused },
    }))
}
fn strand_clearance(rt: &Runtime<StudioApp>) -> f32 {
    let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
    let mut nearest = f32::MAX;
    for (id, points) in magicklala_canvas::cauldron::pour_paths(&rt.state.canvas, &plan) {
        for point in points.iter().skip(11) {
            for card in plan.cards.iter().filter(|card| card.role != magicklala_canvas::BrewRole::Bottle) {
                let r = card.rect;
                if card.id == id && point[1] <= r[1] + r[3] {
                    continue;
                }
                if point[1] < r[1] || point[1] > r[1] + r[3] {
                    continue;
                }
                let gap = if point[0] < r[0] {
                    r[0] - point[0]
                } else if point[0] > r[0] + r[2] {
                    point[0] - r[0] - r[2]
                } else {
                    -1.0
                };
                nearest = nearest.min(gap);
            }
        }
    }
    (nearest * 10.0).round() / 10.0
}

fn fit_and_wording(db_path: &Path) -> Result<serde_json::Value, String> {
    let sized = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0);
        rt
    };
    let dozen = sized(dozen_ingredient_spell(), (960.0, 640.0));
    let clearance = strand_clearance(&dozen);

    let mut rings = Vec::new();
    for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
        let empty = sized(SpellGraph::new("New Potion"), size);
        let plan = magicklala_canvas::brew_plan(&empty.state.canvas, empty.state.canvas_size());
        let centre = plan.pot[1] + plan.pot[3] * 0.5;
        let switch = layout_rect(&empty, "mode-cauldron").ok_or("no mode switch")?;
        let board = layout_rect(&empty, "canvas").ok_or("no canvas")?;
        rings.push(((centre - plan.pour_radius()) - (switch[1] + switch[3] - board[1])).round());
    }

    let compact = sized(StudioApp::from_registry().spell.clone(), (960.0, 640.0));
    let formulas: Vec<String> = compact
        .state
        .canvas
        .nodes
        .iter()
        .filter(|card| {
            let full = compact.state.canvas.card_line(card);
            !text_lines_under(&compact, &format!("node-{}", card.id.0))
                .iter()
                .any(|(line, _)| line == full || line == magicklala_canvas::view::short_line(full))
        })
        .map(|card| compact.state.canvas.card_line(card).to_owned())
        .collect();
    let mut measure = magicklala_studio::studio_text_system();
    let mut formulas_cut = Vec::new();
    for spell in [StudioApp::from_registry().spell.clone(), dozen_ingredient_spell()] {
        let rt = sized(spell, (960.0, 640.0));
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        for card in rt.state.canvas.nodes.iter().filter(|card| plan.card(&card.id).is_some_and(|placed| placed.role != magicklala_canvas::BrewRole::Bottle)) {
            let full = rt.state.canvas.card_line(card).to_owned();
            let Some((shown, rect)) = text_lines_under(&rt, &format!("node-{}", card.id.0))
                .into_iter()
                .find(|(line, _)| *line == full || line == magicklala_canvas::view::short_line(&full))
            else {
                continue;
            };
            let size = (11.2 / plan.scale).clamp(11.5, 13.0);
            let style = if rt.state.canvas.notes.contains_key(&card.id) { rune::TextStyle::new(size, 600.0) } else { rune::TextStyle::new(size, 500.0).mono() };
            let (natural, _) = measure.measure(&shown, style, 1.0);
            if natural * plan.scale > rect[2] + 1.0 {
                formulas_cut.push(shown.clone());
            }
        }
    }

    let empty_log = texts_under(&mut sized(SpellGraph::new("New Potion"), (1280.0, 800.0)), "trace-strip");
    let support_log = texts_under(&mut sized(StudioApp::from_registry().spell.clone(), (1280.0, 800.0)), "trace-strip");

    let mut slow = slow_runtime(1_500, Duration::from_secs(600));
    click_at(&mut slow, "run-spell", 0.5)?;
    slow.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    click_at(&mut slow, "publish", 1.0)?;
    let mid_brew_error = slow.state.store_error.clone();
    live_finish(&mut slow)?;

    let mut no_db = headless(StudioApp::from_registry());
    click(&mut no_db, "publish")?;
    let no_db_error = no_db.state.store_error.clone();

    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.load_graph(tasting_spell());
    let mut shelf = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    shelf.frame(0.0);
    click(&mut shelf, "publish")?;
    click(&mut shelf, "tab-agents")?;
    shelf.frame(2.0);
    let shelf_texts = texts_under(&mut shelf, "left-rail");
    let theme_caption = texts_under(&mut shelf, "theme-toggle");

    let mut menu = headless(StudioApp::from_registry());
    let llm = NodeId::from("llm");
    menu.update(move |app, _| {
        app.canvas.select(Some(llm.clone()));
        let plan = magicklala_canvas::brew_plan(&app.canvas, app.canvas_size());
        app.canvas.context_card = plan.card(&llm).map(|card| card.rect);
        app.canvas.context_menu = Some([24.0, 456.0]);
    });
    menu.frame(1.0);
    let menu_rect = layout_rect(&menu, "context-menu").ok_or("no menu")?;
    let mut routed = runtime_for(example("webhook_router.json")?);
    routed.update(|app, _| app.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } }));
    click(&mut routed, "run-spell")?;
    live_finish(&mut routed)?;
    let lanes: serde_json::Map<String, serde_json::Value> = magicklala_canvas::view::grimoire_lanes(&routed.state.canvas)
        .into_iter()
        .filter_map(|(id, lane)| {
            routed.state.canvas.wires.iter().find(|wire| wire.id == id).map(|wire| (format!("{}->{}", wire.from_socket, wire.to_node.0), json!(lane)))
        })
        .collect();
    let plaque = layout_rect(&menu, "cauldron-plaque").ok_or("no plaque")?;

    Ok(json!({
        "strandClearance": clearance,
        "ringBelowSwitch": rings,
        "formulasMissingAt960": formulas,
        "formulasCutAt960": formulas_cut,
        "logs": { "empty": empty_log, "support": support_log },
        "midBrewPublishError": mid_brew_error,
        "noDbPublishError": no_db_error,
        "shelf": shelf_texts,
        "theme": theme_caption,
        "menuCoversPlaque": rects_touch(menu_rect, plaque),
        "untakenLanes": lanes,
    }))
}
fn chat_reply(rt: &mut Runtime<StudioApp>, text: &str) -> Result<String, String> {
    let text = text.to_owned();
    rt.update(move |app, _| {
        app.chat_input = rune::TextEdit::new(&text);
        app.send_chat();
    });
    live_finish(rt)?;
    Ok(rt.state.chat.iter().rev().find(|message| !message.from_user).map(|message| message.text.clone()).unwrap_or_default())
}

fn switch_to_saved(rt: &mut Runtime<StudioApp>, key: &str) -> Result<(), String> {
    rt.update(|app, _| {
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    click(rt, "tab-agents")?;
    rt.frame(1.0);
    click_at(rt, key, 1.5)?;
    rt.update(|app, _| app.confirm_switch());
    rt.frame(2.0);
    Ok(())
}

fn shelf_keeps(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config: serde_json::Value::Null,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.load_graph(tasting_spell());
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("echo"))));
    let aside_reply = chat_reply(&mut rt, "hi there")?;
    click(&mut rt, "save")?;
    switch_to_saved(&mut rt, "spell-tasting-agent")?;
    let reopened_role = role_of(&rt.state, "echo");
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("echo"))));
    click(&mut rt, "validate")?;
    let poured = json!({ "check": rt.state.validation.label(), "echoWires": wires_of(&rt.state, "echo") });
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("trigger"))));
    click(&mut rt, "save")?;
    switch_to_saved(&mut rt, "spell-tasting-agent")?;
    let reopened_trigger = role_of(&rt.state, "trigger");
    let runs_before = rt.state.runs;
    click(&mut rt, "run-spell")?;
    let shelved_trigger_brews = rt.state.runs - runs_before;

    let mut spare = tasting_spell();
    spare.name = "Spare Agent".to_owned();
    spare.nodes.push(place("spare_reply", "io/send_message", 3.0));
    rt.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(spare.clone());
    });
    click(&mut rt, "publish")?;
    let listed = switch_to_saved(&mut rt, "spell-spare-agent").is_ok();
    let published_only = json!({
        "listed": listed,
        "spare": listed && rt.state.spell.nodes.iter().any(|node| node.id.0 == "spare_reply"),
        "live": rt.state.active_release_version,
    });

    rt.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(StudioApp::from_registry().spell.clone());
    });
    click(&mut rt, "save")?;
    click(&mut rt, "publish")?;
    let _ = switch_to_saved(&mut rt, "spell-support-agent");
    let sample_copy = json!({ "releases": rt.state.releases.len(), "live": rt.state.active_release_version });

    let mut auto = headless(StudioApp::from_registry());
    let trigger = node_of_kind(&auto.state, "event/on_message").ok_or("no trigger")?;
    let shelved = NodeId::from(trigger.as_str());
    auto.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(shelved.clone())));
    auto.update(|app, _| {
        app.spawn("io/send_message");
    });
    let trigger_after_add = role_of(&auto.state, &trigger);

    let mut sub = StudioApp::from_registry().spell.clone();
    sub.nodes.push(NodeInstance {
        id: NodeId::from("call"),
        kind: "flow/subgraph".to_owned(),
        position: [0.0, 400.0],
        config: json!({ "spell_id": SpellId::new() }),
    });
    let mut sub_rt = runtime_for(sub);
    sub_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("call"))));
    click(&mut sub_rt, "validate")?;
    let shelved_subgraph = sub_rt.state.validation.label();

    let mut lonely = StudioApp::from_registry().spell.clone();
    let lonely_trigger = lonely.nodes.iter().find(|node| node.kind == "event/on_message").map(|node| node.id.clone()).ok_or("no trigger")?;
    lonely.nodes.push(place("lonely", "io/send_message", 3.0));
    lonely.wires.push(Wire {
        id: WireId::from("lonely_words"),
        from_node: lonely_trigger,
        from_socket: "content".to_owned(),
        to_node: NodeId::from("lonely"),
        to_socket: "content".to_owned(),
    });
    let lonely_rt = runtime_for(lonely);
    let unreachable_role = role_of(&lonely_rt.state, "lonely");

    let mut thinker = SpellGraph::new("Thinker");
    thinker.nodes = vec![place("trigger", "event/on_message", 0.0), place("think", "ai/generate_text", 1.0), place("reply", "io/send_message", 2.0)];
    thinker.wires = vec![
        wire("w1", ("trigger", "out"), ("think", "in")),
        wire("w2", ("trigger", "content"), ("think", "prompt")),
        wire("w3", ("think", "out"), ("reply", "in")),
        wire("w4", ("think", "text"), ("reply", "content")),
    ];
    let mut thinker_rt = runtime_for(thinker);
    thinker_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("think"))));
    let model_pulled_reply = chat_reply(&mut thinker_rt, "what is brewing")?;

    let mut twins = SpellGraph::new("Twin Triggers");
    twins.nodes = vec![place("first", "event/on_message", 0.0), place("second", "event/on_start", 0.0), place("reply", "io/send_message", 2.0)];
    twins.wires = vec![
        wire("w1", ("first", "out"), ("reply", "in")),
        wire("w2", ("second", "out"), ("reply", "in")),
        wire("w3", ("first", "content"), ("reply", "content")),
    ];
    let mut twins_rt = runtime_for(twins);
    twins_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("second"))));
    twins_rt.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("second"))));
    let second_restored = twins_rt.state.spell.wires.iter().any(|wire| wire.from_node.0 == "second" && wire.to_node.0 == "reply");

    Ok(json!({
        "asideReply": aside_reply,
        "reopenedRole": reopened_role,
        "poured": poured,
        "reopenedTrigger": reopened_trigger,
        "shelvedTriggerBrews": shelved_trigger_brews,
        "publishedOnly": published_only,
        "sampleCopy": sample_copy,
        "triggerAfterAdd": trigger_after_add,
        "shelvedSubgraph": shelved_subgraph,
        "unreachableRole": unreachable_role,
        "modelPulledReply": model_pulled_reply,
        "secondRestored": second_restored,
    }))
}
fn shelf_exact(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let mut exact = SpellGraph::new("Exact Agent");
    exact.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("once", "flow/do_once", 1.0, serde_json::Value::Null),
        place("fill", "text/template", 1.0, json!({ "template": "Hi {{person}}" })),
        place("reply", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    exact.wires = vec![
        wire("w1", ("trigger", "out"), ("once", "in")),
        wire("w2", ("once", "out"), ("reply", "in")),
        wire("w3", ("trigger", "content"), ("fill", "person")),
        wire("w4", ("fill", "text"), ("reply", "content")),
    ];
    let mut rt = studio_at(db_path)?;
    rt.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(exact.clone());
    });
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("fill"))));
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("once"))));
    click(&mut rt, "save")?;
    let pour_back = |rt: &mut Runtime<StudioApp>| -> Result<serde_json::Value, String> {
        rt.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("once"))));
        rt.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("fill"))));
        let mut reply = feeders_of(&rt.state, "reply");
        reply.sort();
        Ok(json!({ "reply": reply, "fill": feeders_of(&rt.state, "fill"), "said": chat_reply(rt, "Ada")? }))
    };
    let mut restarted = studio_at(db_path)?;
    let restart = json!({
        "once": role_of(&restarted.state, "once"),
        "fill": role_of(&restarted.state, "fill"),
        "pouredBack": pour_back(&mut restarted)?,
    });
    let mut picked = studio_at(db_path)?;
    switch_to_saved(&mut picked, "spell-exact-agent")?;
    let from_potions = json!({ "once": role_of(&picked.state, "once"), "pouredBack": pour_back(&mut picked)? });

    let mut pair = SpellGraph::new("Pair Agent");
    pair.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("first", "time/delay", 1.0, json!({ "ms": 0 })),
        place("second", "time/delay", 2.0, json!({ "ms": 0 })),
        place("reply", "io/send_message", 3.0, serde_json::Value::Null),
    ];
    pair.wires = vec![
        wire("p1", ("trigger", "out"), ("first", "in")),
        wire("p2", ("first", "out"), ("second", "in")),
        wire("p3", ("second", "out"), ("reply", "in")),
        wire("p4", ("trigger", "content"), ("reply", "content")),
    ];
    let mut pair_rt = runtime_for(pair);
    pair_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("first"))));
    pair_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("second"))));
    pair_rt.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("first"))));
    let pair_second = role_of(&pair_rt.state, "second");

    let mut sub = StudioApp::from_registry().spell.clone();
    sub.nodes.push(place("call", "flow/subgraph", 0.0, json!({ "spell_id": SpellId::new(), "release_version": 0 })));
    let mut sub_rt = runtime_for(sub);
    sub_rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("call"))));
    click(&mut sub_rt, "validate")?;
    let shelved_bad_version = sub_rt.state.validation.label();

    let mut mute = SpellGraph::new("Mute Agent");
    mute.nodes = vec![place("trigger", "event/on_message", 0.0, serde_json::Value::Null), place("reply", "io/send_message", 1.0, serde_json::Value::Null)];
    mute.wires = vec![wire("m1", ("trigger", "content"), ("reply", "content"))];
    let mute_rt = runtime_for(mute);
    let mute_reply = role_of(&mute_rt.state, "reply");

    let mut gaps = SpellGraph::new("Gappy Agent");
    gaps.nodes = vec![place("trigger", "event/on_message", 0.0, serde_json::Value::Null)];
    for index in 0..4 {
        gaps.nodes.push(place(&format!("say_{index}"), "io/send_message", 1.0 + index as f32, serde_json::Value::Null));
    }
    let chain: Vec<String> = gaps.nodes.iter().map(|node| node.id.0.clone()).collect();
    for (index, link) in chain.windows(2).enumerate() {
        gaps.wires.push(wire(&format!("g{index}"), (&link[0], "out"), (&link[1], "in")));
    }
    let mut gaps_rt = runtime_for(gaps);
    click(&mut gaps_rt, "validate")?;
    gaps_rt.frame(1.0);
    let more = click(&mut gaps_rt, "log-more-issues").is_ok();
    gaps_rt.frame(1.0);
    let issue_rows = (0..64).take_while(|index| layout_rect(&gaps_rt, &format!("trace-issue-{index}")).is_some()).count();
    let issues_total = gaps_rt.state.trace.iter().filter(|line| line.kind == "RunRejected").count();
    gaps_rt.update(|app, _| app.canvas.select(None));
    let opened = click(&mut gaps_rt, "trace-issue-0").is_ok() && gaps_rt.state.canvas.selected_card().is_some();

    Ok(json!({
        "restart": restart,
        "fromPotions": from_potions,
        "pairSecond": pair_second,
        "shelvedBadVersion": shelved_bad_version,
        "muteReply": mute_reply,
        "moreIssues": { "chip": more, "rows": issue_rows, "total": issues_total, "opensIngredient": opened },
    }))
}
fn cauldron_truths() -> Result<serde_json::Value, String> {
    let open = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let disabled_labels = |rt: &Runtime<StudioApp>| -> Vec<String> {
        rt.accessibility_tree().nodes.iter().filter(|(_, node)| node.is_disabled()).filter_map(|(_, node)| node.label().map(str::to_owned)).collect()
    };
    let mut through_cards = Vec::new();
    let mut sharp_turns = Vec::new();
    let mut off_tap = Vec::new();
    let mut flank_after = Vec::new();
    let spells = [
        ("support", StudioApp::from_registry().spell.clone()),
        ("router", example("webhook_router.json")?),
        ("research", example("parallel_research.json")?),
        ("dozen", dozen_ingredient_spell()),
    ];
    for (name, spell) in spells {
        for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
            let rt = open(spell.clone(), size);
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            let pot = plan.pot;
            let bottles: Vec<&magicklala_canvas::BrewCard> = plan.with_role(magicklala_canvas::BrewRole::Bottle).collect();
            for (index, points) in magicklala_canvas::cauldron::ribbon_paths(&plan).iter().enumerate() {
                let own = bottles.get(index).map(|card| card.id.clone());
                let start = points.first().copied().unwrap_or_default();
                if start[0] < pot[0] + pot[2] * 0.75 || start[1] < pot[1] + pot[3] * 0.5 {
                    off_tap.push(format!("{name}@{}: ribbon {index} starts at {:?}", size.0, start));
                }
                let steps: Vec<[f32; 2]> =
                    points.windows(2).map(|pair| [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]]).filter(|step| step[0].hypot(step[1]) > 0.01).collect();
                let sharpest = steps
                    .windows(2)
                    .map(|pair| {
                        let dot = pair[0][0] * pair[1][0] + pair[0][1] * pair[1][1];
                        (dot / (pair[0][0].hypot(pair[0][1]) * pair[1][0].hypot(pair[1][1]))).clamp(-1.0, 1.0).acos().to_degrees()
                    })
                    .fold(0.0_f32, f32::max);
                if sharpest > 75.0 {
                    sharp_turns.push(format!("{name}@{}: ribbon {index} turns {}deg", size.0, sharpest.round()));
                }
                let body = &points[..points.len().saturating_sub(points.len() / 8)];
                for card in plan.cards.iter().filter(|card| Some(&card.id) != own.as_ref()) {
                    if body.iter().any(|point| inside(card.rect, [point[0], point[1], 0.0, 0.0])) {
                        through_cards.push(format!("{name}@{}: ribbon {index} crosses {}", size.0, card.id.0));
                    }
                }
            }
        }
    }
    for action in ["validate", "run-spell"] {
        let mut rt = open(dozen_ingredient_spell(), (960.0, 640.0));
        click(&mut rt, action)?;
        live_finish(&mut rt)?;
        rt.frame(3.0);
        flank_after.push(magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size()).flank);
    }

    let mut router = open(example("webhook_router.json")?, (1280.0, 800.0));
    click(&mut router, "run-spell")?;
    live_finish(&mut router)?;
    router.frame(3.0);
    let mut legends = vec![router.find("canvas-legend").is_some()];
    click(&mut router, "mode-grimoire")?;
    router.frame(2.0);
    legends.push(router.find("canvas-legend").is_some());
    let mut hexed = open(StudioApp::from_registry().spell.clone(), (1280.0, 800.0));
    hexed.update(|app, _| {
        app.canvas.wires.retain(|wire| !(wire.to_node.0 == "recall" && wire.to_socket == "query"));
        app.sync_spell();
    });
    click(&mut hexed, "validate")?;
    hexed.frame(2.0);
    legends.push(hexed.find("canvas-legend").is_some());

    let mut shelf = open(StudioApp::from_registry().spell.clone(), (1280.0, 800.0));
    let memory = node_of_kind(&shelf.state, "memory/search").ok_or("no memory step")?;
    let parked = NodeId::from(memory.as_str());
    shelf.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(parked.clone())));
    shelf.frame(2.0);
    let shelf_words: Vec<String> = text_lines_under(&shelf, "aside-shelf").into_iter().map(|(line, _)| line).filter(|line| line == "Set aside").collect();
    let plank = shelf.find("aside-plank").is_some();
    let mut shelf = open(dozen_ingredient_spell(), (960.0, 640.0));
    let spare = shelf.state.canvas.nodes[3].id.clone();
    shelf.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(spare.clone())));
    for _ in 0..3 {
        click(&mut shelf, "zoom-out")?;
    }
    shelf.frame(2.0);
    let drawn = |key: &str| shelf.presented(key).map(|(rect, _)| rect);
    let plaque = drawn("cauldron-plaque").ok_or("no plaque")?;
    let mut plaque_hits: Vec<String> = shelf
        .state
        .canvas
        .nodes
        .iter()
        .filter_map(|card| drawn(&format!("node-{}", card.id.0)).filter(|rect| rects_touch(plaque, *rect)).map(|_| card.id.0.clone()))
        .collect();
    if drawn("aside-shelf").is_some_and(|rect| rects_touch(plaque, rect)) {
        plaque_hits.push("aside-shelf".to_owned());
    }

    let mut idle = open(StudioApp::from_registry().spell.clone(), (1280.0, 800.0));
    let trigger = NodeId::from(node_of_kind(&idle.state, "event/on_message").ok_or("no trigger")?.as_str());
    idle.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(trigger.clone())));
    idle.frame(1.0);
    let brew_disabled = disabled_labels(&idle).iter().any(|label| label.starts_with("Brew"));
    let ids: Vec<NodeId> = idle.state.canvas.nodes.iter().map(|card| card.id.clone()).collect();
    idle.update(move |app, _| {
        for id in &ids {
            app.toss(magicklala_canvas::Toss::Aside(id.clone()));
        }
    });
    idle.frame(1.0);
    let empty_mood = format!("{:?}", magicklala_canvas::view::cauldron_mood(&idle.state.canvas));

    let mut tasted = runtime_for(tasting_spell());
    type_chat(&mut tasted, "What did Finance ask for Q3?")?;
    tasted.frame(3.0);
    let stand_words: Vec<String> = all_texts(&tasted);

    let mut twin_waits = SpellGraph::new("Twin Waits");
    let at = |id: &str, kind: &str, x: f32, y: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x, y],
        config,
    };
    twin_waits.nodes = vec![
        at("trigger", "event/on_message", 0.0, 0.0, serde_json::Value::Null),
        at("fork", "flow/split", 260.0, 0.0, serde_json::Value::Null),
        at("left", "time/delay", 520.0, -120.0, json!({ "ms": 40 })),
        at("right", "time/delay", 520.0, 120.0, json!({ "ms": 40 })),
        at("join", "flow/wait_for_all", 780.0, 0.0, serde_json::Value::Null),
        at("reply", "io/send_message", 1040.0, 0.0, serde_json::Value::Null),
    ];
    let link = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    twin_waits.wires = vec![
        link("t1", ("trigger", "out"), ("fork", "in")),
        link("t2", ("fork", "out_0"), ("left", "in")),
        link("t3", ("fork", "out_1"), ("right", "in")),
        link("t4", ("left", "out"), ("join", "in")),
        link("t5", ("right", "out"), ("join", "in")),
        link("t6", ("join", "out"), ("reply", "in")),
        link("t7", ("trigger", "content"), ("reply", "content")),
    ];
    let mut timed = runtime_for(twin_waits);
    chat_reply(&mut timed, "twins")?;
    timed.frame(2.0);
    let total = timed.state.trace.last().map_or(0, |line| line.at_ms);
    let total_label = if total == 0 { "<1ms".to_owned() } else { format!("{total}ms") };
    let plaque_line = text_lines_under(&timed, "cauldron-plaque").into_iter().map(|(line, _)| line).collect::<Vec<_>>().join(" ");

    let mut far = open(dozen_ingredient_spell(), (960.0, 640.0));
    for _ in 0..10 {
        let _ = click(&mut far, "zoom-out");
    }
    far.frame(1.0);
    let far_scale = magicklala_canvas::brew_plan(&far.state.canvas, far.state.canvas_size()).scale;

    let mut routed = StudioApp::with_spell(example("webhook_router.json")?, registry_specs());
    routed.size = (1280.0, 800.0);
    routed.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } });
    routed.validate();
    routed.run_spell();
    routed.wait_background(SETTLE);
    let mut routed = Runtime::new(routed, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    routed.frame(2.0);
    let routed_total = routed.state.trace.last().map_or(0, |line| line.at_ms);
    let routed_total = if routed_total == 0 { "<1ms".to_owned() } else { format!("{routed_total}ms") };
    let routed_plaque = text_lines_under(&routed, "cauldron-plaque").into_iter().map(|(line, _)| line).collect::<Vec<_>>().join(" ");

    Ok(json!({
        "sharpTurns": sharp_turns,
        "plaqueTime": [{ "plaque": plaque_line, "total": total_label }, { "plaque": routed_plaque.clone(), "total": routed_total }],
        "farScale": far_scale,
        "routedPlaque": routed_plaque,
        "throughCards": through_cards,
        "offTap": off_tap,
        "flankAfter": flank_after,
        "legends": legends,
        "shelfWords": shelf_words,
        "plank": plank,
        "plaqueHits": plaque_hits,
        "brewDisabled": brew_disabled,
        "emptyMood": empty_mood,
        "standHasTest": stand_words.iter().any(|line| line == "Test"),
    }))
}
fn shot(args: &[String]) -> Result<serde_json::Value, String> {
    let name = args.get(1).ok_or("missing spell")?;
    let number = |index: usize| args.get(index).and_then(|value| value.parse::<f32>().ok()).ok_or("missing size");
    let size = (number(2)?, number(3)?);
    let out = PathBuf::from(args.get(4).ok_or("missing output")?);
    let spell = match name.as_str() {
        "support" => StudioApp::from_registry().spell.clone(),
        "dozen" => dozen_ingredient_spell(),
        "kitchen" => crowded_kitchen(),
        other => example(other)?,
    };
    let mut app = StudioApp::with_spell(spell, registry_specs());
    app.size = size;
    let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
    rt.frame(0.0);
    let asked = |action: &str| args.iter().skip(5).any(|given| given == action);
    if asked("dark") {
        rt.update(|app, _| app.toggle_theme());
    }
    if asked("pantry") {
        click(&mut rt, "expand-nodes")?;
        rt.frame(2.0);
    }
    if asked("brew") {
        click(&mut rt, "run-spell")?;
        live_finish(&mut rt)?;
        rt.frame(3.0);
    }
    if asked("grimoire") {
        click(&mut rt, "mode-grimoire")?;
        rt.frame(4.0);
    }
    if asked("menu")
        && let Some(first) = magicklala_canvas::recipe_order(&rt.state.canvas).first().cloned()
        && let Some(card) = layout_rect(&rt, &format!("node-{}", first.0))
    {
        right_click(&mut rt, (card[0] + card[2] * 0.5, card[1] + card[3] * 0.5));
        rt.frame(4.0);
    }
    let app = std::mem::replace(&mut rt.state, StudioApp::from_registry());
    render_headless_snapshot_state(&out, app).map_err(|error| error.to_string())?;
    Ok(json!({ "out": out }))
}

fn quiet_corners(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut saved = studio_at(db_path)?;
    saved.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(StudioApp::from_registry().spell.clone());
    });
    let memory = NodeId::from(node_of_kind(&saved.state, "memory/search").ok_or("no memory step")?.as_str());
    let parked = memory.clone();
    saved.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(parked.clone())));
    click(&mut saved, "save")?;
    saved.update(|app, _| {
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    click(&mut saved, "tab-agents")?;
    saved.frame(1.0);
    let potion_row: Vec<String> = text_lines_under(&saved, "spell-support-agent").into_iter().map(|(line, _)| line).collect();

    let mut slow_spell = delay_spell(1_500);
    slow_spell.nodes.push(NodeInstance {
        id: NodeId::from("muse"),
        kind: "ai/generate_text".to_owned(),
        position: [280.0, 220.0],
        config: serde_json::Value::Null,
    });
    slow_spell.variables.push(magicklala_graph::VariableDef { name: "mood".to_owned(), ty: SocketType::String, default: None, exposed: false });
    let mut app = StudioApp::with_spell(slow_spell, registry_specs());
    app.run_timeout = Duration::from_secs(600);
    let mut slow = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    slow.frame(0.0);
    slow.update(|app, _| app.canvas.select(Some(NodeId::from("muse"))));
    slow.update(|app, _| {
        app.validate();
    });
    let check_before = slow.state.validation.label();
    click_at(&mut slow, "run-spell", 0.5)?;
    slow.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    slow.update(|app, _| {
        app.toggle_model_menu();
        app.toggle_var_type_menu_at(0);
        app.request_restore_release(1);
        app.pantry_drag_move("io/send_message", [600.0, 300.0]);
    });
    let _ = click(&mut slow, "validate");
    let (vx, vy) = slow.find("node-wait").ok_or("no wait vial")?;
    slow.pointer_move(vx, vy);
    slow.pointer_down(vx, vy);
    for step in 1..=6 {
        slow.pointer_move(vx, vy + step as f32 * 40.0);
        slow.frame(0.0);
    }
    let tray_mid_brew = slow.find("aside-tray").is_some();
    slow.pointer_up(vx, vy + 240.0);
    slow.frame(0.0);
    let mid_brew = json!({
        "running": slow.state.is_running(),
        "modelMenu": slow.state.model_menu_open,
        "typeMenu": slow.state.open_var_type,
        "restore": slow.state.pending_restore_version,
        "pantryDrag": slow.state.pantry_drag.is_some(),
        "tray": tray_mid_brew,
        "checkUnchanged": slow.state.validation.label() == check_before,
        "checkDisabled": slow.accessibility_tree().nodes.iter().any(|(_, node)| node.is_disabled() && node.label() == Some("Check")),
    });

    let mut words = headless(StudioApp::from_registry());
    words.update(|app, _| {
        app.add_variable();
    });
    let added = words.state.spell.variables.last().map(|variable| variable.name.clone()).unwrap_or_default();
    let removed = added.clone();
    words.update(move |app, _| {
        if let Some(index) = app.spell.variables.iter().position(|variable| variable.name == removed) {
            app.remove_variable_at(index);
        }
    });
    let banner = words.state.undo_banner.clone();
    let mut script = runtime_for(example("webhook_router.json")?);
    let rhai = NodeId::from(node_of_kind(&script.state, "code/rhai_script").ok_or("no script")?.as_str());
    script.update(move |app, _| app.canvas.select(Some(rhai.clone())));
    script.frame(1.0);
    let rune_caption = all_texts(&script).iter().any(|line| line == "Rhai rune");

    let mut undo = headless(StudioApp::from_registry());
    let shelved = memory.clone();
    undo.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(shelved.clone())));
    let selected = memory.clone();
    undo.update(move |app, _| {
        app.canvas.select(Some(selected.clone()));
        app.delete_selected_node();
        app.undo();
    });
    let undo_role = role_of(&undo.state, &memory.0);
    let undo_parked = undo.state.aside_wires.get(&memory).map_or(0, Vec::len);
    let poured = memory.clone();
    undo.update(move |app, _| app.toss(magicklala_canvas::Toss::IntoPot(poured.clone())));
    let undo_wires = wires_of(&undo.state, &memory.0);

    let mut vars = headless(StudioApp::from_registry());
    vars.update(|app, _| {
        app.add_variable();
    });
    let name = vars.state.spell.variables.last().map(|variable| variable.name.clone()).unwrap_or_default();
    let typed = name.clone();
    vars.update(move |app, _| {
        app.right_tab = RightTab::Variables;
        if let Some(index) = app.spell.variables.iter().position(|variable| variable.name == typed) {
            app.set_variable_type_at(index, SocketType::Int);
        }
        if let Some(edit) = app.var_default_edits.last_mut() {
            *edit = rune::TextEdit::new("abc");
        }
        app.sync_spell();
    });
    vars.frame(1.0);
    let index = vars.state.spell.variables.len() - 1;
    let flagged = vars.find(&format!("var-default-error-{index}")).is_some();
    let opened = name.clone();
    vars.update(move |app, _| {
        if let Some(index) = app.spell.variables.iter().position(|variable| variable.name == opened) {
            app.toggle_var_type_menu_at(index);
        }
    });
    vars.frame(1.0);
    let menu_row = vars.state.spell.variables.iter().position(|variable| variable.name == name).unwrap_or_default();
    let json_choice = text_lines_under(&vars, &format!("var-type-menu-{menu_row}")).iter().any(|(line, _)| line == "Json");
    let token = format!("{{{{{name}}}}}");
    let note = NodeId::from(node_of_kind(&vars.state, "ai/generate_text").ok_or("no model")?.as_str());
    let hinted = note.clone();
    vars.update(move |app, _| {
        if let Some(edit) = app.prompts.get_mut(&hinted) {
            *edit = rune::TextEdit::new(format!("use {token}"));
        }
        app.sync_spell();
        app.toss(magicklala_canvas::Toss::Aside(hinted.clone()));
    });
    let shelved_hint = vars.state.variable_binding_hint(&name).1;
    let getter = name.clone();
    vars.update(move |app, _| {
        let id = app.spawn("var/get");
        if let Some(id) = id
            && let Some(config) = app.configs.get_mut(&id)
        {
            *config = json!({ "name": getter });
        }
        app.sync_spell();
    });
    let getter_hint = vars.state.variable_binding_hint(&name).1;

    let mut drift = 0.0_f32;
    for spell in [dozen_ingredient_spell(), StudioApp::from_registry().spell.clone()] {
        let rt = runtime_for(spell);
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        for (_, points) in magicklala_canvas::cauldron::pour_paths(&rt.state.canvas, &plan) {
            for (start, end) in magicklala_canvas::cauldron::drip_hops(&points) {
                let middle = [(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5];
                let nearest = points.iter().map(|point| ((point[0] - middle[0]).powi(2) + (point[1] - middle[1]).powi(2)).sqrt()).fold(f32::MAX, f32::min);
                drift = drift.max(nearest);
            }
        }
    }

    let mut twins = runtime_for(example("parallel_research.json")?);
    let twin = twins
        .state
        .canvas
        .nodes
        .iter()
        .find(|card| twins.state.canvas.display_title(&card.id) == "Template · 1")
        .map(|card| card.id.clone())
        .ok_or("no twin")?;
    twins.update(move |app, _| {
        app.canvas.select(Some(twin.clone()));
        app.delete_selected_node();
    });
    let twin_banner = twins.state.undo_banner.clone();

    let mut moved = runtime_for(tasting_spell());
    type_chat(&mut moved, "hello")?;
    let first = moved.state.canvas.nodes[0].id.clone();
    moved.update(move |app, _| {
        if let Some(card) = app.canvas.card_mut(&first) {
            card.position[0] += 40.0;
        }
        app.sync_spell();
    });
    let lit_after_move = moved.state.canvas.last_brew.is_some();

    let mut grim = runtime_for(StudioApp::from_registry().spell.clone());
    click(&mut grim, "mode-grimoire")?;
    grim.frame(1.0);
    let target = grim.state.canvas.nodes[0].id.clone();
    let point = grim.find(&format!("node-{}", target.0)).ok_or("no grimoire card")?;
    right_click(&mut grim, point);
    let grim_menu: Vec<String> = all_texts(&grim).into_iter().filter(|line| line.contains("Grimoire") || line.contains("Cauldron")).collect();

    let mut streamer = runtime_for(dozen_ingredient_spell());
    let stream = NodeId::from(node_of_kind(&streamer.state, "ai/stream_text").ok_or("no stream step")?.as_str());
    streamer.update(move |app, _| {
        app.canvas.select(Some(stream.clone()));
        app.right_tab = RightTab::Inspector;
    });
    streamer.frame(1.0);
    let stream_model = streamer.find("model-picker").is_some();

    let mut taste = runtime_for(tasting_spell());
    taste.update(|app, _| app.size = (960.0, 640.0));
    taste.set_viewport((960.0, 640.0), 1.0);
    type_chat(&mut taste, "hello")?;
    type_chat(&mut taste, "and again")?;
    taste.update(|app, _| app.right_tab = RightTab::Playground);
    taste.frame(1.0);
    let divider = layout_rect(&taste, "tasting-divider").map(|rect| rect[2]);
    let panel = layout_rect(&taste, "inspector").map(|rect| rect[2]);

    let mut empty = headless(StudioApp::from_registry());
    empty.update(|app, _| {
        app.new_empty_spell();
        app.left_tab = LeftTab::Agents;
    });
    empty.frame(1.0);
    empty.frame(2.0);
    let starter = click(&mut empty, "starter-event-on_message").is_ok();
    let starter_nodes = empty.state.canvas.nodes.iter().map(|card| card.kind.clone()).collect::<Vec<_>>();

    Ok(json!({
        "potionRow": potion_row,
        "midBrew": mid_brew,
        "banner": banner,
        "runeCaption": rune_caption,
        "undo": { "role": undo_role, "parked": undo_parked, "wiresAfterPour": undo_wires },
        "variables": { "flagged": flagged, "json": json_choice, "shelvedHint": shelved_hint, "getterHint": getter_hint },
        "dripDrift": drift,
        "twinBanner": twin_banner,
        "litAfterMove": lit_after_move,
        "grimoireMenu": grim_menu,
        "streamModel": stream_model,
        "divider": { "width": divider, "panel": panel },
        "starter": { "clicked": starter, "nodes": starter_nodes },
    }))
}
fn honest_state(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let mut pair = SpellGraph::new("Neighbour Agent");
    pair.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("first", "time/delay", 1.0, json!({ "ms": 0 })),
        place("second", "time/delay", 2.0, json!({ "ms": 0 })),
        place("reply", "io/send_message", 3.0, serde_json::Value::Null),
    ];
    pair.wires = vec![
        wire("n1", ("trigger", "out"), ("first", "in")),
        wire("n2", ("first", "out"), ("second", "in")),
        wire("n3", ("second", "out"), ("reply", "in")),
        wire("n4", ("trigger", "content"), ("reply", "content")),
    ];
    let mut setup = studio_at(db_path)?;
    setup.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(pair.clone());
    });
    setup.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("first"))));
    setup.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("second"))));
    let shelved_reply = chat_reply(&mut setup, "solo")?;
    click(&mut setup, "save")?;
    let mut orders = Vec::new();
    for order in [["first", "second"], ["second", "first"]] {
        let mut rt = studio_at(db_path)?;
        for id in order {
            rt.update(move |app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from(id))));
        }
        let feeders: Vec<String> = feeders_of(&rt.state, "reply").into_iter().filter(|line| line.ends_with("->in")).collect();
        orders.push(json!({ "order": order, "flowIntoReply": feeders, "said": chat_reply(&mut rt, "once")? }));
    }

    let mut mute = SpellGraph::new("Cut Agent");
    mute.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("wait", "time/delay", 1.0, json!({ "ms": 0 })),
        place("reply", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    mute.wires = vec![wire("c2", ("wait", "out"), ("reply", "in")), wire("c3", ("trigger", "content"), ("reply", "content"))];
    let cut = runtime_for(mute);
    let cut_reply = role_of(&cut.state, "reply");

    let mut typed = studio_at(db_path)?;
    typed.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(StudioApp::from_registry().spell.clone());
        app.add_variable();
    });
    let last = typed.state.spell.variables.len() - 1;
    typed.update(move |app, _| {
        app.set_variable_type_at(last, SocketType::Int);
        if let Some(edit) = app.var_default_edits.get_mut(last) {
            *edit = rune::TextEdit::new("5");
        }
        app.sync_spell();
    });
    click(&mut typed, "save")?;
    typed.update(move |app, _| {
        if let Some(edit) = app.var_default_edits.get_mut(last) {
            *edit = rune::TextEdit::new("oops");
        }
        app.sync_spell();
    });
    click(&mut typed, "validate")?;
    let bad_check = typed.state.validation.label();
    let _ = click(&mut typed, "save");
    let bad_save = typed.state.draft_status.clone();
    let releases_before = typed.state.releases.len();
    typed.update(|app, _| {
        let _ = app.publish();
    });
    let bad_publish = typed.state.releases.len() - releases_before;
    typed.update(move |app, _| {
        app.add_variable();
        let fresh = app.spell.variables.len() - 1;
        app.set_variable_type_at(fresh, SocketType::Json);
    });
    let json_default = typed.state.spell.variables.last().and_then(|variable| variable.default.clone()).map(|value| format!("{value:?}"));

    let mut live = studio_at(db_path)?;
    live.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(tasting_spell());
    });
    click(&mut live, "publish")?;
    let echo = NodeId::from("echo");
    let parked = echo.clone();
    live.update(move |app, _| {
        app.toss(magicklala_canvas::Toss::Aside(parked.clone()));
        app.undo();
    });
    let after_undo = live.state.active_release_version;
    live.update(|app, _| {
        let _ = app.spawn("flow/subgraph");
        app.sync_spell();
    });
    let after_parked_sub = live.state.active_release_version;
    let count_before = live.state.releases.len();
    live.update(|app, _| {
        let _ = app.publish();
    });
    let duplicate = live.state.releases.len() - count_before;

    let mut mended = headless(StudioApp::from_registry());
    mended.update(|app, _| {
        app.canvas.wires.retain(|wire| !(wire.to_node.0 == "recall" && wire.to_socket == "query"));
        app.sync_spell();
    });
    click(&mut mended, "validate")?;
    let hexed_first = mended.state.validation.label();
    let trigger = NodeId::from(node_of_kind(&mended.state, "event/on_message").ok_or("no trigger")?.as_str());
    mended.update(move |app, _| {
        let _ = app.canvas.connect_sockets(&SocketRef::new(trigger.clone(), "content"), &SocketRef::new(NodeId::from("recall"), "query"));
        app.sync_spell();
    });
    let mended_state = json!({
        "before": hexed_first,
        "after": mended.state.validation.label(),
        "flagged": mended.state.canvas.nodes.iter().flat_map(|card| card.inputs.iter()).filter(|socket| socket.flagged || socket.mistyped).count(),
    });

    let mut once = SpellGraph::new("Once Agent");
    once.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("once", "flow/do_once", 1.0, serde_json::Value::Null),
        place("reply", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    once.wires = vec![
        wire("o1", ("trigger", "out"), ("once", "in")),
        wire("o2", ("once", "out"), ("reply", "in")),
        wire("o3", ("trigger", "content"), ("reply", "content")),
    ];
    let mut grim = runtime_for(once);
    grim.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("once"))));
    grim.update(|app, _| {
        app.canvas.mode = magicklala_canvas::WorkbenchMode::Grimoire;
        let _ = app.canvas.connect_sockets(&SocketRef::new(NodeId::from("trigger"), "out"), &SocketRef::new(NodeId::from("once"), "in"));
        let _ = app.canvas.connect_sockets(&SocketRef::new(NodeId::from("once"), "out"), &SocketRef::new(NodeId::from("reply"), "in"));
        app.sync_spell();
    });
    let mut grim_reply: Vec<String> = feeders_of(&grim.state, "reply").into_iter().filter(|line| line.ends_with("->in")).collect();
    grim_reply.sort();
    let mut shelf_grim = runtime_for(StudioApp::from_registry().spell.clone());
    let memory = NodeId::from(node_of_kind(&shelf_grim.state, "memory/search").ok_or("no memory")?.as_str());
    let marked = memory.clone();
    shelf_grim.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(marked.clone())));
    click(&mut shelf_grim, "mode-grimoire")?;
    shelf_grim.frame(1.0);
    let grim_badge = text_lines_under(&shelf_grim, &format!("node-{}", memory.0)).iter().any(|(line, _)| line == "Set aside");

    let mut tight = runtime_for(dozen_ingredient_spell());
    tight.update(|app, _| app.size = (960.0, 640.0));
    tight.set_viewport((960.0, 640.0), 1.0);
    click(&mut tight, "run-spell")?;
    live_finish(&mut tight)?;
    tight.frame(2.0);
    let model = NodeId::from(node_of_kind(&tight.state, "ai/generate_text").ok_or("no model")?.as_str());
    tight.update(move |app, _| {
        app.canvas.select(Some(model.clone()));
        app.right_tab = RightTab::Inspector;
    });
    tight.frame(1.0);
    let editor = tight.presented("prompt-editor").map(|(rect, _)| rect[1] + rect[3]);
    let panel = tight.presented("inspector").map(|(rect, _)| rect[1] + rect[3]);

    Ok(json!({
        "shelvedReply": shelved_reply,
        "orders": orders,
        "cutReply": cut_reply,
        "badDefault": { "check": bad_check, "save": bad_save, "published": bad_publish, "json": json_default },
        "live": { "afterUndo": after_undo, "afterParkedSub": after_parked_sub, "duplicate": duplicate },
        "mended": mended_state,
        "grimoire": { "reply": grim_reply, "badge": grim_badge },
        "editor": { "bottom": editor, "panel": panel },
    }))
}
fn drawn_truths() -> Result<serde_json::Value, String> {
    let open = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let near = |points: &[[f32; 2]], point: [f32; 2], gap: f32| points.iter().any(|other| (other[0] - point[0]).hypot(other[1] - point[1]) < gap);
    let mut shared_runs = Vec::new();
    let mut own_tag = Vec::new();
    let mut spigot = Vec::new();
    let mut pour_crossings = Vec::new();
    let spells = [
        ("support", StudioApp::from_registry().spell.clone()),
        ("router", example("webhook_router.json")?),
        ("research", example("parallel_research.json")?),
        ("dozen", dozen_ingredient_spell()),
    ];
    for (name, spell) in spells {
        for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
            let rt = open(spell.clone(), size);
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            let s = plan.scale;
            let pours = magicklala_canvas::cauldron::pour_paths(&rt.state.canvas, &plan);
            if plan.lanes.is_some() {
                for (index, (id, points)) in pours.iter().enumerate() {
                    let body = &points[..points.len() * 3 / 4];
                    for (other_id, other) in pours.iter().skip(index + 1) {
                        let other_body = &other[..other.len() * 3 / 4];
                        let touching = body.iter().filter(|point| near(other_body, **point, 1.5)).count();
                        if touching > 2 {
                            shared_runs.push(format!("{name}@{}: {} shares {touching} points with {}", size.0, id.0, other_id.0));
                        }
                    }
                }
            }
            for (id, points) in &pours {
                let body = &points[3..points.len().saturating_sub(points.len() / 6)];
                for card in plan.cards.iter().filter(|card| &card.id != id) {
                    let r = card.rect;
                    if body.iter().any(|point| point[0] > r[0] + 2.0 && point[0] < r[0] + r[2] - 2.0 && point[1] > r[1] + 2.0 && point[1] < r[1] + r[3] - 2.0) {
                        pour_crossings.push(format!("{name}@{}: {} pours through {}", size.0, id.0, card.id.0));
                    }
                }
            }
            let bottles: Vec<&magicklala_canvas::BrewCard> = plan.with_role(magicklala_canvas::BrewRole::Bottle).collect();
            for (index, ribbon) in magicklala_canvas::cauldron::ribbon_paths(&plan).iter().enumerate() {
                if let Some(card) = bottles.get(index) {
                    let r = card.rect;
                    let tag = [r[0] - 8.0 * s, r[1] + r[3] * 0.46, r[2] + 16.0 * s, r[3] * 0.54];
                    if ribbon.iter().any(|point| point[0] > tag[0] && point[0] < tag[0] + tag[2] && point[1] > tag[1] && point[1] < tag[1] + tag[3]) {
                        own_tag.push(format!("{name}@{}: ribbon {index} crosses its own tag", size.0));
                    }
                }
                if ribbon.len() > 3 {
                    let lead = [ribbon[2][0] - ribbon[0][0], ribbon[2][1] - ribbon[0][1]];
                    let tilt = lead[1].abs().atan2(lead[0].abs()).to_degrees();
                    if tilt > 20.0 {
                        spigot.push(format!("{name}@{}: ribbon {index} leaves the tap at {}deg", size.0, tilt.round()));
                    }
                }
            }
        }
    }

    let mut dozen = open(dozen_ingredient_spell(), (960.0, 640.0));
    click(&mut dozen, "run-spell")?;
    live_finish(&mut dozen)?;
    dozen.frame(3.0);
    let plan = magicklala_canvas::brew_plan(&dozen.state.canvas, dozen.state.canvas_size());
    let mut pills = 0;
    let mut pill_hits = Vec::new();
    for card in plan.cards.iter().filter(|card| card.mirrored) {
        let pill = dozen.presented(&format!("status-pill-{}", card.id.0)).map(|(rect, _)| rect);
        let badge = dozen.presented(&format!("step-badge-{}", card.id.0)).map(|(rect, _)| rect);
        if let (Some(pill), Some(badge)) = (pill, badge) {
            pills += 1;
            if rects_touch(pill, badge) {
                pill_hits.push(card.id.0.clone());
            }
        }
    }

    let light = magicklala_canvas::HearthPalette::for_theme(true);
    let dark = magicklala_canvas::HearthPalette::for_theme(false);
    let borders = [
        contrast(light.untaken_border(), light.paper),
        contrast(dark.untaken_border(), dark.paper),
        contrast(light.hexed_glass_stroke(), light.paper),
        contrast(dark.hexed_glass_stroke(), dark.paper),
    ];

    let research = open(example("parallel_research.json")?, (960.0, 640.0));
    let twin_names: Vec<String> =
        research.state.canvas.nodes.iter().map(|card| research.state.canvas.display_title(&card.id)).filter(|title| title.contains(" · ")).collect();

    let mut hook = open(example("webhook_router.json")?, (1280.0, 800.0));
    click(&mut hook, "run-spell")?;
    live_finish(&mut hook)?;
    let hook_brew = format!("{:?}", hook.state.canvas.last_brew);

    Ok(json!({
        "sharedRuns": shared_runs,
        "ownTag": own_tag,
        "spigot": spigot,
        "pourCrossings": pour_crossings,
        "pills": { "found": pills, "hits": pill_hits },
        "borders": borders,
        "twinNames": twin_names,
        "hookBrew": hook_brew,
    }))
}
fn segment_gap(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let along = [b[0] - a[0], b[1] - a[1]];
    let length = along[0] * along[0] + along[1] * along[1];
    let share = if length <= f32::EPSILON { 0.0 } else { (((point[0] - a[0]) * along[0] + (point[1] - a[1]) * along[1]) / length).clamp(0.0, 1.0) };
    (point[0] - a[0] - along[0] * share).hypot(point[1] - a[1] - along[1] * share)
}

fn plain_sight() -> Result<serde_json::Value, String> {
    let mut text = magicklala_studio::studio_text_system();
    let open = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let brewed = |spell: SpellGraph, size: (f32, f32)| -> Result<Runtime<StudioApp>, String> {
        let mut rt = open(spell, size);
        click(&mut rt, "run-spell")?;
        live_finish(&mut rt)?;
        rt.frame(3.0);
        Ok(rt)
    };
    let samples = || -> Result<Vec<(&'static str, SpellGraph)>, String> {
        Ok(vec![
            ("support", StudioApp::from_registry().spell.clone()),
            ("router", example("webhook_router.json")?),
            ("research", example("parallel_research.json")?),
            ("dozen", dozen_ingredient_spell()),
        ])
    };
    let sizes = [(1280.0_f32, 800.0_f32), (960.0, 640.0)];
    let fits = |text: &mut rune::text::TextSystem, line: &str, rect: [f32; 4], size: f32, weight: f32| {
        let (natural, line_h) = text.measure(line, rune::TextStyle::new(size, weight), 1.0);
        let rows = (rect[3] / line_h).round().max(1.0);
        natural <= rect[2] * rows + 2.0
    };

    let mut ribbon_shares = Vec::new();
    for (name, spell) in samples()? {
        for size in sizes {
            let rt = brewed(spell.clone(), size)?;
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            let ribbons = if magicklala_canvas::cauldron::rack_planks(&plan).is_some() { Vec::new() } else { magicklala_canvas::cauldron::ribbon_paths(&plan) };
            let body = |ribbon: &Vec<[f32; 2]>| -> Vec<[f32; 2]> {
                let tap = ribbon.first().copied().unwrap_or_default();
                let mouth = ribbon.last().copied().unwrap_or_default();
                ribbon
                    .iter()
                    .copied()
                    .filter(|point| {
                        (point[0] - tap[0]).hypot(point[1] - tap[1]) > 40.0 * plan.scale && (point[0] - mouth[0]).hypot(point[1] - mouth[1]) > 12.0 * plan.scale
                    })
                    .collect()
            };
            for (index, ribbon) in ribbons.iter().enumerate() {
                for (other_index, other) in ribbons.iter().enumerate().skip(index + 1) {
                    let (mine, theirs) = (body(ribbon), body(other));
                    let touching = mine.iter().filter(|point| theirs.windows(2).any(|pair| segment_gap(**point, pair[0], pair[1]) < 8.0 * plan.scale)).count();
                    if touching > 3 {
                        ribbon_shares.push(format!("{name}@{}: ribbons {index} and {other_index} share {touching} points", size.0));
                    }
                }
            }
        }
    }

    let mut logs = Vec::new();
    for size in sizes {
        let rt = brewed(example("parallel_research.json")?, size)?;
        let strip = layout_rect(&rt, "trace-strip").ok_or("no log")?;
        let lines = text_lines_under(&rt, "trace-strip");
        let result = lines.iter().any(|(line, rect)| line.starts_with("Failed") && inside(strip, *rect) && rect[0] + rect[2] <= strip[0] + strip[2]);
        let clipped: Vec<String> = lines.iter().filter(|(_, rect)| rect[0] + rect[2] > strip[0] + strip[2] + 0.5).map(|(line, _)| line.clone()).collect();
        logs.push(json!({ "spell": "research", "width": size.0, "resultVisible": result, "clipped": clipped }));
    }
    let mut expanded = brewed(example("webhook_router.json")?, (960.0, 640.0))?;
    expanded.update(|app, _| app.trace_expanded = true);
    expanded.frame(4.0);
    let strip = layout_rect(&expanded, "trace-strip").ok_or("no log")?;
    let lines = text_lines_under(&expanded, "trace-strip");
    let clipped: Vec<String> = lines.iter().filter(|(_, rect)| rect[0] + rect[2] > strip[0] + strip[2] + 0.5).map(|(line, _)| line.clone()).collect();
    let result = lines.iter().any(|(line, rect)| line == "Brewed" && inside(strip, *rect));
    logs.push(json!({ "spell": "router expanded", "width": 960.0, "resultVisible": result, "clipped": clipped }));

    let mut failing = brewed(example("parallel_research.json")?, (960.0, 640.0))?;
    let failed = failing.state.canvas.nodes.iter().find(|card| card.status == ExecutionStatus::Failed).map(|card| card.id.clone()).ok_or("nothing failed")?;
    failing.update(move |app, _| {
        app.canvas.select(Some(failed.clone()));
        app.right_tab = RightTab::Inspector;
    });
    failing.frame(4.0);
    let scroll = layout_rect(&failing, "inspector-scroll").ok_or("no inspector")?;
    let unplug = layout_rect(&failing, "prompt-unplug");
    let failed_inspector = json!({ "typeInsteadInView": unplug.is_some_and(|rect| rect[1] + rect[3] <= scroll[1] + scroll[3] + 0.5) });

    let mut summary = brewed(example("webhook_router.json")?, (960.0, 640.0))?;
    summary.update(|app, _| {
        app.canvas.select(None);
        app.right_tab = RightTab::Trace;
    });
    summary.frame(4.0);
    let step_names: Vec<String> = summary
        .state
        .trace
        .iter()
        .filter(|line| line.kind == "NodeFinished")
        .filter_map(|line| line.node.clone())
        .map(|id| summary.state.canvas.display_title(&id))
        .collect();
    let shown = text_lines_under(&summary, "inspector-scroll");
    let cut_steps: Vec<String> =
        step_names.iter().filter(|name| !shown.iter().any(|(line, rect)| line == *name && fits(&mut text, line, *rect, 12.0, 600.0))).cloned().collect();
    let trace_words: Vec<String> = shown.iter().map(|(line, _)| line.clone()).collect();
    summary.update(|app, _| app.right_tab = RightTab::Inspector);
    summary.frame(5.0);
    let recipe_words: Vec<String> = text_lines_under(&summary, "inspector-scroll").into_iter().map(|(line, _)| line).collect();
    let summary_report = json!({ "cut": cut_steps, "recipeRepeatsTrace": recipe_words == trace_words });

    let mut untaken_after_fail = Vec::new();
    let mut socket_suffixes = Vec::new();
    let mut pill_overlaps = Vec::new();
    for (name, spell) in samples()? {
        for size in sizes {
            let rt = brewed(spell.clone(), size)?;
            let words = all_texts(&rt);
            if format!("{:?}", rt.state.canvas.last_brew) != "Some(Succeeded)" && words.iter().any(|line| line.starts_with("Untaken")) {
                untaken_after_fail.push(format!("{name}@{}", size.0));
            }
            socket_suffixes.extend(
                words
                    .iter()
                    .filter(|line| line.starts_with("Untaken ·") && (line.ends_with("· out") || line.contains("out_")))
                    .map(|line| format!("{name}@{}: {line}", size.0)),
            );
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            for card in &plan.cards {
                let Some((pill, _)) = rt.presented(&format!("status-pill-{}", card.id.0)) else { continue };
                for other in plan.cards.iter().filter(|other| other.id != card.id) {
                    let Some((r, _)) = rt.presented(&format!("node-{}", other.id.0)) else { continue };
                    let overlap_w = (pill[0] + pill[2]).min(r[0] + r[2]) - pill[0].max(r[0]);
                    let overlap_h = (pill[1] + pill[3]).min(r[1] + r[3]) - pill[1].max(r[1]);
                    if overlap_w > 1.0 && overlap_h > 1.0 {
                        pill_overlaps.push(format!("{name}@{}: {} pill on {}", size.0, card.id.0, other.id.0));
                    }
                }
            }
        }
    }

    let light = magicklala_canvas::HearthPalette::for_theme(true);
    let dark = magicklala_canvas::HearthPalette::for_theme(false);
    let categories = ["events", "ai", "memory", "knowledge", "flow", "io", "code", "text", "math", "json", "variables", "time", "logic", "array"];
    let weak_borders: Vec<String> = categories
        .iter()
        .flat_map(|category| {
            [(true, light.paper), (false, dark.paper)].into_iter().filter_map(move |(is_light, ground)| {
                let ratio = contrast(magicklala_canvas::hearth::category_tone(category, is_light).border, ground);
                (ratio < 3.0).then(|| format!("{category} {}: {ratio:.2}", if is_light { "sunlit" } else { "midnight" }))
            })
        })
        .collect();
    let broth = [contrast(light.broth_mid, light.paper), contrast(light.broth_mid, light.shelf)];

    let dozen = open(dozen_ingredient_spell(), (1280.0, 800.0));
    let plan = magicklala_canvas::brew_plan(&dozen.state.canvas, dozen.state.canvas_size());
    let mut spilling = Vec::new();
    for card in plan.cards.iter().filter(|card| card.role != magicklala_canvas::BrewRole::Bottle) {
        let Some((r, _)) = dozen.presented(&format!("node-{}", card.id.0)) else { continue };
        let inset = 3.0 * plan.scale;
        for (line, rect) in text_lines_under(&dozen, &format!("node-{}", card.id.0)) {
            if rect[1] > r[1] && rect[1] + rect[3] > r[1] + r[3] - inset {
                spilling.push(format!("{}: {line}", card.id.0));
            }
        }
    }

    let mut empty = open(SpellGraph::new("Empty Pot"), (1280.0, 800.0));
    click(&mut empty, "tab-playground")?;
    empty.update(|app, _| app.chat_input = rune::TextEdit::new("anyone home"));
    let chat_before = empty.state.chat.len();
    let _ = click(&mut empty, "chat-send");
    let chat_on_empty = empty.state.chat.len() - chat_before;

    let mut twins = SpellGraph::new("Twin Watchers");
    twins.nodes = vec![
        studio_node("first", "event/on_variable_changed", 0.0, serde_json::Value::Null),
        studio_node("second", "event/on_variable_changed", 280.0, serde_json::Value::Null),
    ];
    let mut menu = open(twins, (1280.0, 800.0));
    let target = menu.state.canvas.card(&NodeId::from("second")).map(|card| card.id.clone()).ok_or("no twin")?;
    menu.update(move |app, _| {
        app.canvas.select(Some(target.clone()));
        app.canvas.context_menu = Some([400.0, 300.0]);
    });
    menu.frame(1.0);
    let dismiss = layout_rect(&menu, "context-dismiss").ok_or("no menu")?;
    let header_clash = all_texts(&menu).is_empty()
        || menu
            .elements()
            .iter()
            .filter(|element| {
                element.text.as_deref().is_some_and(|line| line.starts_with("On Variable Changed")) && (element.rect[1] - dismiss[1]).abs() < 12.0
            })
            .any(|element| element.rect[0] + element.rect[2] > dismiss[0] - 2.0);

    Ok(json!({
        "ribbonShares": ribbon_shares,
        "logs": logs,
        "failedInspector": failed_inspector,
        "summary": summary_report,
        "untakenAfterFail": untaken_after_fail,
        "socketSuffixes": socket_suffixes,
        "pillOverlaps": pill_overlaps,
        "weakBorders": weak_borders,
        "broth": broth,
        "spilling": spilling,
        "chatOnEmpty": chat_on_empty,
        "menuHeaderClash": header_clash,
    }))
}
fn close_quarters(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut text = magicklala_studio::studio_text_system();
    let open = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };

    let mut compact = open(StudioApp::from_registry().spell.clone(), (960.0, 640.0));
    let model = NodeId::from(node_of_kind(&compact.state, "ai/generate_text").ok_or("no model")?.as_str());
    compact.update(move |app, _| {
        app.canvas.select(Some(model.clone()));
        app.right_tab = RightTab::Inspector;
    });
    compact.frame(1.0);
    let field = layout_rect(&compact, "prompt-field").ok_or("no prompt field")?;
    let source = text_lines_under(&compact, "prompt-field").into_iter().find(|(line, _)| line.starts_with("From ")).ok_or("no source line")?;
    let (natural, line_h) = text.measure(&source.0, rune::TextStyle::new(12.0, 650.0), 1.0);
    let rows = (source.1[3] / line_h).round().max(1.0);
    let prompt_source = json!({ "text": source.0, "whole": natural <= source.1[2] * rows + 2.0, "inside": inside(field, source.1) });

    let mut crowding = Vec::new();
    for (name, spell) in [("router", example("webhook_router.json")?), ("support", StudioApp::from_registry().spell.clone())] {
        for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
            let mut rt = open(spell.clone(), size);
            click(&mut rt, "run-spell")?;
            live_finish(&mut rt)?;
            rt.frame(3.0);
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            let s = plan.scale;
            let bottles: Vec<&magicklala_canvas::BrewCard> = plan.with_role(magicklala_canvas::BrewRole::Bottle).collect();
            for (index, ribbon) in magicklala_canvas::cauldron::ribbon_paths(&plan).iter().enumerate() {
                let Some(card) = bottles.get(index) else { continue };
                let r = card.rect;
                let tag = [r[0] - 18.0 * s, r[1] + r[3] * 0.46 - 6.0 * s, r[2] + 26.0 * s, r[3] * 0.54 + 6.0 * s];
                let body = &ribbon[..ribbon.len() * 4 / 5];
                if body.iter().any(|point| point[0] > tag[0] && point[0] < tag[0] + tag[2] && point[1] > tag[1] && point[1] < tag[1] + tag[3]) {
                    crowding.push(format!("{name}@{}: ribbon {index} hugs its tag", size.0));
                }
            }
        }
    }

    let dozen = open(dozen_ingredient_spell(), (960.0, 640.0));
    let plan = magicklala_canvas::brew_plan(&dozen.state.canvas, dozen.state.canvas_size());
    let canvas_h = dozen.state.canvas_size().1;
    let pours = magicklala_canvas::cauldron::pour_paths(&dozen.state.canvas, &plan);
    let top =
        pours.iter().flat_map(|(_, points)| points.iter().map(|point| point[1])).chain(plan.cards.iter().map(|card| card.rect[1])).fold(f32::MAX, f32::min);
    let bottom = plan.cards.iter().map(|card| card.rect[1] + card.rect[3]).fold(plan.plaque_top + 30.0 * plan.scale, f32::max).max(plan.pot[1] + plan.pot[3]);
    let balance = json!({ "above": top.round(), "below": (canvas_h - bottom).round() });

    let mut numbers = studio_at(db_path)?;
    let mut paced = delay_spell(5);
    paced.name = "Paced Agent".to_owned();
    numbers.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(paced.clone());
    });
    click(&mut numbers, "publish")?;
    numbers.update(|app, _| {
        app.configs.insert(NodeId::from("wait"), json!({ "ms": 5.0 }));
        app.sync_spell();
    });
    let same_number = json!({ "live": numbers.state.active_release_version, "unchanged": numbers.state.is_unchanged_from_live_release() });

    let mut menu = open(StudioApp::from_registry().spell.clone(), (1280.0, 800.0));
    let last = magicklala_canvas::recipe_order(&menu.state.canvas)
        .into_iter()
        .rfind(|id| {
            magicklala_canvas::brew_plan(&menu.state.canvas, menu.state.canvas_size())
                .card(id)
                .is_some_and(|card| card.role == magicklala_canvas::BrewRole::Ingredient)
        })
        .ok_or("no ingredient")?;
    let card_rect = layout_rect(&menu, &format!("node-{}", last.0)).ok_or("no card")?;
    right_click(&mut menu, (card_rect[0] + card_rect[2] * 0.5, card_rect[1] + card_rect[3] * 0.5));
    menu.frame(1.0);
    let card_rect = layout_rect(&menu, &format!("node-{}", last.0)).ok_or("no card")?;
    let tether = layout_rect(&menu, "context-tether");
    let panel = layout_rect(&menu, "context-menu");
    let touches = |a: [f32; 4], b: [f32; 4]| a[0] <= b[0] + b[2] + 2.0 && b[0] <= a[0] + a[2] + 2.0 && a[1] <= b[1] + b[3] + 2.0 && b[1] <= a[1] + a[3] + 2.0;
    let attached = match (tether, panel) {
        (Some(tether), Some(panel)) => touches(tether, panel) && touches(tether, card_rect),
        (None, Some(panel)) => touches(panel, card_rect),
        _ => false,
    };

    Ok(json!({
        "promptSource": prompt_source,
        "ribbonCrowding": crowding,
        "balance": balance,
        "sameNumber": same_number,
        "menuAttached": attached,
    }))
}
fn true_runs(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };

    let mut router = runtime_for(example("webhook_router.json")?);
    let before: Vec<NodeId> = router.state.canvas.nodes.iter().map(|card| card.id.clone()).collect();
    router.update(|app, _| {
        let _ = app.spawn("io/send_message");
    });
    let added = router.state.canvas.nodes.iter().map(|card| card.id.clone()).find(|id| !before.contains(id)).ok_or("no new bottle")?;
    let added_role = role_of(&router.state, &added.0);
    router.update(|app, _| app.run_payload = json!({ "method": "POST", "path": "/hooks/router", "body": { "type": "order", "id": "42" } }));
    let order_run = run_once(&mut router)?;
    let added_bottle = json!({ "role": added_role, "failed": order_run["failedNodes"], "status": order_run["status"] });

    let child_db = db_path.with_extension("child.sqlite");
    let mut child = SpellGraph::new("Spare Child");
    child.nodes = vec![
        place("start", "event/on_start", 0.0, serde_json::Value::Null),
        place("send", "io/send_message", 1.0, json!({ "content": "child says hi" })),
        place("spare", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    child.wires = vec![wire("cw", ("start", "out"), ("send", "in"))];
    child.shelf =
        vec![magicklala_graph::ShelfEntry { node: NodeId::from("spare"), parked: Vec::new(), bridges: Vec::new(), refused: Vec::new(), parked_at: Vec::new() }];
    StudioStore::open(Some(&child_db)).and_then(|mut store| store.save_draft(&child)).map_err(|error| error.to_string())?;
    let mut parent = SpellGraph::new("Calling Parent");
    parent.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("call", "flow/subgraph", 1.0, json!({ "spell_id": child.id.0.to_string() })),
    ];
    parent.wires = vec![wire("pw", ("trigger", "out"), ("call", "in"))];
    parent.subgraphs = vec![magicklala_graph::SubgraphRef { node_id: NodeId::from("call"), spell_id: child.id, release_version: None }];
    let mut caller = studio_at(&child_db)?;
    caller.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(parent.clone());
    });
    click(&mut caller, "validate")?;
    let spare_child = caller.state.validation.label();

    let mut twins = SpellGraph::new("Twin Doors");
    twins.nodes = vec![
        place("a", "event/on_message", 0.0, serde_json::Value::Null),
        place("b", "event/on_message", 0.0, serde_json::Value::Null),
        place("say", "io/send_message", 1.0, serde_json::Value::Null),
    ];
    twins.wires = vec![wire("t1", ("a", "out"), ("say", "in")), wire("t2", ("b", "out"), ("say", "in")), wire("t3", ("a", "content"), ("say", "content"))];
    let mut doors = studio_at(db_path)?;
    doors.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(twins.clone());
    });
    click(&mut doors, "publish")?;
    let order_before: Vec<String> = doors.state.canvas.nodes.iter().map(|card| card.id.0.clone()).collect();
    doors.update(|app, _| {
        app.canvas.select(Some(NodeId::from("a")));
        app.delete_selected_node();
        app.undo();
    });
    let order_after: Vec<String> = doors.state.canvas.nodes.iter().map(|card| card.id.0.clone()).collect();
    let undo_trigger = json!({ "sameOrder": order_before == order_after, "live": doors.state.active_release_version });

    let mut prompted = SpellGraph::new("Refusing Agent");
    prompted.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("tmpl", "text/template", 1.0, json!({ "template": "Q: {{content}}" })),
        place("gen", "ai/generate_text", 2.0, serde_json::Value::Null),
        place("reply", "io/send_message", 3.0, serde_json::Value::Null),
    ];
    prompted.wires = vec![
        wire("p1", ("trigger", "out"), ("gen", "in")),
        wire("p2", ("gen", "out"), ("reply", "in")),
        wire("p3", ("trigger", "content"), ("tmpl", "content")),
        wire("p4", ("tmpl", "text"), ("gen", "prompt")),
        wire("p5", ("gen", "text"), ("reply", "content")),
    ];
    let mut refusing = studio_at(db_path)?;
    refusing.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(prompted.clone());
    });
    refusing.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("tmpl"))));
    refusing.update(|app, _| {
        app.canvas.select(Some(NodeId::from("gen")));
        app.right_tab = RightTab::Inspector;
    });
    refusing.frame(1.0);
    click(&mut refusing, "prompt-unplug")?;
    refusing.update(|app, _| {
        app.prompts.insert(NodeId::from("gen"), rune::TextEdit::new("typed words"));
        app.sync_spell();
        app.canvas.select(Some(NodeId::from("tmpl")));
        app.delete_selected_node();
        app.undo();
    });
    click(&mut refusing, "save")?;
    let reopened = studio_at(db_path)?;
    let undo_refusal = reopened.state.canvas.wires.iter().any(|wire| wire.to_node.0 == "gen" && wire.to_socket == "prompt");

    let mut stale = runtime_for(tasting_spell());
    run_once(&mut stale)?;
    stale.update(|app, _| {
        app.canvas.wires.retain(|wire| !(wire.to_node.0 == "reply" && wire.to_socket == "content"));
        app.sync_spell();
    });
    click(&mut stale, "validate")?;
    stale.frame(2.0);
    let stale_log: Vec<String> = text_lines_under(&stale, "trace-strip").into_iter().map(|(line, _)| line).collect();
    let mut failed = runtime_for(example("parallel_research.json")?);
    run_once(&mut failed)?;
    click(&mut failed, "validate")?;
    failed.frame(2.0);
    let pill: Vec<String> = text_lines_under(&failed, "validation-pill").into_iter().map(|(line, _)| line).collect();
    let plaque: Vec<String> = text_lines_under(&failed, "cauldron-plaque").into_iter().map(|(line, _)| line).collect();
    let check_after = json!({
        "staleLogSaysBrewed": stale_log.iter().any(|line| line == "Brewed"),
        "staleLogSaysHexed": stale_log.iter().any(|line| line.starts_with("Hexed")),
        "failedPill": pill.join(" "),
        "failedPlaque": plaque.join(" "),
    });

    let mut hook_only = runtime_for(example("webhook_router.json")?);
    click(&mut hook_only, "tab-playground")?;
    hook_only.update(|app, _| app.chat_input = rune::TextEdit::new("order 123"));
    let chat_before = hook_only.state.chat.len();
    hook_only.update(|app, _| app.send_chat());
    settle(&mut hook_only)?;
    let hook_chat = hook_only.state.chat.len() - chat_before;

    let mut lonely = SpellGraph::new("Lonely Reply");
    lonely.nodes = vec![place("reply", "io/send_message", 1.0, serde_json::Value::Null)];
    let alone = runtime_for(lonely);
    let lone_reply = role_of(&alone.state, "reply");

    let mut unpinned = SpellGraph::new("Unpinned Caller");
    unpinned.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("call", "flow/subgraph", 1.0, json!({ "spell_id": child.id.0.to_string() })),
    ];
    unpinned.wires = vec![wire("uw", ("trigger", "out"), ("call", "in"))];
    unpinned.subgraphs = vec![magicklala_graph::SubgraphRef { node_id: NodeId::from("call"), spell_id: child.id, release_version: None }];
    let mut sealed = studio_at(&child_db)?;
    sealed.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(unpinned.clone());
    });
    click(&mut sealed, "publish")?;
    click(&mut sealed, "save")?;
    let after_save = sealed.state.active_release_version;
    sealed.update(|app, _| {
        let _ = app.publish();
    });
    let unpinned_seal = json!({ "afterSave": after_save, "releases": release_versions(&sealed.state) });

    let mut spare_words = echo_spell();
    spare_words.nodes.push(place("other", "text/template", 1.0, json!({ "template": "other words" })));
    let mut evict = runtime_for(spare_words);
    evict.update(|app, _| {
        app.canvas.select(Some(NodeId::from("echo")));
        app.delete_selected_node();
        let _ = app.canvas.connect_sockets(&SocketRef::new(NodeId::from("other"), "text"), &SocketRef::new(NodeId::from("reply"), "content"));
        app.undo();
    });
    let reply_content_wires = evict.state.canvas.wires.iter().filter(|wire| wire.to_node.0 == "reply" && wire.to_socket == "content").count();

    let mut ticking = SpellGraph::new("Ticking Agent");
    ticking.nodes = vec![place("tick", "event/on_tick", 0.0, serde_json::Value::Null), place("say", "io/send_message", 1.0, serde_json::Value::Null)];
    ticking.wires = vec![wire("k1", ("tick", "out"), ("say", "in")), wire("k2", ("tick", "at"), ("say", "content"))];
    let mut tick_rt = runtime_for(ticking);
    let tick_run = run_once(&mut tick_rt)?;
    let mut changing = SpellGraph::new("Watching Agent");
    changing.nodes =
        vec![place("watch", "event/on_variable_changed", 0.0, serde_json::Value::Null), place("say", "io/send_message", 1.0, serde_json::Value::Null)];
    changing.wires = vec![wire("v1", ("watch", "out"), ("say", "in")), wire("v2", ("watch", "name"), ("say", "content"))];
    let mut change_rt = runtime_for(changing);
    let change_run = run_once(&mut change_rt)?;
    let empty_reply =
        |run: &serde_json::Value| run["replies"].as_array().is_none_or(|replies| replies.iter().all(|reply| reply.as_str().is_none_or(str::is_empty)));
    let samples = json!({ "tickEmpty": empty_reply(&tick_run), "changeEmpty": empty_reply(&change_run) });

    let _ = std::fs::remove_file(&child_db);
    Ok(json!({
        "addedBottle": added_bottle,
        "spareChild": spare_child,
        "undoTrigger": undo_trigger,
        "undoRefusalWired": undo_refusal,
        "checkAfter": check_after,
        "hookChat": hook_chat,
        "loneReply": lone_reply,
        "unpinnedSeal": unpinned_seal,
        "replyContentWires": reply_content_wires,
        "samples": samples,
    }))
}
fn segments_cross(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let turn = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (d1, d2, d3, d4) = (turn(c, d, a), turn(c, d, b), turn(a, b, c), turn(a, b, d));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn pot_rect() -> Result<serde_json::Value, String> {
    let size = StudioConfig::default().size;
    let mut app = StudioApp::from_registry();
    app.size = size;
    let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
    rt.frame(0.0);
    let pot = layout_rect(&rt, "cauldron-center").ok_or("no pot")?;
    Ok(json!({ "size": [size.0, size.1], "pot": pot }))
}

fn clear_signals() -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let open = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let words = |rt: &Runtime<StudioApp>, key: &str| text_lines_under(rt, key).into_iter().map(|(line, _)| line).collect::<Vec<_>>().join(" ");

    let mut research = open(example("parallel_research.json")?, (1280.0, 800.0));
    run_once(&mut research)?;
    let failed = research.state.trace.iter().find(|line| line.kind == "NodeFailed").and_then(|line| line.node.clone()).ok_or("nothing failed")?;
    let lit: Vec<String> = research
        .state
        .canvas
        .nodes
        .iter()
        .filter(|card| card.status == ExecutionStatus::Succeeded)
        .filter(|card| magicklala_canvas::cauldron::pour_blocked(&research.state.canvas, &card.id))
        .map(|card| card.id.0.clone())
        .collect();
    let picked = failed.clone();
    research.update(move |app, _| {
        app.canvas.select(Some(picked.clone()));
        app.right_tab = RightTab::Inspector;
    });
    research.frame(1.0);
    let headline = words(&research, "inspector-error");
    research.update(|app, _| {
        app.canvas.select(None);
        app.right_tab = RightTab::Trace;
    });
    research.frame(2.0);
    let trace_words = words(&research, "inspector-scroll");
    let cause = research
        .state
        .trace
        .iter()
        .find(|line| line.kind == "NodeFailed" && line.node.as_ref() == Some(&failed))
        .map(|line| line.detail.split_once(": ").map_or(line.detail.clone(), |(_, cause)| cause.to_owned()))
        .unwrap_or_default();
    let failure = json!({ "headlineHasCause": !cause.is_empty() && headline.contains(&cause), "traceHasCause": !cause.is_empty() && trace_words.contains(&cause), "dimmedFinishedPours": lit });

    let mut tight = open(example("parallel_research.json")?, (960.0, 640.0));
    run_once(&mut tight)?;
    let tight_failed =
        tight.state.canvas.nodes.iter().find(|card| card.status == ExecutionStatus::Failed).map(|card| card.id.clone()).ok_or("nothing failed")?;
    tight.update(move |app, _| {
        app.canvas.select(Some(tight_failed.clone()));
        app.right_tab = RightTab::Inspector;
    });
    tight.frame(2.0);
    let scroll = layout_rect(&tight, "inspector-scroll").ok_or("no inspector")?;
    let tile = layout_rect(&tight, "model-picker").ok_or("no model tile")?;
    let tile_whole = inside(scroll, tile) || tile[1] >= scroll[1] + scroll[3] - 0.5;

    let title_width = |size: (f32, f32)| -> Result<Option<f32>, String> {
        let mut grim = open(StudioApp::from_registry().spell.clone(), size);
        click(&mut grim, "mode-grimoire")?;
        grim.frame(1.0);
        let name = grim.state.spell.name.clone();
        Ok(text_lines_under(&grim, "toolbar-left").into_iter().find(|(line, _)| line == &name).map(|(_, rect)| rect[2]))
    };
    let roomy_title = title_width((1600.0, 900.0))?;
    let compact_title = title_width((960.0, 640.0))?;
    let title_whole = matches!((roomy_title, compact_title), (Some(full), Some(shown)) if shown + 0.5 >= full);

    let mut mismatch = SpellGraph::new("Mismatched Agent");
    mismatch.nodes = vec![place("tick", "event/on_tick", 0.0, serde_json::Value::Null), place("say", "io/send_message", 1.0, serde_json::Value::Null)];
    mismatch.wires = vec![wire("m1", ("tick", "out"), ("say", "in")), wire("m2", ("tick", "tick"), ("say", "content"))];
    let mut wrong = open(mismatch, (1280.0, 800.0));
    click(&mut wrong, "validate")?;
    wrong.update(|app, _| {
        app.canvas.select(Some(NodeId::from("say")));
        app.right_tab = RightTab::Inspector;
    });
    wrong.frame(1.0);
    let wrong_headline = words(&wrong, "inspector-error");

    let mut named = open(tasting_spell(), (1280.0, 800.0));
    named.update(|app, _| {
        app.add_variable();
        app.add_variable();
        app.add_variable();
    });
    named.update(|app, _| {
        let last = app.var_name_edits.len() - 1;
        app.var_name_edits[last - 2] = rune::TextEdit::new("twin");
        app.var_name_edits[last - 1] = rune::TextEdit::new("twin");
        app.var_name_edits[last] = rune::TextEdit::new("");
        app.sync_spell();
        app.canvas.select(None);
        app.right_tab = RightTab::Variables;
    });
    click(&mut named, "validate")?;
    named.frame(1.0);
    let rows = named.state.var_name_edits.len();
    let name_errors: Vec<String> = (0..rows).filter_map(|idx| Some(words(&named, &format!("var-name-error-{idx}"))).filter(|line| !line.is_empty())).collect();
    let name_plaque = named.state.canvas.brew_summary.clone().unwrap_or_default();

    let mut stacked = SpellGraph::new("Many Doors");
    stacked.nodes = vec![place("trigger", "event/on_message", 0.0, serde_json::Value::Null)];
    let mut previous = "trigger".to_owned();
    for index in 0..7 {
        let id = format!("wait_{index}");
        stacked.nodes.push(place(&id, "time/delay", 1.0 + index as f32, json!({ "ms": 0 })));
        stacked.wires.push(wire(&format!("c{index}"), (&previous, "out"), (&id, "in")));
        previous = id;
    }
    stacked.nodes.push(place("route", "flow/switch", 8.0, json!({ "cases": ["alpha", "beta", "gamma"] })));
    stacked.wires.push(wire("cr", (&previous, "out"), ("route", "in")));
    stacked.wires.push(wire("cv", ("trigger", "content"), ("route", "value")));
    for (index, case) in ["alpha", "beta", "gamma", "default"].iter().enumerate() {
        let id = format!("send_{case}");
        stacked.nodes.push(place(&id, "io/send_message", 9.0, serde_json::Value::Null));
        stacked.wires.push(wire(&format!("s{index}"), ("route", case), (&id, "in")));
        stacked.wires.push(wire(&format!("t{index}"), ("trigger", "content"), (&id, "content")));
    }
    let mut stacked_crossings = Vec::new();
    let mut stacked_lanes = Vec::new();
    for size in [(960.0_f32, 640.0_f32), (1280.0, 800.0)] {
        let rt = open(stacked.clone(), size);
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        let s = plan.scale;
        let ribbons = magicklala_canvas::cauldron::ribbon_paths(&plan);
        for (index, ribbon) in ribbons.iter().enumerate() {
            let tap = ribbon.first().copied().unwrap_or_default();
            let far = |point: &&[f32; 2]| (point[0] - tap[0]).hypot(point[1] - tap[1]) > 40.0 * s;
            let mine: Vec<[f32; 2]> = ribbon.iter().filter(far).copied().collect();
            for other in ribbons.iter().skip(index + 1) {
                let theirs: Vec<[f32; 2]> = other.iter().filter(far).copied().collect();
                let crossed = mine.windows(2).any(|a| theirs.windows(2).any(|b| segments_cross(a[0], a[1], b[0], b[1])));
                if crossed {
                    stacked_crossings.push(format!("{}: ribbon {index} crosses another", size.0));
                }
            }
        }
        if plan.lanes.is_some() {
            let pours = magicklala_canvas::cauldron::pour_paths(&rt.state.canvas, &plan);
            for (index, (id, points)) in pours.iter().enumerate() {
                let body = &points[..points.len() * 3 / 4];
                for (other_id, other) in pours.iter().skip(index + 1) {
                    let other_body = &other[..other.len() * 3 / 4];
                    let touching = body.iter().filter(|point| other_body.iter().any(|near| (near[0] - point[0]).hypot(near[1] - point[1]) < 2.0)).count();
                    if touching > 2 {
                        stacked_lanes.push(format!("{}: {} shares {touching} points with {}", size.0, id.0, other_id.0));
                    }
                }
            }
        }
    }
    let mut hexed_branches = stacked.clone();
    hexed_branches.wires.retain(|wire| wire.id.0 != "t0");
    let mut hexed_rt = open(hexed_branches, (960.0, 640.0));
    click(&mut hexed_rt, "validate")?;
    let branch_moods: Vec<String> = ["send_beta", "send_gamma"]
        .iter()
        .filter_map(|id| {
            hexed_rt
                .state
                .canvas
                .card(&NodeId::from(*id))
                .map(|card| format!("{:?}", magicklala_canvas::view::branch_bottle_mood(&hexed_rt.state.canvas, card)))
        })
        .collect();

    let long_case = "priority_enterprise_escalations_overnight";
    let mut long_routes = SpellGraph::new("Long Routes");
    long_routes.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("route", "flow/switch", 1.0, json!({ "cases": [long_case] })),
        place("send_long", "io/send_message", 2.0, serde_json::Value::Null),
        place("send_rest", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    long_routes.wires = vec![
        wire("l1", ("trigger", "out"), ("route", "in")),
        wire("l2", ("trigger", "content"), ("route", "value")),
        wire("l3", ("route", long_case), ("send_long", "in")),
        wire("l4", ("route", "default"), ("send_rest", "in")),
        wire("l5", ("trigger", "content"), ("send_long", "content")),
        wire("l6", ("trigger", "content"), ("send_rest", "content")),
    ];
    let mut long_rt = open(long_routes, (960.0, 640.0));
    long_rt.update(move |app, _| app.run_payload = json!({ "channel": "studio", "sender": "studio", "content": long_case }));
    run_once(&mut long_rt)?;
    long_rt.frame(3.0);
    let card = layout_rect(&long_rt, "node-send_long").ok_or("no long bottle")?;
    let spilled: Vec<String> = text_lines_under(&long_rt, "node-send_long")
        .into_iter()
        .filter(|(line, rect)| !line.chars().all(|c| c.is_ascii_digit()) && !inside(card, *rect))
        .map(|(line, _)| line)
        .collect();
    let strip = layout_rect(&long_rt, "trace-strip").ok_or("no log")?;
    let result = layout_rect(&long_rt, "log-result");
    let result_inside = result.is_some_and(|rect| inside(strip, rect));

    let mut structured = open(dozen_ingredient_spell(), (1280.0, 800.0));
    let json_card = NodeId::from(node_of_kind(&structured.state, "ai/structured_json").ok_or("no structured json")?.as_str());
    structured.update(move |app, _| {
        app.canvas.select(Some(json_card.clone()));
        app.right_tab = RightTab::Inspector;
    });
    structured.frame(1.0);
    let structured_model = layout_rect(&structured, "model-picker").is_some();

    let mut warm = SpellGraph::new("Two Minds");
    warm.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("first", "ai/generate_text", 1.0, json!({ "prompt": "{{content}}", "temperature": 0.4 })),
        place("second", "ai/generate_text", 2.0, json!({ "prompt": "{{content}}", "temperature": 0.4 })),
        place("reply", "io/send_message", 3.0, serde_json::Value::Null),
    ];
    warm.wires = vec![
        wire("w1", ("trigger", "out"), ("first", "in")),
        wire("w2", ("first", "out"), ("second", "in")),
        wire("w3", ("second", "out"), ("reply", "in")),
        wire("w4", ("second", "text"), ("reply", "content")),
    ];
    warm.variables = vec![magicklala_graph::VariableDef {
        name: "temperature".to_owned(),
        ty: SocketType::Float,
        default: Some(magicklala_graph::Value::Float(0.4)),
        exposed: false,
    }];
    let mut warm_rt = open(warm, (1280.0, 800.0));
    warm_rt.update(|app, _| {
        app.canvas.select(Some(NodeId::from("first")));
        app.adjust_temperature(0.1);
    });
    let second_temp = warm_rt.state.configs.get(&NodeId::from("second")).and_then(|config| config.get("temperature")).and_then(serde_json::Value::as_f64);
    let variable_temp = warm_rt.state.spell.variables.first().and_then(|variable| variable.default.clone()).map(|value| format!("{value:?}"));
    let temperature = json!({ "second": second_temp, "variable": variable_temp });

    let dozen = open(dozen_ingredient_spell(), (960.0, 640.0));
    let dozen_plan = magicklala_canvas::brew_plan(&dozen.state.canvas, dozen.state.canvas_size());
    let ladle_crossed = magicklala_canvas::cauldron::ladle_line(&dozen.state.canvas, &dozen_plan).is_some_and(|(a, b)| {
        magicklala_canvas::cauldron::pour_paths(&dozen.state.canvas, &dozen_plan)
            .iter()
            .any(|(_, points)| points.windows(2).any(|pair| segments_cross(pair[0], pair[1], a, b)))
    });
    let everyday = open(tasting_spell(), (1280.0, 800.0));
    let everyday_plan = magicklala_canvas::brew_plan(&everyday.state.canvas, everyday.state.canvas_size());
    let everyday_ladle = magicklala_canvas::cauldron::ladle_line(&everyday.state.canvas, &everyday_plan).is_some();
    let ladle_clearance = magicklala_canvas::cauldron::ladle_line(&dozen.state.canvas, &dozen_plan).map(|(a, b)| {
        let point_to_segment = |p: [f32; 2], s: [f32; 2], e: [f32; 2]| {
            let (dx, dy) = (e[0] - s[0], e[1] - s[1]);
            let along = (((p[0] - s[0]) * dx + (p[1] - s[1]) * dy) / (dx * dx + dy * dy).max(f32::EPSILON)).clamp(0.0, 1.0);
            ((p[0] - s[0] - dx * along).powi(2) + (p[1] - s[1] - dy * along).powi(2)).sqrt()
        };
        magicklala_canvas::cauldron::pour_paths(&dozen.state.canvas, &dozen_plan)
            .iter()
            .flat_map(|(_, points)| magicklala_canvas::cauldron::above_broth(points, a[1]).windows(2).map(|pair| (pair[0], pair[1])).collect::<Vec<_>>())
            .map(|(s, e)| {
                if segments_cross(s, e, a, b) {
                    0.0
                } else {
                    point_to_segment(s, a, b).min(point_to_segment(e, a, b)).min(point_to_segment(a, s, e)).min(point_to_segment(b, s, e))
                }
            })
            .fold(f32::MAX, f32::min)
    });

    let mut pull = open(echo_spell(), (1280.0, 800.0));
    click(&mut pull, "mode-grimoire")?;
    pull.frame(1.0);
    drag(&mut pull, "socket-reply-in-content", (0.0, 140.0))?;
    pull.update(|app, _| {
        app.sync_edits();
    });
    let pulled = pull.state.canvas.wires.iter().filter(|wire| wire.to_node.0 == "reply" && wire.to_socket == "content").count();

    Ok(json!({
        "failure": failure,
        "tileWhole": tile_whole,
        "grimoireTitleWhole": title_whole,
        "wrongHeadline": wrong_headline,
        "nameErrors": name_errors,
        "namePlaque": name_plaque,
        "stackedCrossings": stacked_crossings,
        "stackedLanes": stacked_lanes,
        "branchMoods": branch_moods,
        "longSpilled": spilled,
        "longResultInside": result_inside,
        "structuredModel": structured_model,
        "temperature": temperature,
        "ladleCrossed": ladle_crossed,
        "ladleBrushes": ladle_clearance.is_some_and(|room| room < 4.5 * (dozen_plan.pot[2].min(dozen_plan.pot[3]) / 240.0).clamp(0.55, 1.35) + 3.4),
        "everydayLadle": everyday_ladle,
        "pulledWires": pulled,
    }))
}
fn honest_dials(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let open = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let lines = |rt: &Runtime<StudioApp>, key: &str| text_lines_under(rt, key).into_iter().map(|(line, _)| line).collect::<Vec<_>>();

    let mut research = open(example("parallel_research.json")?, (1280.0, 800.0));
    let model = NodeId::from(node_of_kind(&research.state, "ai/generate_text").ok_or("no model")?.as_str());
    let picked = model.clone();
    research.update(move |app, _| {
        app.canvas.select(Some(picked.clone()));
        app.right_tab = RightTab::Inspector;
    });
    research.frame(1.0);
    let card_lines = lines(&research, "inspector-card");
    let shown_temperature = card_lines
        .iter()
        .position(|line| line == "Temperature")
        .and_then(|at| card_lines.iter().skip(at + 1).find(|line| !["−", "+"].contains(&line.as_str())).cloned());
    let shown_provider = lines(&research, "model-picker").join(" ");
    click(&mut research, "temp-inc")?;
    let written = research.state.configs.get(&model).and_then(|config| config.get("temperature")).and_then(serde_json::Value::as_f64);
    let temperature = json!({ "shown": shown_temperature, "written": written, "providerShown": shown_provider.contains("Model · OpenAI") });

    let support = StudioApp::from_registry();
    let invented: Vec<String> = support
        .spell
        .variables
        .iter()
        .map(|variable| (variable.name.clone(), support.variable_binding_hint(&variable.name)))
        .filter(|(_, (_, linked))| *linked)
        .filter(|(name, _)| {
            !support.spell.nodes.iter().any(|node| {
                let token = format!("{{{{{name}}}}}");
                node.config.as_object().is_some_and(|settings| settings.values().any(|value| value.as_str().is_some_and(|text| text.contains(&token))))
            })
        })
        .map(|(name, (hint, _))| format!("{name} {hint}"))
        .collect();
    let mut greeting = SpellGraph::new("Greeting Agent");
    greeting.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("words", "text/template", 1.0, json!({ "template": "{{greeting}} friend" })),
        place("reply", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    greeting.wires = vec![wire("g1", ("trigger", "out"), ("reply", "in")), wire("g2", ("words", "text"), ("reply", "content"))];
    greeting.variables = vec![magicklala_graph::VariableDef {
        name: "greeting".to_owned(),
        ty: SocketType::String,
        default: Some(magicklala_graph::Value::String("hi".to_owned())),
        exposed: false,
    }];
    let mut greet = open(greeting, (1280.0, 800.0));
    let greeting_hint = greet.state.variable_binding_hint("greeting");
    let first = run_once(&mut greet)?;
    greet.update(|app, _| {
        app.var_default_edits[0] = rune::TextEdit::new("hey");
        app.sync_spell();
    });
    let second = run_once(&mut greet)?;
    let bindings = json!({ "invented": invented, "greetingLinked": greeting_hint.1, "first": first["replies"], "second": second["replies"] });

    let mut grim = open(example("webhook_router.json")?, (960.0, 640.0));
    click(&mut grim, "mode-grimoire")?;
    grim.frame(1.0);
    let socket_names = grim
        .state
        .canvas
        .nodes
        .iter()
        .map(|card| {
            lines(&grim, &format!("node-{}", card.id.0))
                .iter()
                .filter(|line| card.inputs.iter().chain(card.outputs.iter()).any(|socket| &socket.name == *line))
                .count()
        })
        .sum::<usize>();
    let hook = node_of_kind(&grim.state, "event/on_webhook").ok_or("no webhook")?;
    let port = grim.find(&format!("socket-{hook}-out-body")).ok_or("no body socket")?;
    grim.pointer_move(port.0, port.1);
    grim.frame(1.5);
    let hover_tip = words_of(&grim, "socket-tip");
    let compact_grimoire = json!({ "zoom": (grim.state.canvas.camera.zoom * 100.0).round() / 100.0, "socketNames": socket_names, "hoverTip": hover_tip });

    let mut parallel = open(example("parallel_research.json")?, (1280.0, 800.0));
    run_once(&mut parallel)?;
    parallel.frame(2.0);
    let mut shelves = vec![parallel];
    for (spell, size) in [(many_bottle_spell(8), (1280.0, 800.0)), (example("webhook_router.json")?, (1280.0, 800.0)), (crowded_kitchen(), (1280.0, 800.0))] {
        let mut shelf = open(spell, size);
        click(&mut shelf, "run-spell")?;
        live_finish(&mut shelf)?;
        shelf.frame(2.0);
        shelves.push(shelf);
    }
    let mut badges: Vec<(usize, String)> = Vec::new();
    let mut badge_mismatch: Vec<String> = Vec::new();
    for shelf in &shelves {
        for card in &shelf.state.canvas.nodes {
            let key = format!("step-badge-{}", card.id.0);
            let shown = shelf.presented(&key).map(|_| words_of(shelf, &key));
            match (card.step_order, shown) {
                (Some(step), Some(shown)) => {
                    if shown != step.to_string() {
                        badge_mismatch.push(format!("{} {} ran {step} shows {shown}", shelf.state.spell.name, card.id.0));
                    }
                    badges.push((step, shown));
                }
                (None, Some(shown)) => badge_mismatch.push(format!("{} {} never ran shows {shown}", shelf.state.spell.name, card.id.0)),
                _ => {}
            }
        }
    }

    let mut lanes = open(example("webhook_router.json")?, (960.0, 640.0));
    run_once(&mut lanes)?;
    let lane_plan = magicklala_canvas::brew_plan(&lanes.state.canvas, lanes.state.canvas_size());
    let mut tightest = f32::MAX;
    if lane_plan.lanes.is_some() {
        for (_, points) in magicklala_canvas::cauldron::pour_paths(&lanes.state.canvas, &lane_plan) {
            let turns = &points[..points.len().saturating_sub(16)];
            for trio in turns.windows(3) {
                let a = [trio[1][0] - trio[0][0], trio[1][1] - trio[0][1]];
                let b = [trio[2][0] - trio[1][0], trio[2][1] - trio[1][1]];
                let (la, lb) = (a[0].hypot(a[1]), b[0].hypot(b[1]));
                if la < 0.01 || lb < 0.01 {
                    continue;
                }
                let turn = ((a[0] * b[0] + a[1] * b[1]) / (la * lb)).clamp(-1.0, 1.0).acos();
                if turn > 0.03 {
                    tightest = tightest.min((la + lb) * 0.5 / turn / lane_plan.scale);
                }
            }
        }
    }
    let lane_radius = if tightest == f32::MAX { serde_json::Value::Null } else { json!(tightest.round()) };

    let mut chips = open(example("webhook_router.json")?, (960.0, 640.0));
    run_once(&mut chips)?;
    chips.frame(2.0);
    let first_chip_named = lines(&chips, "trace-strip").iter().any(|line| line.starts_with("On Webhook"));

    let mut grim_wide = open(example("webhook_router.json")?, (1280.0, 800.0));
    click(&mut grim_wide, "mode-grimoire")?;
    grim_wide.frame(1.0);
    let header_words = lines(&grim_wide, "toolbar-left").join(" ");

    let mut big = studio_at(db_path)?;
    let mut counted = delay_spell(5);
    counted.name = "Big Numbers".to_owned();
    if let Some(node) = counted.nodes.iter_mut().find(|node| node.id.0 == "wait") {
        node.config = json!({ "ms": 5, "seed": 9_007_199_254_740_993_u64 });
    }
    big.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(counted.clone());
    });
    click(&mut big, "publish")?;
    big.update(|app, _| {
        app.configs.insert(NodeId::from("wait"), json!({ "ms": 5, "seed": 9_007_199_254_740_992_u64 }));
        app.sync_spell();
    });
    let big_numbers = big.state.active_release_version;

    Ok(json!({
        "temperature": temperature,
        "bindings": bindings,
        "compactGrimoire": compact_grimoire,
        "badgeMismatch": badge_mismatch,
        "badgesSeen": badges.len(),
        "laneRadius": lane_radius,
        "firstChipNamed": first_chip_named,
        "headerSaysBottles": header_words.contains("bottle"),
        "bigNumbersLive": big_numbers,
    }))
}

fn gateway_hint() -> Result<serde_json::Value, String> {
    let mut lonely = runtime_for(example("parallel_research.json")?);
    run_once(&mut lonely)?;
    let failed = lonely.state.trace.iter().find(|line| line.kind == "NodeFailed").and_then(|line| line.node.clone()).ok_or("nothing failed")?;
    lonely.update(move |app, _| {
        app.canvas.select(Some(failed.clone()));
        app.right_tab = RightTab::Inspector;
    });
    lonely.frame(1.0);
    Ok(json!({ "error": words_of(&lonely, "inspector-error") }))
}

fn words_of(rt: &Runtime<StudioApp>, key: &str) -> String {
    text_lines_under(rt, key).into_iter().map(|(line, _)| line).collect::<Vec<_>>().join(" ")
}
fn workbench_truths(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let spawned = |rt: &mut Runtime<StudioApp>, kind: &'static str| -> Option<NodeId> {
        let before: Vec<NodeId> = rt.state.canvas.nodes.iter().map(|card| card.id.clone()).collect();
        rt.update(move |app, _| {
            let _ = app.spawn(kind);
        });
        rt.state.canvas.nodes.iter().map(|card| card.id.clone()).find(|id| !before.contains(id))
    };

    let mut undoing = runtime_for(tasting_spell());
    run_once(&mut undoing)?;
    undoing.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("echo"))));
    undoing.frame(1.0);
    let undo_shown = undoing.find("undo-action").is_some();
    let _ = click(&mut undoing, "undo-action");
    let echo_back = !undoing.state.canvas.is_shelved(&NodeId::from("echo"));
    let mut lone_aside = SpellGraph::new("Loner");
    lone_aside.nodes =
        vec![place("trigger", "event/on_message", 0.0, serde_json::Value::Null), place("note", "text/template", 1.0, json!({ "template": "x" }))];
    let mut loner = runtime_for(lone_aside);
    loner.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("note"))));
    let lone_undo = !loner.state.undo_stack.is_empty();
    let undo = json!({ "afterBrewShown": undo_shown, "restored": echo_back, "unwiredAsideUndoable": lone_undo });

    let demo = StudioApp::from_registry();
    let dead_keys: Vec<String> = demo
        .spell
        .nodes
        .iter()
        .filter_map(|node| node.config.as_object().map(|map| (node.kind.clone(), map.keys().cloned().collect::<Vec<_>>())))
        .flat_map(|(kind, keys)| {
            keys.into_iter().filter(move |key| (kind == "memory/search" && key == "max_memories") || (kind == "io/send_message" && key == "agent_name"))
        })
        .collect();

    let mut vars = runtime_for(tasting_spell());
    vars.update(|app, _| app.add_variable());
    let first_var = vars.state.spell.variables.first().map(|variable| variable.name.clone()).unwrap_or_default();
    let getter = spawned(&mut vars, "var/set").ok_or("no var/set")?;
    let seeded = vars.state.spell.node(&getter).and_then(|node| node.config.get("name").and_then(serde_json::Value::as_str).map(str::to_owned));
    let picked = getter.clone();
    vars.update(move |app, _| {
        app.canvas.select(Some(picked.clone()));
        app.right_tab = RightTab::Inspector;
    });
    vars.frame(1.0);
    let has_name_field = vars.find("prompt-field").is_some();
    let blanked = getter.clone();
    vars.update(move |app, _| {
        app.prompts.insert(blanked.clone(), rune::TextEdit::new(""));
        app.sync_spell();
    });
    click(&mut vars, "validate")?;
    let blank_check = vars.state.validation.label();
    let variables = json!({ "seeded": seeded == Some(first_var), "nameField": has_name_field, "blankCheck": blank_check });

    let mut rhai = runtime_for(tasting_spell());
    let script = spawned(&mut rhai, "code/rhai_script").ok_or("no rhai")?;
    let rhai_run = run_once(&mut rhai)?;
    let rhai_report = json!({ "status": rhai_run["status"], "failed": rhai_run["failedNodes"], "inputs": rhai.state.spell.node(&script).and_then(|node| node.config.get("inputs").cloned()) });

    let mut pulled = SpellGraph::new("Pulled Shortcut");
    pulled.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("tmpl", "text/template", 1.0, json!({ "template": "Q: {{content}}" })),
        place("reply", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    pulled.wires = vec![
        wire("p1", ("trigger", "out"), ("reply", "in")),
        wire("p2", ("trigger", "content"), ("tmpl", "content")),
        wire("p3", ("tmpl", "text"), ("reply", "content")),
    ];
    let mut pulling = studio_at(db_path)?;
    pulling.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(pulled.clone());
    });
    pulling.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("tmpl"))));
    click(&mut pulling, "mode-grimoire")?;
    pulling.frame(1.0);
    let still_id = pulling.state.canvas.wires.iter().find(|wire| wire.to_node.0 == "reply" && wire.to_socket == "content").map(|wire| wire.id.clone());
    click(&mut pulling, "socket-reply-in-content")?;
    pulling.update(|app, _| {
        app.sync_edits();
    });
    let after_click = pulling.state.canvas.wires.iter().find(|wire| wire.to_node.0 == "reply" && wire.to_socket == "content").map(|wire| wire.id.clone());
    drag(&mut pulling, "socket-reply-in-content", (0.0, 160.0))?;
    pulling.update(|app, _| {
        app.sync_edits();
    });
    click(&mut pulling, "save")?;
    let reopened = studio_at(db_path)?;
    let pull = json!({
        "clickKeepsWire": still_id.is_some() && still_id == after_click,
        "pulledStaysPulled": !reopened.state.canvas.wires.iter().any(|wire| wire.to_node.0 == "reply" && wire.to_socket == "content"),
    });

    let mut embedding = SpellGraph::new("Embedder");
    embedding.nodes = vec![place("trigger", "event/on_message", 0.0, serde_json::Value::Null), place("embed", "ai/embedding", 1.0, serde_json::Value::Null)];
    embedding.wires = vec![wire("e1", ("trigger", "out"), ("embed", "in"))];
    let mut embed = runtime_for(embedding);
    embed.update(|app, _| {
        app.canvas.select(Some(NodeId::from("embed")));
        app.right_tab = RightTab::Inspector;
    });
    embed.frame(1.0);
    let embed_lines = text_lines_under(&embed, "inspector-card").into_iter().map(|(line, _)| line).collect::<Vec<_>>();
    let mut emptied = runtime_for(StudioApp::from_registry().spell.clone());
    let llm = NodeId::from(node_of_kind(&emptied.state, "ai/generate_text").ok_or("no model")?.as_str());
    let emptied_id = llm.clone();
    emptied.update(move |app, _| {
        app.canvas.wires.retain(|wire| !(wire.to_node == emptied_id && wire.to_socket == "prompt"));
        app.prompts.insert(emptied_id.clone(), rune::TextEdit::new("   "));
        app.sync_spell();
    });
    click(&mut emptied, "validate")?;
    let fields = json!({ "embedCaption": embed_lines.iter().any(|line| line == "Text") && !embed_lines.iter().any(|line| line == "Prompt"), "blankPromptCheck": emptied.state.validation.label() });

    let mut mending = SpellGraph::new("Mend Routes");
    mending.nodes = vec![
        place("memo", "event/on_memory", 0.0, serde_json::Value::Null),
        place("chat", "event/on_message", 0.0, serde_json::Value::Null),
        place("aside", "text/template", 1.0, json!({ "template": "shelf" })),
        place("say", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    mending.wires = vec![wire("m1", ("chat", "out"), ("say", "in"))];
    let mut mend = runtime_for(mending);
    mend.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("aside"))));
    click(&mut mend, "validate")?;
    let offer = mend.state.hex_mend_source(&NodeId::from("say")).map(|(source, _)| source.node.0);

    let mut loops = SpellGraph::new("Looping Words");
    loops.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("left", "text/template", 1.0, json!({ "template": "{{content}}" })),
        place("right", "text/template", 1.0, json!({ "template": "{{content}}" })),
    ];
    loops.wires = vec![wire("l1", ("left", "text"), ("right", "content")), wire("l2", ("right", "text"), ("left", "content"))];
    let mut looped = runtime_for(loops);
    let loop_bottle = spawned(&mut looped, "io/send_message");
    let loop_role = loop_bottle.and_then(|id| {
        let source = looped.state.canvas.wires.iter().find(|wire| wire.to_node == id && wire.to_socket == "content").map(|wire| wire.from_node.0.clone());
        role_of(&looped.state, &id.0).map(|role| json!({ "role": role, "wordsFrom": source }))
    });

    let mut stale = runtime_for(tasting_spell());
    run_once(&mut stale)?;
    stale.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("trigger"))));
    stale.frame(1.0);
    let pill_after_shelving = text_lines_under(&stale, "validation-pill").into_iter().map(|(line, _)| line).collect::<Vec<_>>().join(" ");
    let log_after_shelving = text_lines_under(&stale, "trace-strip").into_iter().map(|(line, _)| line).collect::<Vec<_>>();
    let mut checked = runtime_for(tasting_spell());
    run_once(&mut checked)?;
    click(&mut checked, "validate")?;
    let _ = spawned(&mut checked, "time/delay");
    checked.frame(1.0);
    let log_after_edit = text_lines_under(&checked, "trace-strip").into_iter().map(|(line, _)| line).collect::<Vec<_>>();
    let stale_marks = json!({ "pillAfterShelving": pill_after_shelving, "shelvedLogSaysBrewed": log_after_shelving.iter().any(|line| line == "Brewed"), "logSaysChecked": log_after_edit.iter().any(|line| line == "Checked") });

    Ok(json!({
        "undo": undo,
        "deadKeys": dead_keys,
        "variables": variables,
        "rhai": rhai_report,
        "pull": pull,
        "fields": fields,
        "mendOffer": offer,
        "loopBottle": loop_role,
        "staleMarks": stale_marks,
    }))
}
fn hands_on() -> Result<serde_json::Value, String> {
    let press = |rt: &mut Runtime<StudioApp>, key: rune::Key, cmd: bool| {
        rt.key(rune::KeyEvent { key, mods: rune::Mods { cmd, ..Default::default() } });
        rt.frame(1.0);
    };
    let centre_of = |rt: &Runtime<StudioApp>, key: &str| layout_rect(rt, key).map(|r| (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5));
    let echo = || NodeId::from("echo");

    let mut keys = runtime_for(tasting_spell());
    keys.on_key = Some(magicklala_studio::studio_key);
    keys.update(move |app, _| app.canvas.select(Some(echo())));
    press(&mut keys, rune::Key::Escape, false);
    let escape_clears = keys.state.canvas.selection.is_none();
    let spot = centre_of(&keys, "node-echo").ok_or("no echo card")?;
    right_click(&mut keys, spot);
    let menu_opened = keys.state.canvas.context_menu.is_some();
    press(&mut keys, rune::Key::Escape, false);
    let escape_closes_menu = menu_opened && keys.state.canvas.context_menu.is_none();
    keys.update(move |app, _| app.canvas.select(Some(echo())));
    press(&mut keys, rune::Key::Delete, false);
    let deleted = keys.state.canvas.card(&echo()).is_none();
    press(&mut keys, rune::Key::Command('z'), true);
    let undone = keys.state.canvas.card(&echo()).is_some();
    let keyboard = json!({ "escapeClears": escape_clears, "escapeClosesMenu": escape_closes_menu, "deleteRemoves": deleted, "commandZRestores": undone });

    let mut brewing = slow_runtime(3000, Duration::from_secs(20));
    click(&mut brewing, "run-spell")?;
    brewing.frame(1.0);
    brewing.update(|app, _| app.canvas.mode = magicklala_canvas::WorkbenchMode::Grimoire);
    brewing.frame(1.0);
    let canvas_rect = rect(&brewing, "canvas").ok_or("no canvas")?;
    right_click(&mut brewing, (canvas_rect[0] + canvas_rect[2] - 8.0, canvas_rect[1] + canvas_rect[3] - 8.0));
    let add_locked = brewing.state.is_running();
    let add_disabled =
        brewing.accessibility_tree().nodes.iter().any(|(_, node)| node.is_disabled() && node.label().is_some_and(|label| label.starts_with("Add ")));
    brewing.update(|app, _| app.cancel_run());

    let mut dup = runtime_for(tasting_spell());
    dup.update(move |app, _| {
        app.toss(magicklala_canvas::Toss::Aside(echo()));
        app.canvas.select(Some(echo()));
        app.duplicate_selected_node();
    });
    let copy_aside =
        dup.state.canvas.nodes.iter().find(|card| card.kind == "text/template" && card.id != echo()).is_some_and(|card| dup.state.canvas.is_shelved(&card.id));
    let mut dyn_rt = runtime_for(tasting_spell());
    dyn_rt.update(|app, _| {
        let _ = app.spawn("code/rhai_script");
    });
    let script = dyn_rt.state.canvas.nodes.iter().find(|card| card.kind == "code/rhai_script").map(|card| card.id.clone()).ok_or("no script")?;
    let picked = script.clone();
    dyn_rt.update(move |app, _| {
        app.canvas.select(Some(picked.clone()));
        app.duplicate_selected_node();
    });
    let copy_sockets = dyn_rt
        .state
        .canvas
        .nodes
        .iter()
        .find(|card| card.kind == "code/rhai_script" && card.id != script)
        .map(|card| card.inputs.iter().map(|socket| socket.name.clone()).collect::<Vec<_>>())
        .unwrap_or_default();
    let duplicate = json!({ "asideCopyStaysAside": copy_aside, "copyHasContent": copy_sockets.iter().any(|name| name == "content") });

    let mut bottle = runtime_for(tasting_spell());
    let spot = centre_of(&bottle, "node-reply").ok_or("no reply card")?;
    right_click(&mut bottle, spot);
    let duplicate_caption = text_lines_under(&bottle, "context-duplicate-node").into_iter().map(|(line, _)| line).collect::<Vec<_>>();
    let offered = bottle.find("context-set-aside").is_some();
    let _ = click(&mut bottle, "context-set-aside");
    let bottle_aside = bottle.state.canvas.is_shelved(&NodeId::from("reply"));

    let mut covered = Vec::new();
    for size in [(1280.0, 800.0), (960.0, 700.0)] {
        let mut app = StudioApp::with_spell(StudioApp::from_registry().spell.clone(), registry_specs());
        app.size = size;
        let mut menu = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        menu.frame(0.0);
        let plan = magicklala_canvas::brew_plan(&menu.state.canvas, menu.state.canvas_size());
        for id in magicklala_canvas::recipe_order(&menu.state.canvas) {
            if plan.card(&id).is_none_or(|card| card.role != magicklala_canvas::BrewRole::Ingredient) {
                continue;
            }
            menu.update(|app, _| app.close_context_menu());
            let Some(spot) = centre_of(&menu, &format!("node-{}", id.0)) else { continue };
            right_click(&mut menu, spot);
            menu.frame(1.0);
            if let (Some(panel), Some(pot)) = (layout_rect(&menu, "context-menu"), pot_body(&menu))
                && rects_overlap(panel, pot)
            {
                covered.push(format!("{}x{} {}", size.0, size.1, id.0));
            }
        }
    }

    Ok(json!({
        "keyboard": keyboard,
        "addDuringBrew": { "running": add_locked, "disabled": add_disabled },
        "duplicate": duplicate,
        "bottle": { "caption": duplicate_caption, "offered": offered, "aside": bottle_aside },
        "menuCoversPot": covered,
    }))
}
fn many_bottle_spell(count: usize) -> SpellGraph {
    let mut spell = SpellGraph::new(format!("{count} Bottles"));
    let place = |id: &str, kind: &str, x: f32, y: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x, y],
        config,
    };
    spell.nodes = vec![
        place("trigger", "event/on_message", 0.0, 0.0, serde_json::Value::Null),
        place("words", "text/template", 260.0, 0.0, json!({ "template": "Heard: {{content}}" })),
    ];
    let wire = |id: String, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id.as_str()),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    spell.wires = vec![wire("w1".to_owned(), ("trigger", "content"), ("words", "content"))];
    for index in 0..count {
        let id = format!("reply_{index}");
        spell.nodes.push(place(&id, "io/send_message", 560.0, index as f32 * 120.0, serde_json::Value::Null));
        spell.wires.push(wire(format!("f{index}"), ("trigger", "out"), (&id, "in")));
        spell.wires.push(wire(format!("c{index}"), ("words", "text"), (&id, "content")));
    }
    spell
}

fn crowded_kitchen() -> SpellGraph {
    let specs = registry_specs();
    let flow = |sockets: &[magicklala_graph::SocketSpec]| sockets.iter().find(|socket| socket.ty == SocketType::Flow).map(|socket| socket.name.clone());
    let wordy = |spec: &magicklala_graph::NodeSpec| {
        spec.inputs.iter().filter(|socket| socket.required && !socket.ty.is_flow()).all(|socket| matches!(socket.ty, SocketType::String | SocketType::Any))
    };
    let mut spell = SpellGraph::new("Crowded Kitchen");
    let wire = |id: String, from: (&NodeId, &str), to: (&NodeId, &str)| Wire {
        id: WireId::from(id.as_str()),
        from_node: from.0.clone(),
        from_socket: from.1.to_owned(),
        to_node: to.0.clone(),
        to_socket: to.1.to_owned(),
    };
    let start = NodeId::from("start");
    spell.nodes.push(NodeInstance { id: start.clone(), kind: "event/on_message".to_owned(), position: [0.0, 0.0], config: serde_json::Value::Null });
    let mut previous = (start.clone(), "out".to_owned());
    let chain: Vec<&magicklala_graph::NodeSpec> = specs
        .iter()
        .filter(|spec| !spec.is_trigger && spec.kind != "flow/subgraph" && !["io", "ai", "net"].contains(&spec.kind.split('/').next().unwrap_or_default()))
        .filter(|spec| flow(&spec.inputs).is_some() && flow(&spec.outputs).is_some() && wordy(spec))
        .take(11)
        .collect();
    for (index, spec) in chain.iter().enumerate() {
        let id = NodeId::from(format!("step_{index}").as_str());
        let config = match spec.kind.as_str() {
            "code/rhai_script" => json!({ "script": "\"stirred\"" }),
            "flow/switch" => json!({ "cases": ["stir"] }),
            _ => serde_json::Value::Null,
        };
        spell.nodes.push(NodeInstance { id: id.clone(), kind: spec.kind.clone(), position: [220.0 * (index + 1) as f32, 0.0], config });
        spell.wires.push(wire(format!("chain_{index}"), (&previous.0, &previous.1), (&id, &flow(&spec.inputs).unwrap_or_default())));
        for socket in spec.inputs.iter().filter(|socket| socket.required && !socket.ty.is_flow()) {
            spell.wires.push(wire(format!("feed_{index}_{}", socket.name), (&start, "content"), (&id, &socket.name)));
        }
        previous = (id, flow(&spec.outputs).unwrap_or_default());
    }
    for index in 0..14 {
        let id = NodeId::from(format!("bottle_{index}").as_str());
        spell.nodes.push(NodeInstance {
            id: id.clone(),
            kind: "io/send_message".to_owned(),
            position: [2800.0, index as f32 * 120.0],
            config: serde_json::Value::Null,
        });
        spell.wires.push(wire(format!("pour_{index}"), (&previous.0, &previous.1), (&id, "in")));
        spell.wires.push(wire(format!("words_{index}"), (&start, "content"), (&id, "content")));
    }
    spell
}
fn crowded_bottles(args: &[String]) -> Result<serde_json::Value, String> {
    let mut report = Vec::new();
    let shelves = [
        ("4", many_bottle_spell(4), (1280.0_f32, 800.0_f32), false),
        ("4", many_bottle_spell(4), (960.0, 640.0), false),
        ("8", many_bottle_spell(8), (1280.0, 800.0), false),
        ("8", many_bottle_spell(8), (960.0, 640.0), false),
        ("kitchen", crowded_kitchen(), (1280.0, 800.0), false),
        ("kitchen", crowded_kitchen(), (960.0, 640.0), true),
    ];
    for (count, spell, size, pantry) in shelves {
        {
            let mut app = StudioApp::with_spell(spell.clone(), registry_specs());
            app.size = size;
            let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
            rt.frame(0.0);
            if pantry {
                click(&mut rt, "expand-nodes")?;
                rt.frame(2.0);
            }
            let check = rt.state.validate().label();
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            let s = plan.scale;
            let bottles: Vec<&magicklala_canvas::BrewCard> = plan.with_role(magicklala_canvas::BrewRole::Bottle).collect();
            let ribbons = magicklala_canvas::cauldron::ribbon_paths(&plan);
            let body = |ribbon: &Vec<[f32; 2]>| -> Vec<[f32; 2]> {
                let tap = ribbon.first().copied().unwrap_or_default();
                let mouth = ribbon.last().copied().unwrap_or_default();
                ribbon
                    .iter()
                    .copied()
                    .filter(|p| (p[0] - tap[0]).hypot(p[1] - tap[1]) > 30.0 * s && (p[0] - mouth[0]).hypot(p[1] - mouth[1]) > 10.0 * s)
                    .collect()
            };
            let mut crossings = Vec::new();
            for (index, ribbon) in ribbons.iter().enumerate() {
                for (other, theirs) in ribbons.iter().enumerate().skip(index + 1) {
                    if polylines_cross(&body(ribbon), &body(theirs)) {
                        crossings.push(format!("{index}x{other}"));
                    }
                }
            }
            let mut through = Vec::new();
            for (index, ribbon) in ribbons.iter().enumerate() {
                let own = bottles.get(index).map(|card| card.id.clone());
                for card in &plan.cards {
                    if Some(&card.id) == own.as_ref() {
                        continue;
                    }
                    let r = card.rect;
                    let halo = 4.0 * s;
                    if body(ribbon).iter().any(|p| p[0] > r[0] - halo && p[0] < r[0] + r[2] + halo && p[1] > r[1] - halo && p[1] < r[1] + r[3] + halo) {
                        through.push(format!("{index}>{}", card.id.0));
                    }
                }
            }
            let shelf_left = bottles.iter().map(|card| card.rect[0]).fold(f32::MAX, f32::min);
            let tap_row = magicklala_canvas::cauldron::ribbon_paths(&plan)
                .iter()
                .filter_map(|ribbon| {
                    let tap = ribbon.first().copied()?;
                    let turned = ribbon.iter().position(|point| (point[1] - tap[1]).abs() > 3.0 * s)?;
                    (ribbon[turned][0] < shelf_left - 2.0 * s).then_some(())?;
                    let leaves = ribbon.iter().position(|point| (point[1] - tap[1]).abs() > 0.2 * s)?;
                    Some(ribbon[leaves.saturating_sub(1)][0])
                })
                .collect::<Vec<f32>>();
            let mut departures = tap_row;
            departures.sort_by(f32::total_cmp);
            let tap_row = departures.windows(2).filter(|pair| pair[1] - pair[0] > 6.0 * s).count() + usize::from(!departures.is_empty());
            let smallest_label = plan
                .cards
                .iter()
                .filter_map(|card| text_lines_under(&rt, &format!("node-{}", card.id.0)).first().map(|(_, rect)| rect[3]))
                .fold(f32::MAX, f32::min);
            let mut spills = Vec::new();
            for card in &plan.cards {
                let Some(frame) = layout_rect(&rt, &format!("node-{}-header", card.id.0)) else { continue };
                let lines = text_lines_under(&rt, &format!("node-{}", card.id.0));
                let words: Vec<&(String, [f32; 4])> = lines.iter().filter(|(line, _)| line.parse::<usize>().is_err()).collect();
                let badges: Vec<&(String, [f32; 4])> = lines.iter().filter(|(line, _)| line.parse::<usize>().is_ok()).collect();
                for (line, rect) in &words {
                    if rect[0] < frame[0] - 0.5
                        || rect[0] + rect[2] > frame[0] + frame[2] + 0.5
                        || rect[1] < frame[1] - 0.5
                        || rect[1] + rect[3] > frame[1] + frame[3] + 0.5
                    {
                        spills.push(format!("{}: {line} leaves its card", card.id.0));
                    }
                    if badges.iter().any(|(_, badge)| rects_overlap(*badge, *rect)) {
                        spills.push(format!("{}: badge covers {line}", card.id.0));
                    }
                }
            }
            report.push(json!({ "case": format!("{count}@{}", size.0), "check": check, "crossings": crossings, "through": through, "outlets": tap_row, "smallestLabel": smallest_label.round(), "spills": spills }));
            if let Some(dir) = args.get(1) {
                let app = std::mem::replace(&mut rt.state, StudioApp::from_registry());
                render_headless_snapshot_state(&PathBuf::from(dir).join(format!("bottles{count}_{}.png", size.0)), app).map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(json!(report))
}
fn agreed_edits(db_path: &Path) -> Result<serde_json::Value, String> {
    let query_wire = |rt: &Runtime<StudioApp>| {
        rt.state
            .canvas
            .wires
            .iter()
            .find(|wire| wire.to_node.0 == "recall" && wire.to_socket == "query")
            .map(|wire| format!("{}.{}", wire.from_node.0, wire.from_socket))
    };
    let idle = |rt: &Runtime<StudioApp>| matches!(rt.state.canvas.drag, magicklala_canvas::DragState::Idle);
    let grimoire = || -> Result<Runtime<StudioApp>, String> {
        let mut rt = runtime_for(StudioApp::from_registry().spell.clone());
        click(&mut rt, "mode-grimoire")?;
        rt.frame(1.0);
        rt.update(|app, _| {
            app.canvas.camera.zoom = 1.0;
            app.canvas.camera.pan_x = 40.0;
            app.canvas.camera.pan_y = 40.0;
        });
        rt.frame(1.0);
        Ok(rt)
    };
    let mut away = grimoire()?;
    let before = query_wire(&away);
    drag(&mut away, "socket-recall-in-query", (0.0, 220.0))?;
    away.update(|app, _| {
        app.sync_edits();
    });
    let pulled_off = json!({ "before": before, "after": query_wire(&away), "dragIdle": idle(&away) });
    let mut refused = grimoire()?;
    let from = refused.find("socket-recall-in-query").ok_or("no query socket")?;
    let onto = refused.find("socket-llm-in-in").ok_or("no llm flow socket")?;
    drag(&mut refused, "socket-recall-in-query", (onto.0 - from.0, onto.1 - from.1))?;
    let refused_drop = json!({ "after": query_wire(&refused), "dragIdle": idle(&refused) });

    let mut blank = studio_at(db_path)?;
    blank.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(tasting_spell());
    });
    blank.update(|app, _| {
        app.prompts.insert(NodeId::from("echo"), rune::TextEdit::new(""));
        app.sync_spell();
    });
    let published = blank.state.publish().is_ok();
    let releases = release_versions(&blank.state);

    let mut child = studio_at(db_path)?;
    child.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(child_spell("child heard {{topic}}"));
    });
    click(&mut child, "publish")?;
    let child_id = child.state.spell.id;
    let mut caller_spell = calling_spell("Caller", "event/on_message", child_id);
    caller_spell.subgraphs[0].release_version = Some(1);
    let mut caller = studio_at(db_path)?;
    caller.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(caller_spell.clone());
    });
    let first = run_once(&mut caller)?;
    caller.update(|app, _| {
        app.canvas.select(Some(NodeId::from("call")));
        app.delete_selected_node();
        app.undo();
    });
    let link = caller
        .state
        .spell
        .subgraphs
        .iter()
        .find(|reference| reference.node_id.0 == "call")
        .map(|reference| json!({ "child": reference.spell_id == child_id, "version": reference.release_version }));
    let second = run_once(&mut caller)?;
    let undo_link = json!({ "before": first["status"], "link": link, "after": second["status"] });

    let mut marked = studio_at(db_path)?;
    marked.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(echo_spell());
    });
    click(&mut marked, "publish")?;
    run_once(&mut marked)?;
    let live_before = marked.state.active_release_version;
    click(&mut marked, "tab-variables")?;
    marked.update(|app, _| app.add_variable());
    marked.frame(1.0);
    let variable_marks = json!({
        "liveBefore": live_before,
        "liveAfter": marked.state.active_release_version,
        "badge": marked.state.canvas.release_badge,
        "lastBrew": marked.state.canvas.last_brew.map(|status| format!("{status:?}")),
    });

    let mut zoomed = runtime_for(StudioApp::from_registry().spell.clone());
    click(&mut zoomed, "mode-grimoire")?;
    zoomed.update(|app, _| app.canvas.camera.zoom = 0.45);
    zoomed.frame(1.0);
    let mut names = Vec::new();
    let mut stacked = Vec::new();
    for card in zoomed.state.canvas.nodes.clone() {
        let lines = text_lines_under(&zoomed, &format!("node-{}", card.id.0));
        let shown: Vec<(String, [f32; 4])> =
            lines.into_iter().filter(|(line, _)| card.inputs.iter().chain(card.outputs.iter()).any(|socket| &socket.name == line)).collect();
        for (index, (name, rect)) in shown.iter().enumerate() {
            for (other, other_rect) in shown.iter().skip(index + 1) {
                if rects_overlap(*rect, *other_rect) {
                    stacked.push(format!("{}: {name}/{other}", card.id.0));
                }
            }
        }
        names.extend(shown.into_iter().map(|(name, _)| format!("{}.{name}", card.id.0)));
    }
    let expected: usize = zoomed.state.canvas.nodes.iter().map(|card| card.inputs.len() + card.outputs.len()).sum();

    let mut shelf = runtime_for(StudioApp::from_registry().spell.clone());
    click(&mut shelf, "tab-agents")?;
    shelf.frame(1.0);
    let unbottled: Vec<String> = ["Support Agent", "Research Agent"]
        .iter()
        .map(|name| name.to_ascii_lowercase().replace(' ', "-"))
        .filter(|slug| shelf.find(&format!("spell-{slug}")).is_none() || shelf.find(&format!("potion-mark-{slug}")).is_none())
        .collect();

    Ok(json!({
        "unbottledPotions": unbottled,
        "pulledOff": pulled_off,
        "refusedDrop": refused_drop,
        "blankPublish": { "published": published, "releases": releases },
        "undoLink": undo_link,
        "variableMarks": variable_marks,
        "grimoire45": { "shown": names.len(), "expected": expected, "stacked": stacked },
    }))
}
fn steady_hands() -> Result<serde_json::Value, String> {
    let press = |rt: &mut Runtime<StudioApp>, key: rune::Key, cmd: bool| {
        rt.key(rune::KeyEvent { key, mods: rune::Mods { cmd, ..Default::default() } });
        rt.frame(1.0);
    };
    let near = |a: f32, b: f32| (a - b).abs() <= 3.0;
    let menu_case = |spell: SpellGraph, size: (f32, f32), aside: Option<&str>, target: &str| -> Result<serde_json::Value, String> {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        if let Some(id) = aside {
            let parked = NodeId::from(id);
            rt.update(move |app, _| app.toss(magicklala_canvas::Toss::Aside(parked.clone())));
            rt.frame(1.0);
        }
        let card = layout_rect(&rt, &format!("node-{target}")).ok_or("no card")?;
        right_click(&mut rt, (card[0] + card[2] * 0.5, card[1] + card[3] * 0.5));
        rt.frame(1.0);
        let card = layout_rect(&rt, &format!("node-{target}")).ok_or("no card")?;
        let panel = layout_rect(&rt, "context-menu").ok_or("no menu")?;
        let pot = pot_body(&rt);
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        let hides: Vec<String> = plan
            .cards
            .iter()
            .filter(|other| other.id.0 != target)
            .filter_map(|other| layout_rect(&rt, &format!("node-{}", other.id.0)).filter(|rect| rects_overlap(panel, *rect)).map(|_| other.id.0.clone()))
            .collect();
        let tether = layout_rect(&rt, "context-tether");
        let attached = tether.is_some_and(|t| {
            let below = panel[1] >= card[1] + card[3] - 1.0;
            let above = panel[1] + panel[3] <= card[1] + 1.0;
            let right = panel[0] >= card[0] + card[2] - 1.0;
            let left = panel[0] + panel[2] <= card[0] + 1.0;
            if below {
                near(t[1] + 4.0, card[1] + card[3]) && near(t[1] + t[3] - 4.0, panel[1])
            } else if above {
                near(t[1] + 4.0, panel[1] + panel[3]) && near(t[1] + t[3] - 4.0, card[1])
            } else if right {
                near(t[0] + 4.0, card[0] + card[2]) && near(t[0] + t[2] - 4.0, panel[0])
            } else if left {
                near(t[0] + 4.0, panel[0] + panel[2]) && near(t[0] + t[2] - 4.0, card[0])
            } else {
                false
            }
        });
        Ok(json!({ "coversPot": pot.is_some_and(|pot| rects_overlap(panel, pot)), "hides": hides, "attached": attached }))
    };
    let support = || StudioApp::from_registry().spell.clone();
    let menus = json!({
        "wide": menu_case(support(), (1280.0, 800.0), None, "llm")?,
        "aside": menu_case(support(), (1280.0, 800.0), Some("search"), "search")?,
        "compact": menu_case(support(), (960.0, 640.0), None, "llm")?,
    });

    let keyed = || {
        let mut rt = runtime_for(tasting_spell());
        rt.on_key = Some(magicklala_studio::studio_key);
        rt.update(|app, _| app.canvas.select(Some(NodeId::from("echo"))));
        rt.frame(1.0);
        rt
    };
    let mut model_menu = keyed();
    model_menu.update(|app, _| app.model_menu_open = true);
    press(&mut model_menu, rune::Key::Escape, false);
    let mut type_menu = keyed();
    type_menu.update(|app, _| {
        app.add_variable();
        app.open_var_type = Some(0);
    });
    press(&mut type_menu, rune::Key::Escape, false);
    let mut other_tab = keyed();
    other_tab.update(|app, _| app.right_tab = RightTab::Variables);
    press(&mut other_tab, rune::Key::Delete, false);
    let mut typing = keyed();
    typing.update(|app, _| app.right_tab = RightTab::Inspector);
    typing.frame(1.0);
    click(&mut typing, "prompt-field")?;
    let before_text = typing.state.prompts.get(&NodeId::from("echo")).map(|edit| edit.text.clone()).unwrap_or_default();
    press(&mut typing, rune::Key::Backspace, false);
    press(&mut typing, rune::Key::Command('z'), true);
    let keys = json!({
        "escapeClosesModelMenu": !model_menu.state.model_menu_open && model_menu.state.canvas.selection.is_some(),
        "escapeClosesTypeMenu": type_menu.state.open_var_type.is_none() && type_menu.state.canvas.selection.is_some(),
        "deleteOnVariablesKeepsStep": other_tab.state.canvas.card(&NodeId::from("echo")).is_some(),
        "typingKeepsStep": typing.state.canvas.card(&NodeId::from("echo")).is_some() && !before_text.is_empty(),
    });

    let mut both = runtime_for(tasting_spell());
    both.update(|app, _| {
        app.add_variable();
        if let Some(edit) = app.var_name_edits.get_mut(0) {
            *edit = rune::TextEdit::new("");
        }
        app.canvas.wires.retain(|wire| !(wire.to_node.0 == "reply" && wire.to_socket == "content"));
        app.sync_spell();
    });
    click(&mut both, "validate")?;
    let flagged =
        both.state.canvas.card(&NodeId::from("reply")).is_some_and(|card| card.inputs.iter().any(|socket| socket.flagged && socket.name == "content"));
    let errors = match &both.state.validation {
        ValidationState::Invalid(errors) => errors.clone(),
        _ => Vec::new(),
    };
    let findings = json!({ "variableNamed": errors.iter().any(|error| error.contains("needs a name")), "replyFlagged": flagged });

    let mut binding = SpellGraph::new("Bound");
    binding.nodes =
        vec![studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null), studio_node("read", "var/get", 280.0, json!({ "name": "greeting" }))];
    binding.variables = vec![magicklala_graph::VariableDef {
        name: "greeting".to_owned(),
        ty: SocketType::String,
        default: Some(magicklala_graph::Value::String("hi".to_owned())),
        exposed: true,
    }];
    let mut bound = runtime_for(binding);
    click(&mut bound, "tab-variables")?;
    bound.frame(1.0);
    let hint_before = bound.state.variable_binding_hint("greeting").0;
    bound.update(|app, _| {
        if let Some(edit) = app.var_name_edits.get_mut(0) {
            *edit = rune::TextEdit::new("");
        }
        app.sync_spell();
    });
    bound.frame(1.0);
    let stale_hint = all_texts(&bound).iter().any(|line| line == &hint_before);

    let mut orphans = Vec::new();
    let mut measurer = magicklala_studio::studio_text_system();
    for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
        let mut app = StudioApp::with_spell(example("parallel_research.json")?, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        for card in rt.state.canvas.nodes.clone() {
            let Some(frame) = layout_rect(&rt, &format!("node-{}-header", card.id.0)) else { continue };
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            for (line, rect) in text_lines_under(&rt, &format!("node-{}-header", card.id.0)) {
                let (natural, _) = measurer.measure(&line, rune::TextStyle::new(13.0, 750.0), 1.0);
                let wraps = natural * plan.scale > rect[2] + 1.0;
                let tail = line.rsplit(' ').next().unwrap_or_default();
                if wraps && (tail.chars().all(|c| c.is_ascii_digit()) || tail == "·") {
                    orphans.push(format!("{}@{}: lone \"{tail}\" under \"{line}\"", card.id.0, size.0));
                }
                if rect[1] + rect[3] > frame[1] + frame[3] - 3.0 {
                    orphans.push(format!("{}@{}: \"{line}\" touches the bottom", card.id.0, size.0));
                }
            }
        }
    }

    Ok(json!({ "menus": menus, "keys": keys, "findings": findings, "staleHint": stale_hint, "hintBefore": hint_before, "orphans": orphans }))
}
fn fair_shelves() -> Result<serde_json::Value, String> {
    let open = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let mut layouts = Vec::new();
    let mut bends = Vec::new();
    for (name, spell) in [("4", many_bottle_spell(4)), ("8", many_bottle_spell(8)), ("kitchen", crowded_kitchen())] {
        let rt = open(spell, (1280.0, 800.0));
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        if name != "kitchen" {
            layouts.push(json!({ "case": name, "stacked": plan.lanes.is_some() }));
        }
        let s = plan.scale;
        for (index, ribbon) in magicklala_canvas::cauldron::ribbon_paths(&plan).iter().enumerate() {
            let mouth = ribbon.last().copied().unwrap_or_default();
            let body: Vec<[f32; 2]> = ribbon.iter().copied().filter(|p| (p[0] - mouth[0]).hypot(p[1] - mouth[1]) > 24.0 * s).collect();
            let tightest = body
                .windows(5)
                .filter_map(|w| {
                    let (a, b, c) = (w[0], w[2], w[4]);
                    let ab = (b[0] - a[0]).hypot(b[1] - a[1]);
                    let bc = (c[0] - b[0]).hypot(c[1] - b[1]);
                    let ca = (a[0] - c[0]).hypot(a[1] - c[1]);
                    let area2 = ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs();
                    (area2 > 1e-3).then(|| ab * bc * ca / (2.0 * area2))
                })
                .fold(f32::MAX, f32::min);
            if tightest < 14.0 * s {
                bends.push(format!("{name}: ribbon {index} bends at {:.0}px", tightest));
            }
        }
    }

    let mut grim = runtime_for(StudioApp::from_registry().spell.clone());
    click(&mut grim, "mode-grimoire")?;
    grim.update(|app, _| {
        app.canvas.camera.zoom = 0.36;
        app.canvas.camera.pan_x = 4.0;
        app.canvas.camera.pan_y = 60.0;
    });
    grim.frame(1.0);
    let leftmost = grim.state.canvas.nodes.iter().min_by(|a, b| a.position[0].total_cmp(&b.position[0])).cloned().ok_or("no cards")?;
    let socket = leftmost.inputs.first().or(leftmost.outputs.first()).cloned().ok_or("no socket")?;
    let is_output = leftmost.inputs.is_empty();
    let hovered = (leftmost.id.clone(), socket.name.clone(), is_output);
    grim.update(move |app, _| app.canvas.hover_socket = Some(hovered.clone()));
    grim.frame(1.0);
    let canvas_rect = layout_rect(&grim, "canvas").ok_or("no canvas")?;
    let tip = layout_rect(&grim, "socket-tip");
    let elements = grim.elements();
    let tip_index = elements.iter().position(|element| element.key.as_deref() == Some("socket-tip"));
    let last_card = elements.iter().rposition(|element| element.key.as_deref().is_some_and(|key| key.starts_with("node-")));
    let tooltip = json!({
        "shown": tip.is_some(),
        "inside": tip.is_some_and(|t| t[0] >= canvas_rect[0] && t[0] + t[2] <= canvas_rect[0] + canvas_rect[2] && t[1] >= canvas_rect[1]),
        "onTop": matches!((tip_index, last_card), (Some(tip), Some(card)) if tip > card),
    });

    let bottle_kind = |spell: SpellGraph| {
        let rt = open(spell, (960.0, 640.0));
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        plan.with_role(magicklala_canvas::BrewRole::Bottle)
            .any(|card| text_lines_under(&rt, &format!("node-{}", card.id.0)).iter().any(|(line, _)| line == "io/send_message"))
    };
    let kinds = json!({ "dozen": bottle_kind(dozen_ingredient_spell()), "support": bottle_kind(StudioApp::from_registry().spell.clone()) });

    let mut hexed = open(example("parallel_research.json")?, (960.0, 640.0));
    let plan = magicklala_canvas::brew_plan(&hexed.state.canvas, hexed.state.canvas_size());
    let mirrored = plan
        .cards
        .iter()
        .find(|card| card.mirrored && hexed.state.canvas.card(&card.id).is_some_and(|c| c.inputs.iter().any(|s| s.name == "prompt")))
        .map(|card| card.id.clone())
        .ok_or("no mirrored model")?;
    let cut = mirrored.clone();
    hexed.update(move |app, _| {
        app.canvas.wires.retain(|wire| !(wire.to_node == cut && wire.to_socket == "prompt"));
        app.prompts.insert(cut.clone(), rune::TextEdit::new(""));
        app.sync_spell();
    });
    click(&mut hexed, "validate")?;
    hexed.frame(1.0);
    let card_rect = layout_rect(&hexed, &format!("node-{}", mirrored.0)).ok_or("no card")?;
    let pill = layout_rect(&hexed, &format!("status-pill-{}", mirrored.0));
    let pill_inside = pill.is_some_and(|p| p[0] >= card_rect[0] - 1.0 && p[0] + p[2] <= card_rect[0] + card_rect[2] + 1.0);

    let mut fresh = runtime_for(tasting_spell());
    click(&mut fresh, "mode-grimoire")?;
    let model = fresh.state.spawn("ai/generate_text").ok_or("no model")?;
    let seeded = fresh.state.prompts.get(&model).map(|edit| edit.text.clone()).unwrap_or_default();
    let mut words = SpellGraph::new("Words");
    words.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("ask", "ai/generate_text", 280.0, json!({ "prompt": "Reply in a {{mood}} voice" })),
        studio_node("say", "io/send_message", 560.0, serde_json::Value::Null),
    ];
    words.wires = vec![
        studio_wire("w1", ("trigger", "out"), ("ask", "in")),
        studio_wire("w2", ("ask", "out"), ("say", "in")),
        studio_wire("w3", ("ask", "text"), ("say", "content")),
    ];
    let mut worded = runtime_for(words.clone());
    click(&mut worded, "validate")?;
    let unfilled = worded.state.validation.label();
    worded.update(|app, _| app.canvas.select(Some(NodeId::from("ask"))));
    worded.frame(0.0);
    let unfilled_says = texts_under(&mut worded, "prompt-literal-note");
    let mut templated = words;
    templated.nodes[1] = studio_node("ask", "text/template", 280.0, json!({ "template": "Reply in a {{mood}} voice" }));
    templated.wires = vec![studio_wire("w1", ("trigger", "out"), ("say", "in")), studio_wire("w3", ("ask", "text"), ("say", "content"))];
    templated.variables = vec![magicklala_graph::VariableDef {
        name: "mood".to_owned(),
        ty: SocketType::String,
        default: Some(magicklala_graph::Value::String("calm".to_owned())),
        exposed: true,
    }];
    let mut filled = runtime_for(templated);
    click(&mut filled, "validate")?;
    let prompts = json!({
        "seededTokens": seeded.matches("{{").count(),
        "unfilledCheck": unfilled,
        "namesWord": unfilled_says.contains("{{mood}}"),
        "filledCheck": filled.state.validation.label(),
    });

    Ok(json!({ "layouts": layouts, "bends": bends, "tooltip": tooltip, "bottleKinds": kinds, "pillInside": pill_inside, "prompts": prompts }))
}
fn last_mile(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut both = studio_at(db_path)?;
    both.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(tasting_spell());
    });
    both.update(|app, _| {
        app.add_variable();
        if let Some(edit) = app.var_name_edits.get_mut(0) {
            *edit = rune::TextEdit::new("");
        }
        app.prompts.insert(NodeId::from("echo"), rune::TextEdit::new(""));
        app.sync_spell();
    });
    let published = both.state.publish().is_ok();
    let errors_of = |app: &StudioApp| match &app.validation {
        ValidationState::Invalid(errors) => errors.clone(),
        _ => Vec::new(),
    };
    let mut after_publish = errors_of(&both.state);
    click(&mut both, "validate")?;
    let mut after_check = errors_of(&both.state);
    after_publish.sort();
    after_check.sort();
    let publish = json!({ "published": published, "publishCount": after_publish.len(), "same": after_publish == after_check, "checkCount": after_check.len() });

    let mut vars = runtime_for(tasting_spell());
    vars.update(|app, _| app.add_variable());
    let added = vars.state.spell.variables.len();
    vars.update(|app, _| app.undo());
    let variable_undo = json!({ "added": added, "afterUndo": vars.state.spell.variables.len() });

    let mut brewing = slow_runtime(3000, Duration::from_secs(20));
    brewing.on_key = Some(magicklala_studio::studio_key);
    brewing.update(|app, _| app.add_variable());
    let entries = brewing.state.undo_stack.len();
    click(&mut brewing, "run-spell")?;
    brewing.frame(1.0);
    brewing.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
    brewing.frame(1.0);
    let kept = brewing.state.undo_stack.len() == entries && brewing.state.is_running();
    brewing.update(|app, _| app.cancel_run());

    let mut empty = runtime_for(SpellGraph::new("Fresh"));
    empty.update(|app, _| app.left_tab = LeftTab::Agents);
    empty.frame(1.0);
    let starters_in_pot: Vec<String> = text_lines_under(&empty, "canvas").into_iter().map(|(line, _)| line).filter(|line| line.starts_with("On ")).collect();
    let starters_in_panel: Vec<String> =
        text_lines_under(&empty, "inspector").into_iter().map(|(line, _)| line).filter(|line| line.starts_with("On ")).collect();
    let canvas_rect = layout_rect(&empty, "canvas").ok_or("no canvas")?;
    let starter_inside =
        layout_rect(&empty, "starter-event-on_message").is_some_and(|r| r[0] >= canvas_rect[0] && r[0] + r[2] <= canvas_rect[0] + canvas_rect[2]);
    let empty_pot = json!({ "inPot": starters_in_pot.len(), "inPanel": starters_in_panel.len(), "starterOnShelf": starter_inside });

    Ok(json!({ "publish": publish, "variableUndo": variable_undo, "undoDuringBrewKept": kept, "emptyPot": empty_pot }))
}
fn honest_edits(db_path: &Path) -> Result<serde_json::Value, String> {
    let chain = |ids: &[(&str, &str)]| -> SpellGraph {
        let mut spell = SpellGraph::new("Chain Agent");
        for (index, (id, kind)) in ids.iter().enumerate() {
            spell.nodes.push(studio_node(id, kind, 240.0 * index as f32, serde_json::Value::Null));
        }
        spell
    };
    let mut branch_spell = chain(&[("trigger", "event/on_message"), ("fork", "flow/branch")]);
    branch_spell.wires = vec![studio_wire("w1", ("trigger", "out"), ("fork", "in"))];
    let mut branch = runtime_for(branch_spell);
    branch.update(|app, _| {
        app.spawn("io/send_message");
        app.spawn("io/send_message");
        app.sync_edits();
    });
    let mut branch_outs: Vec<String> = branch
        .state
        .canvas
        .wires
        .iter()
        .filter(|wire| wire.from_node.0 == "fork" && branch.state.canvas.card(&wire.to_node).is_some_and(|card| card.kind == "io/send_message"))
        .map(|wire| wire.from_socket.clone())
        .collect();
    branch_outs.sort();

    let mut cut_spell = chain(&[
        ("trigger", "event/on_message"),
        ("a", "time/delay"),
        ("b", "time/delay"),
        ("c", "time/delay"),
        ("d", "time/delay"),
        ("reply", "io/send_message"),
    ]);
    cut_spell.wires = vec![
        studio_wire("w1", ("trigger", "out"), ("a", "in")),
        studio_wire("w2", ("a", "out"), ("b", "in")),
        studio_wire("w3", ("b", "out"), ("c", "in")),
        studio_wire("w4", ("c", "out"), ("d", "in")),
        studio_wire("w5", ("d", "out"), ("reply", "in")),
        studio_wire("w6", ("trigger", "content"), ("reply", "content")),
    ];
    let mut cut = runtime_for(cut_spell);
    let shortcut = |app: &StudioApp| app.canvas.wires.iter().any(|wire| wire.from_node.0 == "trigger" && wire.to_node.0 == "c" && wire.from_socket == "out");
    cut.update(|app, _| {
        app.toss(magicklala_canvas::Toss::Aside(NodeId::from("a")));
        app.sync_edits();
        app.toss(magicklala_canvas::Toss::Aside(NodeId::from("b")));
        app.sync_edits();
    });
    let bridged = shortcut(&cut.state);
    cut.update(|app, _| {
        app.canvas.wires.retain(|wire| !(wire.from_node.0 == "trigger" && wire.to_node.0 == "c" && wire.from_socket == "out"));
        app.sync_edits();
        app.toss(magicklala_canvas::Toss::Aside(NodeId::from("d")));
        app.sync_edits();
        app.sync_edits();
    });
    let cut_report = json!({ "bridged": bridged, "cameBack": shortcut(&cut.state) });

    let mut resized = runtime_for(tasting_spell());
    resized.set_viewport((960.0, 640.0), 1.0);
    resized.frame(0.0);
    resized.frame(0.0);
    let resize = json!({ "size": [resized.state.size.0, resized.state.size.1], "compact": resized.state.compact_toolbar() });

    let mut drop = runtime_for(tasting_spell());
    click(&mut drop, "run-spell")?;
    live_finish(&mut drop)?;
    click(&mut drop, "mode-grimoire")?;
    drop.update(|app, _| {
        app.canvas.camera.zoom = 1.0;
        app.canvas.camera.pan_x = 40.0;
        app.canvas.camera.pan_y = 40.0;
    });
    drop.frame(1.0);
    let order = |app: &StudioApp| app.spell.wires.iter().map(|wire| wire.id.0.clone()).collect::<Vec<_>>();
    let before = (order(&drop.state), drop.state.dirty, drop.state.canvas.last_brew.is_some());
    let from = drop.find("socket-echo-in-content").ok_or("no echo content socket")?;
    let onto = drop.find("socket-reply-in-in").ok_or("no reply flow socket")?;
    drag(&mut drop, "socket-echo-in-content", (onto.0 - from.0, onto.1 - from.1))?;
    drop.update(|app, _| {
        app.sync_edits();
    });
    drop.frame(0.0);
    let refused_drop = json!({
        "sameOrder": order(&drop.state) == before.0,
        "dirtyBefore": before.1,
        "dirtyAfter": drop.state.dirty,
        "brewKept": before.2 && drop.state.canvas.last_brew.is_some(),
        "storeError": drop.state.store_error,
        "wireError": drop.state.wire_error.is_some(),
        "wireErrorShown": texts_under(&mut drop, "trace-strip").contains("Wire refused"),
    });

    let mut both = studio_at(db_path)?;
    both.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(tasting_spell());
        app.prompts.insert(NodeId::from("echo"), rune::TextEdit::new(""));
        app.sync_spell();
    });
    let marks = |app: &StudioApp| {
        let failed: Vec<String> = app.canvas.nodes.iter().filter(|card| card.status == ExecutionStatus::Failed).map(|card| card.id.0.clone()).collect();
        let rejected: Vec<String> = app.trace.iter().filter(|line| line.kind == "RunRejected").map(|line| line.detail.clone()).collect();
        (failed, rejected)
    };
    let refusal = both.state.publish().err().map(|error| error.to_string()).unwrap_or_default();
    let after_publish = marks(&both.state);
    click(&mut both, "validate")?;
    let after_check = marks(&both.state);
    let publish_marks = json!({
        "failed": after_publish.0,
        "sameAsCheck": after_publish == after_check,
        "rejected": after_publish.1.len(),
        "debugIds": refusal.contains("node \""),
    });

    let mut lone = runtime_for(chain(&[("trigger", "event/on_message")]));
    click(&mut lone, "run-spell")?;
    live_finish(&mut lone)?;
    let brewed = lone.state.canvas.last_brew.is_some();
    lone.update(|app, _| {
        app.toss(magicklala_canvas::Toss::Aside(NodeId::from("trigger")));
        app.sync_edits();
    });
    let lone_report = json!({ "brewed": brewed, "marksAfterAside": lone.state.canvas.last_brew.is_some() });

    let mut typed = runtime_for(tasting_spell());
    typed.update(|app, _| app.add_variable());
    click(&mut typed, "validate")?;
    let checked = typed.state.validation.label().to_owned();
    typed.update(|app, _| {
        app.set_variable_type_at(0, SocketType::Int);
        app.sync_edits();
    });
    let after_type = typed.state.validation.label().to_owned();
    let new_type = typed.state.spell.variables.first().map(|var| format!("{:?}", var.ty));
    typed.update(|app, _| app.undo());
    let variable_type = json!({ "checked": checked, "afterType": after_type, "type": new_type, "undone": typed.state.spell.variables.first().map(|var| format!("{:?}", var.ty)) });

    let mut research = runtime_for(tasting_spell());
    research.update(|app, _| app.open_sample_spell("Research Agent"));
    let into_prompt: Vec<magicklala_graph::WireId> =
        research.state.canvas.wires.iter().filter(|wire| wire.to_node.0 == "llm" && wire.to_socket == "prompt").map(|wire| wire.id.clone()).collect();
    research.update(move |app, _| {
        for wire in &into_prompt {
            app.unplug_wire(wire);
        }
    });
    click(&mut research, "validate")?;
    let research_report = json!({ "validation": research.state.validation.label(), "errors": match &research.state.validation {
        ValidationState::Invalid(errors) => errors.clone(),
        _ => Vec::new(),
    } });

    let mut hexed = runtime_for(tasting_spell());
    hexed.update(|app, _| {
        app.prompts.insert(NodeId::from("echo"), rune::TextEdit::new(""));
    });
    click(&mut hexed, "validate")?;
    let hexed_mood = format!("{:?}", magicklala_studio::potion_mood(&hexed.state, "Tasting Agent"));
    let other_mood = format!("{:?}", magicklala_studio::potion_mood(&hexed.state, "Research Agent"));
    click(&mut hexed, "run-spell")?;
    click(&mut hexed, "tab-playground")?;
    let reason = hexed.state.canvas.brew_summary.clone().unwrap_or_default();
    let tasting_reason = !reason.is_empty() && texts_under(&mut hexed, "playground-panel").contains(&reason);
    let moods = json!({ "hexed": hexed_mood, "other": other_mood });

    let mut keyed = runtime_for(tasting_spell());
    keyed.on_key = Some(magicklala_studio::studio_key);
    keyed.update(|app, _| app.add_variable());
    click(&mut keyed, "tab-variables")?;
    keyed.update(|app, _| app.remove_variable_at(0));
    keyed.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
    keyed.frame(0.0);
    let undo_on_variables = keyed.state.spell.variables.len();

    let mut undeclared_spell = tasting_spell();
    undeclared_spell.nodes.push(studio_node("peek", "var/set", 280.0, json!({ "name": "mood" })));
    undeclared_spell.wires.retain(|wire| wire.id.0 != "w1");
    undeclared_spell.wires.push(studio_wire("w4", ("trigger", "out"), ("peek", "in")));
    undeclared_spell.wires.push(studio_wire("w5", ("peek", "out"), ("reply", "in")));
    undeclared_spell.wires.push(studio_wire("w6", ("trigger", "content"), ("peek", "value")));
    let mut undeclared = runtime_for(undeclared_spell);
    click(&mut undeclared, "validate")?;
    let undeclared_named = match &undeclared.state.validation {
        ValidationState::Invalid(errors) => errors.iter().any(|error| error.contains("mood")),
        _ => false,
    };

    Ok(json!({
        "branchOuts": branch_outs,
        "cut": cut_report,
        "resize": resize,
        "refusedDrop": refused_drop,
        "publishMarks": publish_marks,
        "loneAside": lone_report,
        "variableType": variable_type,
        "research": research_report,
        "moods": moods,
        "tastingReason": tasting_reason,
        "undoOnVariables": undo_on_variables,
        "undeclaredNamed": undeclared_named,
    }))
}
fn legible_shelves() -> Result<serde_json::Value, String> {
    let scene = |spell: SpellGraph, size: (f32, f32), pantry: bool, brew: bool, zoom_out: usize| -> Result<Runtime<StudioApp>, String> {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        if pantry {
            click(&mut rt, "expand-nodes")?;
        }
        if brew {
            click(&mut rt, "run-spell")?;
            live_finish(&mut rt)?;
        }
        for _ in 0..zoom_out {
            click(&mut rt, "zoom-out")?;
        }
        rt.frame(3.0);
        Ok(rt)
    };
    let reference = {
        let rt = scene(tasting_spell(), (1280.0, 800.0), false, false, 0)?;
        rt.elements().iter().filter(|element| element.text.as_deref() == Some("Not brewed yet")).map(|element| element.rect[3]).next().unwrap_or(0.0)
    };
    let cases = [
        ("kitchen-pantry@960", crowded_kitchen(), (960.0_f32, 640.0_f32), true, false, 0usize),
        ("kitchen-brewed@1280", crowded_kitchen(), (1280.0, 800.0), false, true, 0),
        ("kitchen-brewed@960", crowded_kitchen(), (960.0, 640.0), false, true, 0),
        ("eight@960", many_bottle_spell(8), (960.0, 640.0), false, true, 0),
        ("router-brewed@960", example("webhook_router.json")?, (960.0, 640.0), false, true, 0),
        ("single-min-zoom@1280", tasting_spell(), (1280.0, 800.0), false, true, 12),
        ("dozen@960", dozen_ingredient_spell(), (960.0, 640.0), false, false, 0),
    ];
    let mut report = serde_json::Map::new();
    for (name, spell, size, pantry, brew, zoom_out) in cases {
        let rt = scene(spell, size, pantry, brew, zoom_out)?;
        let canvas = rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
        let lines = text_lines_under(&rt, "canvas");
        let mut small: Vec<(String, f32)> = lines
            .iter()
            .filter(|(line, rect)| !line.trim().is_empty() && rect[3] < reference * 10.5 / 12.0 - 0.1 && inside(canvas, *rect))
            .map(|(line, rect)| (line.clone(), (rect[3] * 10.0).round() / 10.0))
            .collect();
        small.sort_by(|a, b| a.1.total_cmp(&b.1));
        small.dedup();
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        let vialless: Vec<String> = plan
            .with_role(magicklala_canvas::BrewRole::Ingredient)
            .filter(|card| rt.presented(&format!("vial-{}", card.id.0)).is_none())
            .map(|card| card.id.0.clone())
            .collect();
        let pot = rt.presented("cauldron-center").map(|(rect, _)| rect[2]).unwrap_or(0.0);
        report.insert(
            name.to_owned(),
            json!({ "tooSmall": small.iter().take(6).map(|(line, h)| format!("{line} {h}")).collect::<Vec<_>>(), "vialless": vialless.len(), "potWidth": pot.round() }),
        );
    }
    Ok(json!({ "reference": reference, "cases": report }))
}
fn clear_sight() -> Result<serde_json::Value, String> {
    let mut menu_rt = sized_runtime((960.0, 640.0), Theme::Light);
    let trigger = menu_rt.state.canvas.nodes.iter().find(|card| card.kind == "event/on_message").map(|card| card.id.clone()).ok_or("no trigger")?;
    let header = menu_rt.find(&format!("node-{}-header", trigger.0)).ok_or("no trigger card")?;
    right_click(&mut menu_rt, header);
    menu_rt.frame(1.0);
    let menu = rect(&menu_rt, "context-menu").ok_or("no menu")?;
    let plan = magicklala_canvas::brew_plan(&menu_rt.state.canvas, menu_rt.state.canvas_size());
    let origin = menu_rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    let covered: Vec<String> = plan
        .cards
        .iter()
        .filter(|card| card.id != trigger)
        .filter(|card| rects_overlap([card.rect[0] + origin[0], card.rect[1] + origin[1], card.rect[2], card.rect[3]], menu))
        .map(|card| card.id.0.clone())
        .collect();

    let mut grimoire = runtime_for(example("webhook_router.json")?);
    click(&mut grimoire, "mode-grimoire")?;
    let mut crowded = Vec::new();
    for zoom in [0.6_f32, 0.64, 0.68, 0.75, 1.0] {
        grimoire.update(move |app, _| app.canvas.camera.zoom = zoom);
        grimoire.frame(1.0);
        let ids: Vec<String> = grimoire.state.canvas.nodes.iter().map(|card| card.id.0.clone()).collect();
        for id in ids {
            let Some(frame) = layout_rect(&grimoire, &format!("node-{id}-header")) else { continue };
            for (line, r) in text_lines_under(&grimoire, &format!("node-{id}-header")) {
                if r[1] < frame[1] - 0.5 || r[1] + r[3] > frame[1] + frame[3] + 0.5 {
                    crowded.push(format!("{zoom}: {id} {line}"));
                }
            }
        }
    }
    crowded.dedup();
    let mut last = sized_runtime((960.0, 640.0), Theme::Light);
    let llm = last.state.canvas.nodes.iter().find(|card| card.kind == "ai/generate_text").map(|card| card.id.clone()).ok_or("no llm")?;
    let spot = last.find(&format!("node-{}-header", llm.0)).ok_or("no llm card")?;
    right_click(&mut last, spot);
    last.frame(1.0);
    let docked_menu = rect(&last, "context-menu").ok_or("no docked menu")?;
    let inspector = rect(&last, "inspector").ok_or("no inspector")?;
    let in_pane = docked_menu[0] >= inspector[0] - 0.5 && docked_menu[0] + docked_menu[2] <= inspector[0] + inspector[2] + 0.5;
    let menu_words: Vec<[f32; 4]> = text_lines_under(&last, "context-menu").into_iter().map(|(_, rect)| rect).collect();
    let hidden_by_menu: Vec<String> = text_lines_under(&last, "inspector")
        .into_iter()
        .filter(|(_, rect)| !menu_words.contains(rect))
        .filter(|(_, rect)| {
            rect[0] < docked_menu[0] + docked_menu[2]
                && docked_menu[0] < rect[0] + rect[2]
                && rect[1] < docked_menu[1] + docked_menu[3]
                && docked_menu[1] < rect[1] + rect[3]
        })
        .map(|(line, _)| line)
        .collect();
    let before = last.state.canvas.nodes.len();
    click(&mut last, "context-duplicate-node")?;
    let docked = json!({ "inRecipePane": in_pane, "hiddenByMenu": hidden_by_menu, "duplicateWorks": last.state.canvas.nodes.len() == before + 1 });
    if let Ok(dir) = std::env::var("CLEAR_SIGHT_SHOTS") {
        let mut again = sized_runtime((960.0, 640.0), Theme::Light);
        right_click(&mut again, spot);
        again.frame(1.0);
        let app = std::mem::replace(&mut again.state, StudioApp::from_registry());
        render_headless_snapshot_state(&PathBuf::from(dir).join("docked-menu-960.png"), app).map_err(|error| error.to_string())?;
    }
    Ok(json!({ "menuCovers": covered, "headerSpills": crowded.into_iter().take(8).collect::<Vec<_>>(), "docked": docked }))
}
fn mixed_spell(ingredients: usize, bottles: usize) -> SpellGraph {
    let mut spell = SpellGraph::new(format!("{ingredients} steps {bottles} bottles"));
    spell.nodes.push(studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null));
    let mut previous = "trigger".to_owned();
    for index in 1..ingredients {
        let id = format!("wait_{index}");
        spell.nodes.push(studio_node(&id, "time/delay", 200.0 * index as f32, json!({ "ms": 0 })));
        spell.wires.push(studio_wire(&format!("chain_{index}"), (&previous, "out"), (&id, "in")));
        previous = id;
    }
    for index in 0..bottles {
        let id = format!("reply_{index}");
        spell.nodes.push(studio_node(&id, "io/send_message", 2000.0, serde_json::Value::Null));
        spell.wires.push(studio_wire(&format!("pour_{index}"), (&previous, "out"), (&id, "in")));
        spell.wires.push(studio_wire(&format!("words_{index}"), ("trigger", "content"), (&id, "content")));
    }
    spell
}

fn mixed_shelves(args: &[String]) -> Result<serde_json::Value, String> {
    let mut report = Vec::new();
    for ingredients in [6usize, 8] {
        for bottles in [2usize, 3, 4, 6] {
            for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
                let mut app = StudioApp::with_spell(mixed_spell(ingredients, bottles), registry_specs());
                app.size = size;
                let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
                rt.frame(1.0);
                let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
                let s = plan.scale;
                let mut through = Vec::new();
                for (id, points) in magicklala_canvas::cauldron::pour_paths(&rt.state.canvas, &plan) {
                    let body = &points[points.len().min(3)..points.len().saturating_sub(3)];
                    for card in plan.cards.iter().filter(|card| card.id != id) {
                        let r = card.rect;
                        let halo = 3.0 * s;
                        let badge = if card.role == magicklala_canvas::BrewRole::Ingredient && plan.lanes.is_some() { 10.0 * s } else { 0.0 };
                        if body.iter().any(|p| p[0] > r[0] - halo - badge && p[0] < r[0] + r[2] + halo && p[1] > r[1] - halo && p[1] < r[1] + r[3] + halo) {
                            through.push(format!("{}>{}", id.0, card.id.0));
                        }
                    }
                }
                let case = format!("{ingredients}+{bottles}@{}", size.0);
                if let Some(dir) = args.get(1) {
                    let app = std::mem::replace(&mut rt.state, StudioApp::from_registry());
                    render_headless_snapshot_state(&PathBuf::from(dir).join(format!("mixed_{ingredients}_{bottles}_{}.png", size.0)), app)
                        .map_err(|error| error.to_string())?;
                }
                let columns = {
                    let mut xs: Vec<f32> =
                        plan.with_role(magicklala_canvas::BrewRole::Ingredient).filter(|card| !card.mirrored).map(|card| card.rect[0]).collect();
                    xs.sort_by(f32::total_cmp);
                    xs.dedup_by(|a, b| (*a - *b).abs() < 1.0);
                    xs.len()
                };
                let shape = if plan.lanes.is_some() {
                    "lanes"
                } else if columns > 1 {
                    "back-row ring"
                } else {
                    "ring"
                };
                report.push(json!({ "case": case, "shape": shape, "through": through }));
            }
        }
    }
    Ok(json!(report))
}
fn undo_order() -> Result<serde_json::Value, String> {
    let cmd_z = |rt: &mut Runtime<StudioApp>| {
        rt.key(rune::KeyEvent { key: rune::Key::Command('z'), mods: rune::Mods { cmd: true, ..Default::default() } });
        rt.frame(0.0);
    };
    let remove_echo = |rt: &mut Runtime<StudioApp>| {
        rt.update(|app, _| {
            app.canvas.select(Some(NodeId::from("echo")));
            app.delete_selected_node();
            app.sync_edits();
        });
        rt.frame(0.0);
    };
    let fed_by_trigger = |app: &StudioApp| {
        app.canvas
            .wires
            .iter()
            .any(|wire| wire.from_node.0 == "trigger" && wire.from_socket == "sender" && wire.to_node.0 == "reply" && wire.to_socket == "content")
    };
    let mut later = runtime_for(tasting_spell());
    later.on_key = Some(magicklala_studio::studio_key);
    remove_echo(&mut later);
    let removed = later.state.canvas.card(&NodeId::from("echo")).is_none();
    later.update(|app, _| {
        let _ = app
            .canvas
            .connect_sockets(&SocketRef::new(NodeId::from("trigger"), "sender".to_owned()), &SocketRef::new(NodeId::from("reply"), "content".to_owned()));
        app.sync_edits();
    });
    later.frame(0.0);
    cmd_z(&mut later);
    let wire_undone = !fed_by_trigger(&later.state);
    let step_kept_after_first = later.state.canvas.card(&NodeId::from("echo")).is_none();
    cmd_z(&mut later);
    let step_back_after_second = later.state.canvas.card(&NodeId::from("echo")).is_some();

    let mut tracked = runtime_for(tasting_spell());
    tracked.on_key = Some(magicklala_studio::studio_key);
    remove_echo(&mut tracked);
    cmd_z(&mut tracked);
    let undo_after_tracked = tracked.state.canvas.card(&NodeId::from("echo")).is_some();

    let mut typed = runtime_for(tasting_spell());
    typed.on_key = Some(magicklala_studio::studio_key);
    typed.update(|app, _| app.add_variable());
    for draft in ["C", "Ch", "Changed: {{content}}"] {
        typed.update(move |app, _| {
            app.prompts.insert(NodeId::from("echo"), rune::TextEdit::new(draft));
            app.sync_edits();
        });
        typed.frame(0.0);
    }
    cmd_z(&mut typed);
    let prompt_after_one = typed.state.prompts.get(&NodeId::from("echo")).map(|edit| edit.text.clone());
    let variables_after_one = typed.state.spell.variables.len();
    cmd_z(&mut typed);
    let variables_after_two = typed.state.spell.variables.len();

    let mut listed = runtime_for(router_spell());
    listed.on_key = Some(magicklala_studio::studio_key);
    listed.update(|app, _| {
        app.prompts.insert(NodeId::from("switch"), rune::TextEdit::new("refund, billing"));
        app.sync_edits();
    });
    listed.frame(0.0);
    let cases_before_undo = outputs_of(&listed.state, "switch");
    cmd_z(&mut listed);
    let cases_after_undo = outputs_of(&listed.state, "switch");

    Ok(json!({
        "removed": removed,
        "wireUndone": wire_undone,
        "stepKeptAfterFirst": step_kept_after_first,
        "stepBackAfterSecond": step_back_after_second,
        "undoAfterTracked": undo_after_tracked,
        "typed": { "promptAfterOne": prompt_after_one, "variablesAfterOne": variables_after_one, "variablesAfterTwo": variables_after_two },
        "listed": { "before": cases_before_undo, "after": cases_after_undo },
    }))
}
fn potion_rack() -> Result<serde_json::Value, String> {
    let cases = [
        ("kitchen-pantry@960", crowded_kitchen(), (960.0_f32, 640.0_f32), true, false),
        ("kitchen-brewed@1280", crowded_kitchen(), (1280.0, 800.0), false, true),
        ("eight-brewed@1280", many_bottle_spell(8), (1280.0, 800.0), false, true),
        ("eight@960", many_bottle_spell(8), (960.0, 640.0), false, false),
        ("router-brewed@1280", example("webhook_router.json")?, (1280.0, 800.0), false, true),
        ("three@960", many_bottle_spell(3), (960.0, 640.0), false, false),
    ];
    let mut report = serde_json::Map::new();
    for (name, spell, size, pantry, brew) in cases {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        if pantry {
            click(&mut rt, "expand-nodes")?;
        }
        if brew {
            click(&mut rt, "run-spell")?;
            live_finish(&mut rt)?;
        }
        rt.frame(3.0);
        rt.frame(6.0);
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        let mut measure = studio_text_system();
        let cut: Vec<String> = rt
            .state
            .canvas
            .nodes
            .iter()
            .filter(|card| plan.card(&card.id).is_some_and(|placed| placed.role == magicklala_canvas::BrewRole::Ingredient))
            .filter_map(|card| {
                let title = rt.state.canvas.display_title(&card.id);
                let (_, rect) = text_lines_under(&rt, &format!("node-{}", card.id.0)).into_iter().find(|(line, _)| line.replace('\u{a0}', " ") == title)?;
                let (natural, line_h) = measure.measure(&title, rune::TextStyle::new(13.0_f32.max(10.5 / plan.scale), 750.0), 1.0);
                let lines = (rect[3] / (line_h * plan.scale)).round().max(1.0);
                (natural * plan.scale > rect[2] * lines + 3.0).then_some(title)
            })
            .collect();
        let mut tops: Vec<f32> = plan.with_role(magicklala_canvas::BrewRole::Bottle).map(|card| card.rect[1]).collect();
        tops.sort_by(f32::total_cmp);
        tops.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        let mut pipes: Vec<f32> = magicklala_canvas::cauldron::ribbon_paths(&plan)
            .iter()
            .filter_map(|ribbon| ribbon.get(ribbon.len().saturating_sub(2)).map(|point| point[1]))
            .collect();
        pipes.sort_by(f32::total_cmp);
        pipes.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        let origin = rt.presented("canvas").map(|(rect, _)| [rect[0], rect[1]]).ok_or("no canvas")?;
        let bottles: Vec<(String, [f32; 4], String)> = rt
            .state
            .canvas
            .nodes
            .iter()
            .filter(|card| plan.card(&card.id).is_some_and(|placed| placed.role == magicklala_canvas::BrewRole::Bottle))
            .filter_map(|card| {
                rt.presented(&format!("node-{}-header", card.id.0)).map(|(rect, _)| (card.id.0.clone(), rect, rt.state.canvas.display_title(&card.id)))
            })
            .collect();
        let wrapped: Vec<String> = bottles
            .iter()
            .filter(|(id, _, title)| {
                let (_, line_h) = measure.measure(title, rune::TextStyle::new(12.5_f32.max(10.5 / plan.scale), 750.0), 1.0);
                text_lines_under(&rt, &format!("node-{id}-header"))
                    .iter()
                    .find(|(line, _)| line.replace('\u{a0}', " ") == *title)
                    .is_none_or(|(_, rect)| rect[3] > line_h * plan.scale * 1.5)
            })
            .map(|(_, _, title)| title.clone())
            .collect();
        let overlaps = |a: [f32; 4], b: [f32; 4]| a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
        let kind = rt.presented("rack-kind").map(|(rect, _)| rect);
        let rack = rt.presented("bottle-rack").map(|(rect, _)| rect);
        let lowest_tag = bottles
            .iter()
            .flat_map(|(id, _, _)| text_lines_under(&rt, &format!("node-{id}-header")).into_iter().map(|(_, rect)| rect[1] + rect[3]))
            .fold(f32::MIN, f32::max);
        let kind_gap = kind.map(|kind| ((kind[1] - lowest_tag) / plan.scale).round());
        let kind_clash: Vec<String> =
            kind.map_or_else(Vec::new, |kind| bottles.iter().filter(|(_, rect, _)| overlaps(kind, *rect)).map(|(_, _, title)| title.clone()).collect());
        let kind_centered = match (kind, rack) {
            (Some(kind), Some(rack)) => ((kind[0] + kind[2] * 0.5) - (rack[0] + rack[2] * 0.5)).abs() < 3.0,
            _ => true,
        };
        let mut grazes: Vec<String> = Vec::new();
        for pipe in magicklala_canvas::cauldron::rack_glass(&plan) {
            for pair in pipe.windows(2) {
                let (a, b) = ([pair[0][0] + origin[0], pair[0][1] + origin[1]], [pair[1][0] + origin[0], pair[1][1] + origin[1]]);
                if (a[1] - b[1]).abs() > 0.5 {
                    continue;
                }
                let (left, right) = (a[0].min(b[0]), a[0].max(b[0]));
                for (_, rect, title) in &bottles {
                    let above = rect[1] + rect[3] <= a[1] + 1.0 && rect[1] < a[1];
                    if above && rect[0] < right && left < rect[0] + rect[2] && a[1] - (rect[1] + rect[3]) < 6.0 * plan.scale {
                        grazes.push(format!("{title} {:.1}", a[1] - (rect[1] + rect[3])));
                    }
                }
            }
        }
        grazes.sort();
        grazes.dedup();
        let planks: Vec<String> = magicklala_canvas::cauldron::rack_planks(&plan)
            .map(|(frame, rows)| {
                rows.iter()
                    .filter_map(|row| {
                        let row_bottles: Vec<&magicklala_canvas::BrewCard> = plan
                            .with_role(magicklala_canvas::BrewRole::Bottle)
                            .filter(|card| (card.rect[1] + 68.0 * plan.scale - frame[1] - row[0]).abs() < 1.0)
                            .collect();
                        let left = row_bottles.iter().map(|card| card.rect[0]).fold(f32::MAX, f32::min) - frame[0];
                        let right = row_bottles.iter().map(|card| card.rect[0] + card.rect[2]).fold(f32::MIN, f32::max) - frame[0];
                        (row[1] < left - 14.0 * plan.scale || row[2] > right + 14.0 * plan.scale)
                            .then(|| format!("{:.0}..{:.0} for {left:.0}..{right:.0}", row[1], row[2]))
                    })
                    .collect()
            })
            .unwrap_or_default();
        report.insert(
            name.to_owned(),
            json!({
                "cut": cut.into_iter().take(6).collect::<Vec<_>>(),
                "rack": rt.presented("bottle-rack").is_some(),
                "rows": tops.len(),
                "pipes": pipes.len(),
                "wrapped": wrapped,
                "kindClash": kind_clash,
                "kindCentered": kind_centered,
                "grazes": grazes,
                "overhangingPlanks": planks,
                "kindGap": kind_gap,
            }),
        );
    }
    Ok(json!(report))
}
fn true_records(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut checked = runtime_for(tasting_spell());
    click(&mut checked, "run-spell")?;
    live_finish(&mut checked)?;
    click(&mut checked, "validate")?;
    let check_keeps_brew = json!({ "brew": checked.state.canvas.last_brew.map(|status| format!("{status:?}")), "stale": checked.state.trace_stale });

    let mut literal = SpellGraph::new("Literal");
    literal.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("ask", "ai/generate_text", 280.0, json!({ "prompt": "Show the literal example {{customer}} exactly as written" })),
        studio_node("say", "io/send_message", 560.0, serde_json::Value::Null),
    ];
    literal.wires = vec![
        studio_wire("w1", ("trigger", "out"), ("ask", "in")),
        studio_wire("w2", ("ask", "out"), ("say", "in")),
        studio_wire("w3", ("ask", "text"), ("say", "content")),
    ];
    let mut braces = runtime_for(literal);
    click(&mut braces, "validate")?;
    braces.update(|app, _| app.canvas.select(Some(NodeId::from("ask"))));
    braces.frame(0.0);
    let literal_braces = json!({ "check": braces.state.validation.label(), "noted": braces.find("prompt-literal-note").is_some() });

    let mut relay = SpellGraph::new("Two Replies");
    relay.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("first", "io/send_message", 280.0, serde_json::Value::Null),
        studio_node("second", "io/send_message", 560.0, serde_json::Value::Null),
    ];
    relay.wires = vec![
        studio_wire("w1", ("trigger", "out"), ("first", "in")),
        studio_wire("w2", ("first", "out"), ("second", "in")),
        studio_wire("w3", ("trigger", "content"), ("first", "content")),
        studio_wire("w4", ("trigger", "content"), ("second", "content")),
    ];
    let relay_rt = runtime_for(relay);
    let relay_plan = magicklala_canvas::brew_plan(&relay_rt.state.canvas, relay_rt.state.canvas_size());
    let mut replies: Vec<String> = relay_plan.with_role(magicklala_canvas::BrewRole::Bottle).map(|card| card.id.0.clone()).collect();
    replies.sort();

    let mut router = runtime_for(example("parallel_research.json")?);
    click(&mut router, "run-spell")?;
    live_finish(&mut router)?;
    router.frame(1.0);
    let ids: Vec<String> = router.state.canvas.nodes.iter().map(|card| card.id.0.clone()).collect();
    let mut numbers = Vec::new();
    let mut untaken_numbered = Vec::new();
    for id in ids {
        let shown = texts_under(&mut router, &format!("step-badge-{id}"));
        let shown = shown.trim().to_owned();
        if shown.is_empty() {
            continue;
        }
        let ran = router
            .state
            .canvas
            .card(&NodeId::from(id.as_str()))
            .is_some_and(|card| matches!(card.status, ExecutionStatus::Succeeded | ExecutionStatus::Failed));
        if !ran {
            untaken_numbered.push(id.clone());
        }
        numbers.push(shown);
    }
    let mut unique = numbers.clone();
    unique.sort();
    unique.dedup();
    let step_numbers = json!({ "duplicates": numbers.len() - unique.len(), "untakenNumbered": untaken_numbered });

    let mut tossed = runtime_for(tasting_spell());
    tossed.update(|app, _| app.add_variable());
    tossed.update(|app, _| {
        app.spawn("flow/do_once");
        app.sync_edits();
    });
    let added = tossed.state.canvas.nodes.iter().any(|card| card.kind == "flow/do_once");
    tossed.update(|app, _| app.undo());
    let toss_undo = json!({
        "added": added,
        "removedByUndo": !tossed.state.canvas.nodes.iter().any(|card| card.kind == "flow/do_once"),
        "variables": tossed.state.spell.variables.len(),
    });

    let mut draft = studio_at(db_path)?;
    draft.update(|app, _| {
        app.new_empty_spell();
        app.add_variable();
    });
    let saved = draft.state.save_draft().is_ok();
    draft.update(|app, _| {
        app.add_variable();
        app.request_open_sample("Research Agent");
    });
    let empty_draft = json!({ "saved": saved, "asked": draft.state.pending_switch.is_some(), "kept": draft.state.spell.variables.len() });

    let mut moved = runtime_for(tasting_spell());
    moved.update(|app, _| {
        app.wire_error = Some("Wire refused: a flow socket and a value socket cannot meet".to_owned());
        app.open_sample_spell("Research Agent");
    });
    let wire_message_left = moved.state.wire_error.is_some();

    Ok(json!({
        "checkKeepsBrew": check_keeps_brew,
        "literalBraces": literal_braces,
        "replies": replies,
        "stepNumbers": step_numbers,
        "tossUndo": toss_undo,
        "emptyDraft": empty_draft,
        "wireMessageLeft": wire_message_left,
    }))
}
fn tidy_bottles() -> Result<serde_json::Value, String> {
    let cases = [
        ("kitchen@1280", crowded_kitchen(), (1280.0_f32, 800.0_f32)),
        ("kitchen@960", crowded_kitchen(), (960.0, 640.0)),
        ("router@960", example("webhook_router.json")?, (960.0, 640.0)),
        ("dozen@960", dozen_ingredient_spell(), (960.0, 640.0)),
        ("three@960", many_bottle_spell(3), (960.0, 640.0)),
    ];
    let mut measure = studio_text_system();
    let mut report = serde_json::Map::new();
    for (name, spell, size) in cases {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        click(&mut rt, "run-spell")?;
        live_finish(&mut rt)?;
        rt.frame(3.0);
        let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
        let mut clashes = Vec::new();
        let mut cut_kinds = Vec::new();
        for card in plan.with_role(magicklala_canvas::BrewRole::Bottle) {
            let id = &card.id.0;
            let lines = text_lines_under(&rt, &format!("node-{id}"));
            let badge = layout_rect(&rt, &format!("step-badge-{id}"));
            let words: Vec<&(String, [f32; 4])> = lines.iter().filter(|(line, _)| line.parse::<usize>().is_err() && !line.ends_with("ms")).collect();
            if let Some(badge) = badge {
                for (line, rect) in &words {
                    if rects_overlap(badge, *rect) {
                        clashes.push(format!("{id}: step number covers {line}"));
                    }
                }
            }
            for (index, (line, rect)) in words.iter().enumerate() {
                for (other, other_rect) in words.iter().skip(index + 1) {
                    if rects_overlap(*rect, *other_rect) {
                        clashes.push(format!("{id}: {line} overlaps {other}"));
                    }
                }
            }
            if let Some(kind) = rt.state.canvas.card(&card.id).map(|bottle| bottle.kind.clone())
                && let Some((_, rect)) = lines.iter().find(|(line, _)| line == &kind)
            {
                let (natural, _) = measure.measure(&kind, rune::TextStyle::new(11.0_f32.max(10.5 / plan.scale), 500.0).mono(), 1.0);
                if natural * plan.scale > rect[2] + 2.0 {
                    cut_kinds.push(format!("{id}: {kind}"));
                }
            }
        }
        clashes.dedup();
        report.insert(
            name.to_owned(),
            json!({ "clashes": clashes.into_iter().take(6).collect::<Vec<_>>(), "cutKinds": cut_kinds.into_iter().take(4).collect::<Vec<_>>() }),
        );
    }
    Ok(json!(report))
}
fn starter_shelf_room() -> Result<serde_json::Value, String> {
    let scene = |size: (f32, f32), pantry: Option<bool>| -> Result<serde_json::Value, String> {
        let mut app = StudioApp::with_spell(SpellGraph::new("Fresh"), registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        match pantry {
            Some(true) if rt.state.left_rail_collapsed() => click(&mut rt, "expand-nodes")?,
            Some(false) => rt.update(|app, _| app.left_tab = LeftTab::Agents),
            _ => {}
        }
        rt.frame(1.0);
        let starters: Vec<[f32; 4]> = rt
            .state
            .specs
            .iter()
            .filter(|spec| spec.is_trigger)
            .filter_map(|spec| layout_rect(&rt, &format!("starter-{}", spec.kind.replace('/', "-"))))
            .collect();
        let pot = pot_body(&rt);
        let pantry_open = rt.state.left_tab == LeftTab::Palette && !rt.state.left_rail_collapsed();
        Ok(json!({
            "pantryOpen": pantry_open,
            "starters": starters.len(),
            "coverPot": pot.is_some_and(|pot| starters.iter().any(|starter| rects_overlap(*starter, pot))),
        }))
    };
    Ok(json!({
        "wideWithPantry": scene((1280.0, 800.0), Some(true))?,
        "wideWithPotions": scene((1280.0, 800.0), Some(false))?,
        "compactClosed": scene((960.0, 640.0), None)?,
        "compactWithPantry": scene((960.0, 640.0), Some(true))?,
    }))
}
fn wire_records(db_path: &Path) -> Result<serde_json::Value, String> {
    let ids = |app: &StudioApp| app.spell.wires.iter().map(|wire| wire.id.0.clone()).collect::<Vec<_>>();
    let mut rt = studio_at(db_path)?;
    let original = ids(&rt.state);
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("recall"))));
    let same_session = ids(&rt.state) == original;
    rt.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    let saved = rt.state.save_draft().is_ok();
    drop(rt);
    let mut reopened = studio_at(db_path)?;
    reopened.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("recall"))));
    let after = ids(&reopened.state);
    Ok(json!({
        "sameSession": same_session,
        "saved": saved,
        "afterReopen": after == original,
        "missing": original.iter().filter(|id| !after.contains(id)).cloned().collect::<Vec<_>>(),
    }))
}
fn grimoire_room() -> Result<serde_json::Value, String> {
    let mut measure = studio_text_system();
    let mut report = serde_json::Map::new();
    for (name, spell) in [("support", StudioApp::from_registry().spell.clone()), ("research", example("parallel_research.json")?)] {
        let mut rt = runtime_for(spell);
        click(&mut rt, "mode-grimoire")?;
        click(&mut rt, "zoom-fit")?;
        rt.frame(1.0);
        let card_at = |id: &NodeId| rt.state.canvas.card(id).map(|card| (card.position, card.size));
        let tight: Vec<String> = rt
            .state
            .canvas
            .wires
            .iter()
            .filter_map(|wire| {
                let ((from, from_size), (to, _)) = (card_at(&wire.from_node)?, card_at(&wire.to_node)?);
                (to[0] - (from[0] + from_size[0]) < 30.0).then(|| format!("{}>{}", wire.from_node.0, wire.to_node.0))
            })
            .collect();
        let zoom = rt.state.canvas.camera.zoom;
        let mut twins_cut = Vec::new();
        for card in rt.state.canvas.nodes.clone() {
            let title = rt.state.canvas.display_title(&card.id);
            if !title.contains(" · ") {
                continue;
            }
            let lines = text_lines_under(&rt, &format!("node-{}-header", card.id.0));
            let shown = lines.iter().any(|(line, rect)| {
                let clean = line.replace('\u{a0}', " ");
                let (natural, _) = measure.measure(&clean, rune::TextStyle::new((13.6 * zoom).clamp(11.8, 15.5), 750.0), 1.0);
                clean.trim_start().starts_with('·') && natural <= rect[2] + 2.0 || clean == title && natural <= rect[2] + 2.0
            });
            if !shown {
                twins_cut.push(title);
            }
        }
        report.insert(name.to_owned(), json!({ "tight": tight, "twinsCut": twins_cut }));
    }
    Ok(json!(report))
}
fn log_counts() -> Result<serde_json::Value, String> {
    let mut rt = runtime_for(many_bottle_spell(8));
    click(&mut rt, "run-spell")?;
    live_finish(&mut rt)?;
    rt.frame(1.0);
    let strip = texts_under(&mut rt, "trace-strip");
    let distinct = {
        let mut ran: Vec<String> =
            rt.state.trace.iter().filter(|line| line.kind == "NodeFinished").filter_map(|line| line.node.as_ref().map(|node| node.0.clone())).collect();
        ran.sort();
        ran.dedup();
        ran.len()
    };
    Ok(json!({ "distinct": distinct, "saysSteps": strip.contains(&format!("{distinct} steps")) }))
}
fn kept_promises(db_path: &Path) -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32, config: serde_json::Value| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let fed = |app: &StudioApp, node: &str, socket: &str| app.canvas.wires.iter().any(|wire| wire.to_node.0 == node && wire.to_socket == socket);

    let mut prompted = SpellGraph::new("Typed Agent");
    prompted.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("tmpl", "text/template", 1.0, json!({ "template": "Q: {{content}}" })),
        place("gen", "ai/generate_text", 2.0, serde_json::Value::Null),
        place("reply", "io/send_message", 3.0, serde_json::Value::Null),
    ];
    prompted.wires = vec![
        wire("p1", ("trigger", "out"), ("gen", "in")),
        wire("p2", ("gen", "out"), ("reply", "in")),
        wire("p3", ("trigger", "content"), ("tmpl", "content")),
        wire("p4", ("tmpl", "text"), ("gen", "prompt")),
        wire("p5", ("gen", "text"), ("reply", "content")),
    ];
    let mut typing = studio_at(db_path)?;
    typing.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(prompted.clone());
    });
    typing.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("tmpl"))));
    let bridged = fed(&typing.state, "gen", "prompt");
    typing.update(|app, _| {
        app.canvas.select(Some(NodeId::from("gen")));
        app.right_tab = RightTab::Inspector;
    });
    typing.frame(1.0);
    click(&mut typing, "prompt-unplug")?;
    typing.update(|app, _| {
        app.prompts.insert(NodeId::from("gen"), rune::TextEdit::new("typed words"));
        app.sync_spell();
    });
    click(&mut typing, "save")?;
    let reopened = studio_at(db_path)?;
    let typed = json!({
        "bridged": bridged,
        "wired": fed(&reopened.state, "gen", "prompt"),
        "prompt": prompt_of(&reopened.state, &NodeId::from("gen")),
    });

    let callee_db = db_path.with_extension("callee.sqlite");
    let mut callee = SpellGraph::new("Callee Spell");
    callee.nodes = vec![
        studio_node("start", "event/on_start", 0.0, serde_json::Value::Null),
        studio_node("send", "io/send_message", 280.0, json!({ "content": "from callee" })),
    ];
    callee.wires = vec![studio_wire("cw", ("start", "out"), ("send", "in"))];
    StudioStore::open(Some(&callee_db)).and_then(|mut store| store.publish(&callee)).map_err(|error| error.to_string())?;
    let mut caller = SpellGraph::new("Caller Spell");
    caller.nodes = vec![
        studio_node("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        studio_node("call", "flow/subgraph", 280.0, json!({ "spell_id": callee.id.0.to_string(), "release_version": 1 })),
    ];
    caller.wires = vec![studio_wire("cc", ("trigger", "out"), ("call", "in"))];
    caller.subgraphs = vec![magicklala_graph::SubgraphRef { node_id: NodeId::from("call"), spell_id: callee.id, release_version: Some(1) }];
    let mut calling = studio_at(&callee_db)?;
    calling.update(move |app, _| {
        app.new_empty_spell();
        app.load_graph(caller.clone());
    });
    calling.update(|app, _| {
        let _ = app.publish();
        let _ = app.publish();
    });
    let caller_releases = release_versions(&calling.state);
    click(&mut calling, "save")?;
    let calling_again = studio_at(&callee_db)?;
    let sub_spell = json!({ "releases": caller_releases, "liveAfterReopen": calling_again.state.active_release_version });

    let mut routes = SpellGraph::new("Two Door Agent");
    routes.nodes = vec![
        place("chat", "event/on_message", 0.0, serde_json::Value::Null),
        place("said", "io/send_message", 1.0, serde_json::Value::Null),
        place("hook", "event/on_webhook", 0.0, serde_json::Value::Null),
        place("hooked", "io/send_message", 1.0, json!({ "content": "hooked" })),
    ];
    routes.wires =
        vec![wire("r1", ("chat", "out"), ("said", "in")), wire("r2", ("chat", "content"), ("said", "content")), wire("r3", ("hook", "out"), ("hooked", "in"))];
    let mut doors = runtime_for(routes);
    let door_run = run_once(&mut doors)?;
    let two_doors = json!({ "replies": door_run["replies"], "sent": doors.state.run_payload["content"] });

    let mut numbers = studio_at(db_path)?;
    numbers.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(tasting_spell());
        app.add_variable();
    });
    let slot = numbers.state.spell.variables.len() - 1;
    let set_default = move |rt: &mut Runtime<StudioApp>, text: &'static str| {
        rt.update(move |app, _| {
            if let Some(edit) = app.var_default_edits.get_mut(slot) {
                *edit = rune::TextEdit::new(text);
            }
            app.sync_spell();
        });
    };
    numbers.update(move |app, _| app.set_variable_type_at(slot, SocketType::Int));
    set_default(&mut numbers, "5");
    click(&mut numbers, "publish")?;
    let live_before = numbers.state.active_release_version;
    set_default(&mut numbers, "nope");
    let while_bad = json!({ "live": numbers.state.active_release_version, "dirty": numbers.state.dirty, "badge": numbers.state.canvas.release_badge });
    click(&mut numbers, "validate")?;
    let hexed = numbers.state.validation.label();
    set_default(&mut numbers, "5");
    numbers.frame(1.0);
    let mended_number = json!({
        "liveBefore": live_before,
        "whileBad": while_bad,
        "hexed": hexed,
        "after": numbers.state.validation.label(),
        "brewLocked": numbers.state.canvas.invalid_brew,
    });
    numbers.update(|app, _| {
        app.add_variable();
        app.add_variable();
    });
    let names = numbers.state.spell.variables.len();
    numbers.update(move |app, _| {
        for idx in [names - 2, names - 1] {
            if let Some(edit) = app.var_name_edits.get_mut(idx) {
                *edit = rune::TextEdit::new("twin");
            }
        }
        app.sync_spell();
        app.canvas.select(None);
        app.right_tab = RightTab::Variables;
    });
    numbers.frame(1.0);
    click(&mut numbers, "validate")?;
    let duplicate_check = numbers.state.validation.label();
    let type_keys = (0..names).filter(|idx| numbers.find(&format!("var-type-{idx}")).is_some()).count();
    numbers.update(move |app, _| {
        if let Some(edit) = app.var_name_edits.get_mut(names - 1) {
            *edit = rune::TextEdit::new("");
        }
        app.sync_spell();
    });
    click(&mut numbers, "validate")?;
    let blank_check = numbers.state.validation.label();
    let names_report = json!({ "duplicate": duplicate_check, "blank": blank_check, "typeKeys": type_keys, "rows": names });

    let mut lonely = SpellGraph::new("Lonely Agent");
    lonely.nodes = vec![place("trigger", "event/on_message", 0.0, serde_json::Value::Null), place("reply", "io/send_message", 1.0, serde_json::Value::Null)];
    let mut alone = runtime_for(lonely);
    click(&mut alone, "validate")?;
    let lone = json!({ "trigger": role_of(&alone.state, "trigger"), "reply": role_of(&alone.state, "reply"), "check": alone.state.validation.label() });

    let mut restoring = studio_at(db_path)?;
    restoring.update(|app, _| {
        app.new_empty_spell();
        let mut restore = tasting_spell();
        restore.name = "Restore Agent".to_owned();
        app.load_graph(restore);
    });
    click(&mut restoring, "publish")?;
    restoring.update(|app, _| {
        app.request_restore_release(1);
        app.confirm_restore_to_draft();
    });
    let restored = json!({ "live": restoring.state.active_release_version, "badge": restoring.state.canvas.release_badge });

    let mut once = SpellGraph::new("Once Agent");
    once.nodes = vec![
        place("trigger", "event/on_message", 0.0, serde_json::Value::Null),
        place("once", "flow/do_once", 1.0, serde_json::Value::Null),
        place("reply", "io/send_message", 2.0, serde_json::Value::Null),
    ];
    once.wires = vec![
        wire("o1", ("trigger", "out"), ("once", "in")),
        wire("o2", ("once", "out"), ("reply", "in")),
        wire("o3", ("trigger", "content"), ("reply", "content")),
    ];
    let mut grim = runtime_for(once);
    grim.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("once"))));
    click(&mut grim, "mode-grimoire")?;
    grim.frame(1.0);
    let socket_drag = |rt: &mut Runtime<StudioApp>, from: &str, to: &str| -> Result<(), String> {
        let (start, end) = (rt.find(from).ok_or(format!("no {from}"))?, rt.find(to).ok_or(format!("no {to}"))?);
        drag(rt, from, (end.0 - start.0, end.1 - start.1))?;
        rt.update(|app, _| {
            app.sync_edits();
        });
        rt.frame(1.0);
        Ok(())
    };
    socket_drag(&mut grim, "socket-trigger-out-out", "socket-once-in-in")?;
    socket_drag(&mut grim, "socket-once-out-out", "socket-reply-in-in")?;
    let mut flow_into_reply: Vec<String> = feeders_of(&grim.state, "reply").into_iter().filter(|line| line.ends_with("->in")).collect();
    flow_into_reply.sort();
    let _ = socket_drag(&mut grim, "socket-trigger-out-out", "socket-reply-in-content");
    grim.frame(2.0);
    let refusal = all_texts(&grim).into_iter().find(|line| line.contains("cannot meet") || line.contains("does not fit"));
    let live_drag = json!({ "flowIntoReply": flow_into_reply, "refusalShown": refusal.is_some() });

    let _ = std::fs::remove_file(&callee_db);
    Ok(json!({
        "typed": typed,
        "subSpell": sub_spell,
        "twoDoors": two_doors,
        "number": mended_number,
        "names": names_report,
        "lone": lone,
        "restored": restored,
        "liveDrag": live_drag,
    }))
}
fn one_voice(db_path: &Path) -> Result<serde_json::Value, String> {
    let words = |rt: &Runtime<StudioApp>, key: &str| text_lines_under(rt, key).into_iter().map(|(line, _)| line).collect::<Vec<_>>().join(" ");
    let mut sealed = studio_at(db_path)?;
    sealed.update(|app, _| {
        app.new_empty_spell();
        app.load_graph(tasting_spell());
    });
    click(&mut sealed, "publish")?;
    sealed.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("echo"))));
    sealed.update(|app, _| app.toss(magicklala_canvas::Toss::IntoPot(NodeId::from("echo"))));
    sealed.frame(1.0);
    let bottle_key = magicklala_canvas::brew_plan(&sealed.state.canvas, sealed.state.canvas_size())
        .with_role(magicklala_canvas::BrewRole::Bottle)
        .next()
        .map(|card| format!("node-{}-header", card.id.0))
        .unwrap_or_default();
    let bottle_words = words(&sealed, &bottle_key);
    let round_trip = json!({
        "header": words(&sealed, "toolbar"),
        "plaque": words(&sealed, "cauldron-plaque"),
        "bottle": bottle_words,
    });
    let moved = sealed.state.canvas.nodes[0].id.clone();
    sealed.update(move |app, _| {
        if let Some(card) = app.canvas.card_mut(&moved) {
            card.position[0] += 120.0;
        }
        app.sync_spell();
    });
    click(&mut sealed, "save")?;
    let mut reopened = studio_at(db_path)?;
    let live_after_reopen = reopened.state.active_release_version;
    reopened.update(|app, _| {
        let _ = app.publish();
    });
    let releases_after_reopen = release_versions(&reopened.state);

    let cauldron = runtime_for(StudioApp::from_registry().spell.clone());
    let toolbar_counts = words(&cauldron, "toolbar-left").contains("ingredient");
    let pantry_counts = all_texts(&cauldron).iter().any(|line| line.contains("in cauldron"));
    let mut grimoire = runtime_for(StudioApp::from_registry().spell.clone());
    click(&mut grimoire, "mode-grimoire")?;
    grimoire.frame(1.0);
    let grimoire_counts = words(&grimoire, "toolbar-left").contains("ingredient");

    let mut peaks = Vec::new();
    for (name, spell) in [("router", example("webhook_router.json")?), ("support", StudioApp::from_registry().spell.clone())] {
        for size in [(1280.0_f32, 800.0_f32), (960.0, 640.0)] {
            let mut app = StudioApp::with_spell(spell.clone(), registry_specs());
            app.size = size;
            let rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
            let plan = magicklala_canvas::brew_plan(&rt.state.canvas, rt.state.canvas_size());
            for (index, ribbon) in magicklala_canvas::cauldron::ribbon_paths(&plan).iter().enumerate() {
                let mouth = ribbon.last().copied().unwrap_or_default();
                let tap = ribbon.first().copied().unwrap_or_default();
                let top = ribbon.iter().map(|point| point[1]).fold(f32::MAX, f32::min);
                let climb = mouth[1].min(tap[1]) - top;
                if climb > 24.0 * plan.scale {
                    peaks.push(format!("{name}@{}: ribbon {index} climbs {}px above its tap and mouth", size.0, climb.round()));
                }
            }
        }
    }

    Ok(json!({
        "roundTrip": round_trip,
        "liveAfterReopen": live_after_reopen,
        "releasesAfterReopen": releases_after_reopen,
        "counts": { "toolbar": toolbar_counts, "pantry": pantry_counts, "grimoire": grimoire_counts },
        "peaks": peaks,
    }))
}
fn brew_locks(db_path: &Path) -> Result<serde_json::Value, String> {
    let mut slow_spell = delay_spell(1_500);
    slow_spell.nodes.push(NodeInstance {
        id: NodeId::from("note"),
        kind: "text/template".to_owned(),
        position: [280.0, 200.0],
        config: json!({ "template": "said {{content}}" }),
    });
    slow_spell.wires.retain(|wire| wire.id.0 != "w3");
    slow_spell.wires.push(Wire {
        id: WireId::from("w4"),
        from_node: NodeId::from("trigger"),
        from_socket: "content".to_owned(),
        to_node: NodeId::from("note"),
        to_socket: "content".to_owned(),
    });
    slow_spell.wires.push(Wire {
        id: WireId::from("w5"),
        from_node: NodeId::from("note"),
        from_socket: "text".to_owned(),
        to_node: NodeId::from("reply"),
        to_socket: "content".to_owned(),
    });
    let mut app = StudioApp::with_spell(slow_spell, registry_specs());
    app.run_timeout = Duration::from_secs(600);
    let mut rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|app, _| app.canvas.select(Some(NodeId::from("note"))));
    click_at(&mut rt, "run-spell", 0.5)?;
    rt.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    let prompt_before = rt.state.prompts.get(&NodeId::from("note")).map(|edit| edit.text.clone());
    if click_at(&mut rt, "prompt-field", 1.0).is_ok() {
        rt.ime(rune::Ime::Commit("zz".to_owned()));
        rt.frame(1.2);
    }
    let prompt_after = rt.state.prompts.get(&NodeId::from("note")).map(|edit| edit.text.clone());
    click_at(&mut rt, "save", 1.4)?;
    let save_error = rt.state.store_error.clone();
    rt.update(|app, _| {
        app.canvas.select(None);
        app.canvas.context_menu = Some([600.0, 300.0]);
    });
    let nodes_before = rt.state.canvas.nodes.len();
    let _ = click_at(&mut rt, "context-add-ai-generate_text", 1.6);
    let added = rt.state.canvas.nodes.len() - nodes_before;
    click_at(&mut rt, "mode-grimoire", 1.8)?;
    rt.frame(2.0);
    let position_before = rt.state.canvas.card(&NodeId::from("note")).map(|card| card.position);
    let _ = drag(&mut rt, "node-note-header", (90.0, 40.0));
    let position_after = rt.state.canvas.card(&NodeId::from("note")).map(|card| card.position);
    let wires_before = rt.state.canvas.wires.len();
    if let (Some(from), Some(to)) = (rt.find("socket-trigger-out-out"), rt.find("socket-note-in-content")) {
        let _ = drag(&mut rt, "socket-trigger-out-out", (to.0 - from.0, to.1 - from.1));
    }
    let wires_after = rt.state.canvas.wires.len();
    live_finish(&mut rt)?;
    rt.frame(3.0);
    let lit_after = rt.state.canvas.has_run();

    let mut hook = runtime_for(example("webhook_router.json")?);
    hook.frame(1.0);
    let idle_badges: Vec<String> = ["order_reply", "support_reply", "ignored_reply"]
        .iter()
        .flat_map(|id| text_lines_under(&hook, &format!("node-{id}")).into_iter().map(|(line, _)| line))
        .filter(|line| line.chars().all(|c| c.is_ascii_digit()))
        .collect();
    hook.update(|app, _| {
        if let Some(card) = app.canvas.nodes.iter_mut().find(|card| card.id.0 == "order_reply") {
            card.status = ExecutionStatus::Running;
        }
        app.canvas.running_brew = true;
    });
    hook.frame(2.0);
    let running_lines: Vec<String> = text_lines_under(&hook, "node-order_reply").into_iter().map(|(line, _)| line).collect();

    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.new_empty_spell();
    app.load_graph(example("webhook_router.json")?);
    let mut sealed = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    sealed.frame(0.0);
    click(&mut sealed, "publish")?;
    sealed.frame(2.0);
    let sealed_lines: Vec<String> = text_lines_under(&sealed, "node-order_reply").into_iter().map(|(line, _)| line).collect();

    let mut logs = Vec::new();
    for (pauses, size) in [(11, (960.0_f32, 640.0_f32)), (11, (1280.0, 800.0)), (36, (960.0, 640.0))] {
        logs.push(brewed_log(delay_chain(pauses), size)?);
    }
    logs.push(brewed_log(dozen_ingredient_spell(), (960.0, 640.0))?);

    Ok(json!({
        "midBrew": {
            "promptUnchanged": prompt_before == prompt_after,
            "saveError": save_error,
            "added": added,
            "moved": position_before != position_after,
            "wiresAdded": wires_after as i64 - wires_before as i64,
            "litAfterBrew": lit_after,
        },
        "idleBadges": idle_badges,
        "runningBottle": running_lines,
        "sealedBottle": sealed_lines,
        "logs": logs,
    }))
}

fn brewed_log(spell: SpellGraph, size: (f32, f32)) -> Result<serde_json::Value, String> {
    let mut app = StudioApp::with_spell(spell, registry_specs());
    app.size = size;
    let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "run-spell")?;
    live_finish(&mut rt)?;
    rt.frame(3.0);
    let strip = layout_rect(&rt, "trace-strip").ok_or("no log")?;
    let lines = text_lines_under(&rt, "trace-strip");
    let result_visible = lines.iter().any(|(line, rect)| {
        (line == "Brewed" || line.starts_with("Failed") || line.starts_with("Hexed")) && inside(strip, *rect) && rect[0] + rect[2] <= size.0
    });
    let clipped: Vec<String> = lines.iter().filter(|(_, rect)| !inside(strip, *rect) || rect[0] + rect[2] > size.0).map(|(line, _)| line.clone()).collect();
    Ok(
        json!({ "width": size.0, "steps": rt.state.trace.iter().filter(|line| line.kind == "NodeFinished").count(), "resultVisible": result_visible, "clipped": clipped }),
    )
}

fn delay_chain(pauses: usize) -> SpellGraph {
    let mut chain = SpellGraph::new("Long Chain");
    chain.nodes.push(NodeInstance { id: NodeId::from("trigger"), kind: "event/on_message".to_owned(), position: [0.0, 0.0], config: serde_json::Value::Null });
    for step in 0..pauses {
        chain.nodes.push(NodeInstance {
            id: NodeId::from(format!("pause_{step}").as_str()),
            kind: "time/delay".to_owned(),
            position: [(step as f32 + 1.0) * 260.0, 0.0],
            config: json!({ "ms": 0 }),
        });
    }
    chain.nodes.push(NodeInstance { id: NodeId::from("reply"), kind: "io/send_message".to_owned(), position: [3120.0, 0.0], config: serde_json::Value::Null });
    let ids: Vec<String> = chain.nodes.iter().map(|node| node.id.0.clone()).collect();
    for (index, pair) in ids.windows(2).enumerate() {
        chain.wires.push(Wire {
            id: WireId::from(format!("flow_{index}").as_str()),
            from_node: NodeId::from(pair[0].as_str()),
            from_socket: "out".to_owned(),
            to_node: NodeId::from(pair[1].as_str()),
            to_socket: "in".to_owned(),
        });
    }
    chain.wires.push(Wire {
        id: WireId::from("words"),
        from_node: NodeId::from("trigger"),
        from_socket: "content".to_owned(),
        to_node: NodeId::from("reply"),
        to_socket: "content".to_owned(),
    });
    chain
}
fn geometry_and_polish(db_path: &Path) -> Result<serde_json::Value, String> {
    let sized = |spell: SpellGraph, size: (f32, f32)| {
        let mut app = StudioApp::with_spell(spell, registry_specs());
        app.size = size;
        let mut rt = Runtime::new(app, studio_view, size, Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0);
        rt
    };
    let dozen = sized(dozen_ingredient_spell(), (960.0, 640.0));
    let plan = magicklala_canvas::brew_plan(&dozen.state.canvas, dozen.state.canvas_size());
    let pours = magicklala_canvas::cauldron::pour_paths(&dozen.state.canvas, &plan);
    let mut landings: Vec<i64> = pours.iter().filter_map(|(_, points)| points.last().map(|point| (point[0] * 2.0).round() as i64)).collect();
    landings.sort_unstable();
    landings.dedup();
    let ribbons = magicklala_canvas::cauldron::ribbon_paths(&plan);
    let dozen_crossings = pours.iter().filter(|(_, points)| ribbons.iter().any(|ribbon| polylines_cross(points, ribbon))).count();
    let pour_overlaps: usize =
        pours.iter().enumerate().map(|(index, (_, a))| pours.iter().skip(index + 1).filter(|(_, b)| polylines_cross(a, b)).count()).sum();

    let research = sized(example("parallel_research.json")?, (1280.0, 800.0));
    let research_plan = magicklala_canvas::brew_plan(&research.state.canvas, research.state.canvas_size());
    let pot = research_plan.pot;
    let rim_y = pot[1] + pot[3] * 0.44;
    let through_belly: Vec<String> = magicklala_canvas::cauldron::pour_paths(&research.state.canvas, &research_plan)
        .into_iter()
        .filter(|(_, points)| {
            points
                .iter()
                .take(points.len().saturating_sub(4))
                .any(|point| point[0] > pot[0] && point[0] < pot[0] + pot[2] && point[1] > rim_y + 4.0 && point[1] < pot[1] + pot[3])
        })
        .map(|(id, _)| id.0)
        .collect();

    let hook = sized(example("webhook_router.json")?, (1280.0, 800.0));
    let hook_plan = magicklala_canvas::brew_plan(&hook.state.canvas, hook.state.canvas_size());
    let hpot = hook_plan.pot;
    let ribbon_through_pot = magicklala_canvas::cauldron::ribbon_paths(&hook_plan)
        .iter()
        .flat_map(|ribbon| ribbon.iter().skip(5).copied().collect::<Vec<_>>())
        .filter(|point| {
            let u = (hpot[2].min(hpot[3]) / 240.0).clamp(0.55, 1.35);
            let (cx, cy) = (hpot[0] + hpot[2] * 0.5, hpot[1] + hpot[3] * 0.56);
            (point[0] - cx).abs() < 86.0 * u && point[1] > cy - 30.0 * u && point[1] < cy + 56.0 * u
        })
        .count();

    let light = magicklala_canvas::HearthPalette::for_theme(true);
    let dark = magicklala_canvas::HearthPalette::for_theme(false);
    let running_contrast = [contrast(light.running_mark(), light.card), contrast(dark.running_mark(), dark.card)];
    let sun_contrast = [contrast(light.sun_ink(), light.wash_2), contrast(dark.sun_ink(), dark.wash_2)];
    let blend = |top: rune::Color, alpha: f32, under: rune::Color| rune::Color {
        r: top.r * alpha + under.r * (1.0 - alpha),
        g: top.g * alpha + under.g * (1.0 - alpha),
        b: top.b * alpha + under.b * (1.0 - alpha),
        a: 1.0,
    };
    let marker_contrast = [
        contrast(light.socket_flow_mark(), light.card),
        contrast(magicklala_canvas::hearth::stream_ink("flow", true), light.card),
        contrast(magicklala_canvas::hearth::category_tone("flow", true).liquid, light.card),
    ];
    let summary_contrast = contrast(dark.summary_ink(), blend(dark.broth_mid, 0.22, dark.shelf));

    let mut rail = sized(StudioApp::from_registry().spell.clone(), (960.0, 640.0));
    click(&mut rail, "expand-nodes")?;
    rail.frame(2.0);
    let tab_a = layout_rect(&rail, "tab-palette").ok_or("no pantry tab")?;
    let tab_b = layout_rect(&rail, "tab-agents").ok_or("no potions tab")?;
    let tab_gap = (tab_b[0] - (tab_a[0] + tab_a[2])).round();
    let tab_words_fit = text_lines_under(&rail, "tab-palette").iter().all(|(_, rect)| inside(tab_a, *rect))
        && text_lines_under(&rail, "tab-agents").iter().all(|(_, rect)| inside(tab_b, *rect));

    let compact = sized(StudioApp::from_registry().spell.clone(), (960.0, 640.0));
    let badge_overlaps: Vec<String> = compact
        .state
        .canvas
        .nodes
        .iter()
        .filter_map(|card| {
            let badge = layout_rect(&compact, &format!("step-badge-{}", card.id.0))?;
            let hit = text_lines_under(&compact, &format!("node-{}", card.id.0))
                .iter()
                .any(|(line, rect)| line != &card.kind && !line.chars().all(|c| c.is_ascii_digit()) && rects_touch(badge, *rect));
            hit.then(|| card.title.clone())
        })
        .collect();
    let badges_found = compact.state.canvas.nodes.iter().filter(|card| layout_rect(&compact, &format!("step-badge-{}", card.id.0)).is_some()).count();

    let place = |id: &str, kind: &str, x: f32| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config: serde_json::Value::Null,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let mut twins = SpellGraph::new("Twin Pauses");
    twins.nodes = vec![
        place("trigger", "event/on_message", 0.0),
        place("first", "time/delay", 1.0),
        place("second", "time/delay", 2.0),
        place("reply", "io/send_message", 3.0),
        place("again", "event/on_message", 0.0),
    ];
    twins.wires = vec![
        wire("w1", ("trigger", "out"), ("first", "in")),
        wire("w2", ("first", "out"), ("second", "in")),
        wire("w3", ("second", "out"), ("reply", "in")),
        wire("w4", ("trigger", "content"), ("reply", "content")),
    ];
    let mut twin_rt = sized(twins, (1280.0, 800.0));
    let vial_titles: Vec<String> = ["first", "second"]
        .iter()
        .filter_map(|id| {
            text_lines_under(&twin_rt, &format!("node-{id}")).into_iter().map(|(line, _)| line.replace('\u{a0}', " ")).find(|line| line.starts_with("Delay"))
        })
        .collect();
    let trigger_names = [twin_rt.state.canvas.display_title(&NodeId::from("trigger")), twin_rt.state.canvas.display_title(&NodeId::from("again"))];
    twin_rt.update(|app, _| {
        app.canvas.select(Some(NodeId::from("second")));
        app.canvas.context_menu = Some([400.0, 300.0]);
    });
    twin_rt.frame(2.0);
    let menu_title = text_lines_under(&twin_rt, "context-menu").into_iter().map(|(line, _)| line).next();

    let mut aside = sized(StudioApp::from_registry().spell.clone(), (1280.0, 800.0));
    aside.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    aside.frame(2.0);
    let pantry_marked = layout_rect(&aside, "pantry-in-memory-search").is_some();
    let pantry_kept = layout_rect(&aside, "pantry-in-ai-generate_text").is_some();
    click(&mut aside, "tab-agents")?;
    aside.frame(3.0);
    let potion_row = texts_under(&mut aside, "spell-support-agent");

    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.new_empty_spell();
    app.load_graph(example("webhook_router.json")?);
    let mut released = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    released.frame(0.0);
    click(&mut released, "publish")?;
    click(&mut released, "tab-agents")?;
    released.frame(2.0);
    let release_rows = texts_under(&mut released, "left-rail");

    let mut lone = SpellGraph::new("Lone");
    lone.nodes = vec![place("trigger", "event/on_message", 0.0)];
    let mut lone_rt = sized(lone, (1280.0, 800.0));
    click(&mut lone_rt, "run-spell")?;
    live_finish(&mut lone_rt)?;
    lone_rt.frame(3.0);
    let lone_chip = texts_under(&mut lone_rt, "validation-pill");

    let mut grim = sized(example("parallel_research.json")?, (1280.0, 800.0));
    click(&mut grim, "mode-grimoire")?;
    grim.update(|app, _| app.canvas.camera.zoom = 0.5);
    grim.frame(3.0);
    let mut grim_issues = Vec::new();
    for card in grim.state.canvas.nodes.clone() {
        let Some(header) = layout_rect(&grim, &format!("node-{}-header", card.id.0)) else { continue };
        let lines = text_lines_under(&grim, &format!("node-{}", card.id.0));
        if lines.iter().any(|(line, rect)| (line == &card.title || line.starts_with(&card.title)) && !inside(header, *rect)) {
            grim_issues.push(format!("{} title cut", card.id.0));
        }
        let labels: Vec<[f32; 4]> =
            lines.iter().filter(|(line, _)| card.inputs.iter().chain(card.outputs.iter()).any(|socket| &socket.name == line)).map(|(_, rect)| *rect).collect();
        if labels.iter().enumerate().any(|(index, a)| labels.iter().skip(index + 1).any(|b| rects_touch(*a, *b))) {
            grim_issues.push(format!("{} labels collide", card.id.0));
        }
    }
    grim.update(|app, _| app.canvas.camera.zoom = 0.64);
    grim.frame(3.0);
    let mut grim_bare = Vec::new();
    for card in grim.state.canvas.nodes.clone() {
        let lines = text_lines_under(&grim, &format!("node-{}", card.id.0));
        let shown = card.inputs.iter().chain(card.outputs.iter()).filter(|socket| lines.iter().any(|(line, _)| line == &socket.name)).count();
        if shown < card.inputs.len() + card.outputs.len() {
            grim_bare.push(format!("{} shows {shown} of {} socket names", card.id.0, card.inputs.len() + card.outputs.len()));
        }
    }

    Ok(json!({
        "dozen": { "landings": landings.len(), "pours": pours.len(), "ribbonCrossings": dozen_crossings, "pourCrossings": pour_overlaps, "lanes": plan.lanes.is_some() },
        "throughBelly": through_belly,
        "ribbonThroughPot": ribbon_through_pot,
        "runningContrast": running_contrast,
        "sunContrast": sun_contrast,
        "markerContrast": marker_contrast,
        "summaryContrast": summary_contrast,
        "tabs": { "gap": tab_gap, "wordsFit": tab_words_fit },
        "badges": { "found": badges_found, "overlaps": badge_overlaps },
        "names": { "vials": vial_titles, "triggers": trigger_names, "menu": menu_title },
        "pantryMarksAside": pantry_marked,
        "pantryMarksInPot": pantry_kept,
        "potionRow": potion_row,
        "releaseRows": release_rows,
        "loneChip": lone_chip,
        "grimoireAtHalf": grim_issues,
        "grimoireNamesAt64": grim_bare,
    }))
}
fn pull_middle() -> Result<serde_json::Value, String> {
    let place = |id: &str, kind: &str, x: f32| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config: serde_json::Value::Null,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let spell = || {
        let mut spell = SpellGraph::new("Recall Agent");
        spell.nodes = vec![place("trigger", "event/on_message", 0.0), place("recall", "memory/search", 1.0), place("reply", "io/send_message", 2.0)];
        spell.wires = vec![
            wire("w1", ("trigger", "out"), ("recall", "in")),
            wire("w2", ("trigger", "content"), ("recall", "query")),
            wire("w3", ("recall", "out"), ("reply", "in")),
            wire("w4", ("trigger", "content"), ("reply", "content")),
        ];
        spell
    };
    let brew = |rt: &mut Runtime<StudioApp>| -> Result<serde_json::Value, String> {
        click(rt, "validate")?;
        let check = rt.state.validation.label();
        click(rt, "run-spell")?;
        live_finish(rt)?;
        Ok(json!({
            "bottleFull": rt.state.canvas.card(&NodeId::from("reply")).is_some_and(|card| magicklala_canvas::cauldron::bottle_filled(&rt.state.canvas, card)),
            "check": check,
            "outcome": rt.state.last_run.as_ref().map(|report| report.outcome.label()),
            "reply": rt.state.canvas.card(&NodeId::from("reply")).map(|card| card.status.label()),
            "plaque": texts_under(rt, "cauldron-plaque"),
            "flowIntoReply": rt.state.spell.wires.iter().any(|wire| wire.to_node.0 == "reply" && wire.to_socket == "in"),
        }))
    };
    let mut aside = runtime_for(spell());
    aside.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    let set_aside = brew(&mut aside)?;
    let mut deleted = runtime_for(spell());
    deleted.update(|app, _| {
        app.canvas.select(Some(NodeId::from("recall")));
        app.delete_selected_node();
    });
    let removed = brew(&mut deleted)?;
    let mut hollow = runtime_for(spell());
    hollow.update(|app, _| app.toss(magicklala_canvas::Toss::Aside(NodeId::from("recall"))));
    hollow.update(|app, _| {
        app.canvas.wires.retain(|wire| wire.to_node.0 != "reply" || wire.to_socket != "in");
        app.sync_spell();
    });
    let empty = brew(&mut hollow)?;
    Ok(json!({ "setAside": set_aside, "deleted": removed, "emptyBottle": empty }))
}

fn corner_truths(db_path: &Path) -> Result<serde_json::Value, String> {
    let replace_report = |order: &[&str]| -> Result<serde_json::Value, String> {
        let mut rt = headless(StudioApp::with_spell(SpellGraph::new("Shout"), registry_specs()));
        for kind in order {
            let kind = (*kind).to_owned();
            rt.update(move |app, _| {
                app.spawn(&kind);
            });
        }
        let replace = node_of_kind(&rt.state, "text/replace").ok_or("no replace")?;
        let reply = node_of_kind(&rt.state, "io/send_message").ok_or("no reply")?;
        Ok(
            json!({ "role": role_of(&rt.state, &replace), "replaceFeeders": feeders_of(&rt.state, &replace), "replyFeeders": feeders_of(&rt.state, &reply), "replace": replace }),
        )
    };
    let tossed = replace_report(&["event/on_message", "text/replace", "io/send_message"])?;
    let spliced = replace_report(&["event/on_message", "io/send_message", "text/replace"])?;

    let place = |id: &str, kind: &str, x: f32| NodeInstance {
        id: NodeId::from(id),
        kind: kind.to_owned(),
        position: [x * 260.0, 0.0],
        config: serde_json::Value::Null,
    };
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    let mut twin = SpellGraph::new("Twin Triggers");
    twin.nodes = vec![place("spare", "event/on_message", 0.0), place("trigger", "event/on_message", 0.0), place("reply", "io/send_message", 1.0)];
    twin.wires = vec![wire("w1", ("trigger", "out"), ("reply", "in")), wire("w2", ("trigger", "content"), ("reply", "content"))];
    let mut twin_rt = runtime_for(twin);
    click(&mut twin_rt, "run-spell")?;
    live_finish(&mut twin_rt)?;
    let twin_reply = twin_rt.state.canvas.card(&NodeId::from("reply")).map(|card| card.status.label());

    let mut clock = Clock(0.0);
    let mut slow = slow_runtime(1_500, Duration::from_secs(600));
    click_at(&mut slow, "run-spell", clock.tick())?;
    slow.update(|app, _| {
        app.wait_until(SETTLE, delay_started);
    });
    click_at(&mut slow, "tab-trace", clock.tick())?;
    slow.frame(clock.tick());
    let trace_running = texts_under(&mut slow, "inspector-trace");
    let before = slow.state.spell.nodes.len();
    slow.update(|app, _| app.palette_query = rune::TextEdit::new("Template"));
    slow.frame(clock.tick());
    slow.frame(clock.tick());
    click_at(&mut slow, "palette-text-template", clock.tick())?;
    let added_mid_brew = slow.state.spell.nodes.len() - before;
    live_finish(&mut slow)?;
    slow.update(|app, _| {
        app.toss(magicklala_canvas::Toss::Aside(NodeId::from("wait")));
    });
    slow.frame(clock.tick());
    let trace_after_edit = texts_under(&mut slow, "inspector-trace");

    let mut prompt = headless(StudioApp::from_registry());
    prompt.update(|app, _| app.canvas.select(Some(NodeId::from("llm"))));
    prompt.frame(0.0);
    let prompt_field = texts_under(&mut prompt, "prompt-field");
    let unplug_shown = prompt.find("prompt-unplug").is_some();
    if unplug_shown {
        click(&mut prompt, "prompt-unplug")?;
    }
    let prompt_after = json!({
        "wired": prompt.state.spell.wires.iter().any(|wire| wire.to_node.0 == "llm" && wire.to_socket == "prompt"),
        "editor": prompt.find("prompt-editor").is_some(),
    });

    let mut long = StudioApp::from_registry();
    long.spell.name = "Customer Support Escalation Triage And Follow Up Potion".to_owned();
    long.canvas.spell_name = long.spell.name.clone();
    long.size = (960.0, 640.0);
    let mut long_rt = Runtime::new(long, studio_view, (960.0, 640.0), Env::new(true, 1.0));
    long_rt.frame(0.0);
    let clipped: Vec<String> = ["run-spell", "save", "publish", "theme-toggle", "zoom-fit"]
        .iter()
        .filter(|key| long_rt.presented(key).is_none_or(|(rect, _)| rect[0] < 0.0 || rect[0] + rect[2] > 960.5))
        .map(|key| (*key).to_owned())
        .collect();

    let config = StudioConfig { db_path: Some(db_path.to_path_buf()), ..StudioConfig::default() };
    let mut app = StudioApp::open(&config).map_err(|error| error.to_string())?;
    app.load_graph(tasting_spell());
    let mut store_rt = Runtime::new(app, studio_view, (1280.0, 800.0), Env::new(true, 1.0));
    store_rt.frame(0.0);
    click(&mut store_rt, "publish")?;
    click(&mut store_rt, "run-spell")?;
    live_finish(&mut store_rt)?;
    click(&mut store_rt, "publish")?;
    let releases = store_rt.state.releases.len();
    click(&mut store_rt, "save")?;
    store_rt.update(|app, _| {
        app.request_open_sample("Research Agent");
        app.confirm_switch();
    });
    click(&mut store_rt, "tab-agents")?;
    store_rt.frame(1.0);
    let listed = store_rt.find("spell-tasting-agent").is_some();
    if listed {
        click(&mut store_rt, "spell-tasting-agent")?;
        store_rt.update(|app, _| app.confirm_switch());
    }
    let reopened = store_rt.state.spell.name.clone();
    let glyphs: Vec<String> = visible_texts(&store_rt).into_iter().filter(|text| text.contains('\u{2713}')).collect();

    let mut brewed = runtime_for(tasting_spell());
    click(&mut brewed, "run-spell")?;
    live_finish(&mut brewed)?;
    let mut checks: Vec<String> = visible_texts(&brewed).into_iter().filter(|text| text.contains('\u{2713}')).collect();
    checks.extend(glyphs);

    Ok(json!({
        "tossed": tossed,
        "spliced": spliced,
        "twinReply": twin_reply,
        "traceRunning": trace_running,
        "addedMidBrew": added_mid_brew,
        "traceAfterEdit": trace_after_edit,
        "promptField": prompt_field,
        "unplugShown": unplug_shown,
        "promptAfter": prompt_after,
        "clipped": clipped,
        "releases": releases,
        "listed": listed,
        "reopened": reopened,
        "checkGlyphs": checks,
    }))
}

fn tray_drop() -> Result<serde_json::Value, String> {
    let mut rt = headless(StudioApp::from_registry());
    rt.update(|app, _| app.palette_query = rune::TextEdit::new("Generate Text"));
    rt.frame(1.0);
    rt.frame(3.0);
    let key = "palette-ai-generate_text";
    let row = rt.find(key).ok_or("no pantry row")?;
    let canvas = rt.presented("canvas").map(|(rect, _)| rect).ok_or("no canvas")?;
    let before = rt.state.spell.nodes.len();
    drag(&mut rt, key, (canvas[0] + 60.0 - row.0, canvas[1] + canvas[3] - 30.0 - row.1))?;
    let added = rt.state.spell.nodes.iter().skip(before).map(|node| node.id.0.clone()).next();
    Ok(json!({
        "added": rt.state.spell.nodes.len() - before,
        "role": added.as_ref().and_then(|id| role_of(&rt.state, id)),
        "wires": added.as_ref().map(|id| wires_of(&rt.state, id)),
    }))
}

fn dispatch(args: &[String]) -> Result<serde_json::Value, String> {
    match args.first().map(String::as_str) {
        Some("surfaces") => capture_surfaces(Path::new(args.get(1).ok_or("missing output directory")?)),
        Some("hex-pinpoint") => hex_pinpoint(),
        Some("pot-layout") => pot_layout(),
        Some("toss") => toss(),
        Some("plain-words") => plain_words(),
        Some("brew-truth") => brew_truth(),
        Some("switch-mid-brew") => switch_mid_brew(),
        Some("variable-defaults") => variable_defaults(),
        Some("publish-plaque") => publish_plaque(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("toss-safety") => toss_safety(),
        Some("shelf-seals") => shelf_seals(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("recipe-honesty") => recipe_honesty(),
        Some("potion-colours") => potion_colours(),
        Some("grimoire-place") => grimoire_place(),
        Some("pour-shape") => pour_shape(),
        Some("reads-cleanly") => cauldron_reads_cleanly(),
        Some("pull-middle") => pull_middle(),
        Some("one-name") => one_name_per_step(),
        Some("compact-fit") => compact_fit(),
        Some("recipe-mending") => recipe_mending(),
        Some("brew-locks") => brew_locks(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("geometry-and-polish") => geometry_and_polish(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("shelf-keeps") => shelf_keeps(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("shelf-exact") => shelf_exact(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("cauldron-truths") => cauldron_truths(),
        Some("plain-sight") => plain_sight(),
        Some("close-quarters") => close_quarters(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("true-runs") => true_runs(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("clear-signals") => clear_signals(),
        Some("pantry-settings") => pantry_settings(),
        Some("empty-recipe") => empty_recipe(),
        Some("list-truth") => list_truth(),
        Some("one-step-undo") => one_step_undo(),
        Some("wire-return") => wire_return(),
        Some("pantry-whole") => pantry_whole(),
        Some("empty-start") => empty_start(),
        Some("downhill") => downhill(),
        Some("failed-stub") => failed_stub(),
        Some("bound-settings") => bound_settings(),
        Some("twin-routes") => twin_routes(),
        Some("small-truths") => small_truths(),
        Some("card-notes") => card_notes(),
        Some("var-undo") => var_undo(),
        Some("aside-undo-pour") => aside_undo_pour(),
        Some("picker-undo") => picker_undo(),
        Some("split-cleared") => split_cleared(),
        Some("step-numbers") => step_numbers(),
        Some("redo-order") => redo_order(),
        Some("bound-looks") => bound_looks(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("check-save-truth") => check_save_truth(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("pot-rect") => pot_rect(),
        Some("gateway-hint") => gateway_hint(),
        Some("honest-dials") => honest_dials(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("workbench-truths") => workbench_truths(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("hands-on") => hands_on(),
        Some("crowded-bottles") => crowded_bottles(args),
        Some("agreed-edits") => agreed_edits(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("steady-hands") => steady_hands(),
        Some("fair-shelves") => fair_shelves(),
        Some("last-mile") => last_mile(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("legible-shelves") => legible_shelves(),
        Some("clear-sight") => clear_sight(),
        Some("mixed-shelves") => mixed_shelves(args),
        Some("undo-order") => undo_order(),
        Some("potion-rack") => potion_rack(),
        Some("true-records") => true_records(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("tidy-bottles") => tidy_bottles(),
        Some("starter-shelf") => starter_shelf_room(),
        Some("wire-records") => wire_records(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("grimoire-room") => grimoire_room(),
        Some("log-counts") => log_counts(),
        Some("honest-edits") => honest_edits(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("kept-promises") => kept_promises(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("one-voice") => one_voice(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("drawn-truths") => drawn_truths(),
        Some("honest-state") => honest_state(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("quiet-corners") => quiet_corners(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("shot") => shot(args),
        Some("fit-and-wording") => fit_and_wording(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("one-recipe") => one_recipe(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("tray-drop") => tray_drop(),
        Some("corner-truths") => corner_truths(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("cancel-run") => cancel_run(number(args.get(1), "delay ms")?, number(args.get(2), "deadline ms")?),
        Some("subgraph") => subgraph(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("subgraph-ref-edit") => subgraph_ref_edit(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("subgraph-collision") => subgraph_collision(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("draft-reopen") => draft_reopen(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("save-guards") => save_guards(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("release-identity") => release_identity(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("memory-db") => memory_db(&args[1..]),
        Some("visible-status") => visible_status(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("unopenable-db") => unopenable_db(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("project-scope") => project_scope(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("foreign-child") => foreign_child(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("edit-clears-store-error") => {
            let db = args.get(1).ok_or("missing database path")?;
            edit_clears_store_error(Path::new(db), Path::new(args.get(2).ok_or("missing unopenable database path")?))
        }
        Some("out-of-band-release") => out_of_band_release(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("publish-renamed") => {
            let db = args.get(1).ok_or("missing database path")?;
            publish_renamed(Path::new(db), args.get(2).ok_or("missing new spell name")?)
        }
        _ => match args {
            [desktop, compact, db_path] => probe(PathBuf::from(desktop), PathBuf::from(compact), PathBuf::from(db_path)),
            _ => Err("usage: studio_probe <desktop.png> <compact.png> <studio.sqlite> | cancel-run <delay-ms> <deadline-ms> | publish-renamed <studio.sqlite> <name> | subgraph <studio.sqlite> | subgraph-ref-edit <studio.sqlite> | subgraph-collision <studio.sqlite> | draft-reopen <studio.sqlite> | save-guards <studio.sqlite> | release-identity <studio.sqlite> | memory-db <db>... | visible-status <studio.sqlite> | unopenable-db <path> | project-scope <studio.sqlite> | out-of-band-release <studio.sqlite>".to_owned()),
        },
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
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
