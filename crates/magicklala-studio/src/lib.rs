use std::path::Path;

use magicklala_canvas::{CanvasState, canvas_view};
use magicklala_graph::{NodeId, NodeInstance, NodeSpec, SocketSpec, SocketType, SpellGraph, Wire, WireId};
use rune::{App, AppSpec, Color, Cx, El, col, row, text};
use thiserror::Error;

const TITLE: &str = "MagickLaLa Studio";
const TOOLBAR_HEIGHT: f32 = 48.0;
const BACKGROUND: Color = Color { r: 0.045, g: 0.05, b: 0.06, a: 1.0 };
const TOOLBAR: Color = Color { r: 0.09, g: 0.095, b: 0.115, a: 1.0 };
const INK: Color = Color { r: 0.92, g: 0.93, b: 0.95, a: 1.0 };
const MUTED: Color = Color { r: 0.55, g: 0.58, b: 0.64, a: 1.0 };

#[derive(Debug, Error)]
pub enum StudioError {
    #[error("rune failed: {0}")]
    Rune(String),
    #[error("snapshot was not written to {0}")]
    MissingSnapshot(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioConfig {
    pub size: (f32, f32),
}

impl Default for StudioConfig {
    fn default() -> Self {
        Self { size: (1280.0, 800.0) }
    }
}

pub struct StudioApp {
    pub spell: SpellGraph,
    pub canvas: CanvasState,
    pub size: (f32, f32),
}

impl StudioApp {
    pub fn demo() -> Self {
        let spell = demo_spell();
        let canvas = CanvasState::from_graph(&spell, &demo_specs());
        Self { spell, canvas, size: StudioConfig::default().size }
    }
}

pub fn run_studio(config: StudioConfig) -> Result<(), StudioError> {
    App::new(spec(config)).args(Vec::new()).try_run().map_err(StudioError::Rune)
}

pub fn render_headless_snapshot(out_path: &Path) -> Result<(), StudioError> {
    let target = out_path.display().to_string();
    App::new(spec(StudioConfig::default())).args(vec!["--snap".to_owned(), target.clone()]).try_run().map_err(StudioError::Rune)?;
    if out_path.is_file() { Ok(()) } else { Err(StudioError::MissingSnapshot(target)) }
}

fn spec(config: StudioConfig) -> AppSpec<StudioApp> {
    AppSpec::new(TITLE, config.size, StudioApp::demo, view)
}

fn view(app: &StudioApp, _: &mut Cx<StudioApp>) -> El {
    let canvas_size = (app.size.0, app.size.1 - TOOLBAR_HEIGHT);
    let toolbar = row()
        .key("toolbar")
        .w_full()
        .h(TOOLBAR_HEIGHT)
        .padding(14.0)
        .gap(12.0)
        .fill(TOOLBAR)
        .child(text(TITLE).color(INK))
        .child(text(&app.spell.name).color(MUTED));
    col().key("studio").w_full().h_full().fill(BACKGROUND).child(toolbar).child(canvas_view(&app.canvas, canvas_size))
}

fn demo_specs() -> Vec<NodeSpec> {
    let node = |kind: &str, title: &str, inputs: Vec<SocketSpec>, outputs: Vec<SocketSpec>| NodeSpec {
        kind: kind.to_owned(),
        title: title.to_owned(),
        category: "core".to_owned(),
        description: String::new(),
        inputs,
        outputs,
        config: Vec::new(),
        is_trigger: false,
    };
    vec![
        node("core/on_message", "On Message", vec![], vec![SocketSpec::output("out", SocketType::Flow), SocketSpec::output("content", SocketType::String)]),
        node(
            "core/generate_text",
            "Generate Text",
            vec![SocketSpec::input("in", SocketType::Flow), SocketSpec::input("prompt", SocketType::String)],
            vec![SocketSpec::output("out", SocketType::Flow), SocketSpec::output("text", SocketType::String)],
        ),
        node("core/send_message", "Send Message", vec![SocketSpec::input("in", SocketType::Flow), SocketSpec::input("content", SocketType::String)], vec![]),
    ]
}

fn demo_spell() -> SpellGraph {
    let mut spell = SpellGraph::new("Hello Agent");
    let place =
        |id: &str, kind: &str, position: [f32; 2]| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position, config: serde_json::Value::Null };
    spell.nodes = vec![
        place("trigger", "core/on_message", [60.0, 120.0]),
        place("llm", "core/generate_text", [420.0, 200.0]),
        place("reply", "core/send_message", [780.0, 120.0]),
    ];
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    spell.wires = vec![
        wire("w1", ("trigger", "out"), ("llm", "in")),
        wire("w2", ("trigger", "content"), ("llm", "prompt")),
        wire("w3", ("llm", "out"), ("reply", "in")),
        wire("w4", ("llm", "text"), ("reply", "content")),
    ];
    spell
}
