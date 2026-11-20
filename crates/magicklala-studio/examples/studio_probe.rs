use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use magicklala_canvas::{ExecutionStatus, SocketRef, category_accent};
use magicklala_graph::{NodeCatalog, NodeId, NodeInstance, SocketType, SpellGraph, SpellId, SpellValidator, Wire, WireId};
use magicklala_studio::{
    PALETTE_CATEGORY_ORDER, PROMPT_EDITOR_PADDING, StudioApp, StudioConfig, StudioStore, Theme, ValidationState, palette_categories, prompt_font,
    registry_specs, render_headless_snapshot_themed, studio_view,
};
use rune::{Env, Runtime};
use serde_json::json;

const TOOLBAR_KEYS: [&str; 11] =
    ["toolbar-left", "validation-pill", "validate", "run-spell", "save", "publish", "theme-toggle", "zoom-out", "zoom-readout", "zoom-in", "zoom-fit"];
const CONTEXT_MENU_ITEMS: [&str; 4] = ["context-add-ai-generate_text", "context-delete-node", "context-fit-view", "context-dismiss"];

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
    let rt = sized_runtime(size, Theme::Dark);
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
    settle(&mut rt)?;
    let run = run_report(&rt.state);
    let succeeded = rt.state.canvas.nodes.iter().filter(|card| card.status == ExecutionStatus::Succeeded).count();
    let active_wires = rt.state.canvas.wires.iter().filter(|wire| wire.active).count();

    let theme_before = rt.state.theme.label();
    click(&mut rt, "theme-toggle")?;
    let theme_after = rt.state.theme.label();
    if rt.state.theme != Theme::Light || !rt.state.canvas.light {
        return Err("theme toggle did not reach the canvas".to_owned());
    }

    let preserved = vec![preserve("webhook_router.json")?, preserve("parallel_research.json")?];
    let edit = edit_script()?;
    let execution = execution()?;
    let chat = chat()?;
    let published = publish(&db_path)?;

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

fn number(raw: Option<&String>, name: &str) -> Result<u64, String> {
    raw.ok_or_else(|| format!("missing {name}"))?.parse().map_err(|error| format!("{name}: {error}"))
}

fn dispatch(args: &[String]) -> Result<serde_json::Value, String> {
    match args.first().map(String::as_str) {
        Some("cancel-run") => cancel_run(number(args.get(1), "delay ms")?, number(args.get(2), "deadline ms")?),
        Some("subgraph") => subgraph(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("subgraph-ref-edit") => subgraph_ref_edit(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("subgraph-collision") => subgraph_collision(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("draft-reopen") => draft_reopen(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("save-guards") => save_guards(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("release-identity") => release_identity(Path::new(args.get(1).ok_or("missing database path")?)),
        Some("publish-renamed") => {
            let db = args.get(1).ok_or("missing database path")?;
            publish_renamed(Path::new(db), args.get(2).ok_or("missing new spell name")?)
        }
        _ => match args {
            [desktop, compact, db_path] => probe(PathBuf::from(desktop), PathBuf::from(compact), PathBuf::from(db_path)),
            _ => Err("usage: studio_probe <desktop.png> <compact.png> <studio.sqlite> | cancel-run <delay-ms> <deadline-ms> | publish-renamed <studio.sqlite> <name> | subgraph <studio.sqlite> | subgraph-ref-edit <studio.sqlite> | subgraph-collision <studio.sqlite> | draft-reopen <studio.sqlite> | save-guards <studio.sqlite> | release-identity <studio.sqlite>".to_owned()),
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
