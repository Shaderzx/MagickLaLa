use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use magicklala_canvas::{CanvasState, ExecutionStatus, category_accent, interactive_canvas_view, status_color};
use magicklala_graph::{
    EngineServices, EventEmitter, NodeCatalog, NodeId, NodeInstance, NodeSpec, RunStatus, RunSummary, RunTrigger, SpellEngine, SpellGraph, SpellValidator,
    TraceEvent, Value, VariableDef, Wire, WireId,
};
use magicklala_nodes::{HttpLlmGateway, InMemoryKnowledge, InMemoryMemory, LlmGatewayConfig, default_registry};
use magicklala_store::{NewProject, NewSpell, NewSpellRelease, Repository, SpellRecordId, SpellRelease, SqliteStore};
use rune::{App, AppSpec, Color, Cx, El, ScrollSpec, TextEdit, TextStyle, col, el, row, text, text_input};
use serde_json::json;
use thiserror::Error;

const TITLE: &str = "MagickLaLa Studio";
pub const TOOLBAR_HEIGHT: f32 = 48.0;
pub const LEFT_RAIL_WIDTH: f32 = 240.0;
pub const INSPECTOR_WIDTH: f32 = 300.0;
pub const TRACE_HEIGHT: f32 = 110.0;
pub const PALETTE_CATEGORY_ORDER: [&str; 13] =
    ["events", "flow", "ai", "memory", "knowledge", "io", "code", "variables", "text", "logic", "json", "network", "time"];
pub const LEGACY_KIND_PREFIX: &str = "core/";
pub const CONTEXT_MENU_SIZE: (f32, f32) = (210.0, 150.0);
pub const QUICK_ADD_KIND: &str = "ai/generate_text";
pub const COMPACT_TOOLBAR_BELOW: f32 = 1100.0;
pub const STUDIO_PROJECT: &str = "Studio";
pub const STUDIO_CHANNEL: &str = "studio";
pub const STUDIO_SENDER: &str = "studio-user";
pub const CHAT_TRIGGER_KIND: &str = "event/on_message";
const REPLY_OUTPUTS: [&str; 2] = ["reply", "content"];
const SENT_OUTPUT_SUFFIX: &str = ".sent";

static LAUNCH_SIZE: Mutex<Option<(f32, f32)>> = Mutex::new(None);
static LAUNCH_THEME: Mutex<Option<Theme>> = Mutex::new(None);
static LAUNCH_DB: Mutex<Option<PathBuf>> = Mutex::new(None);
static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum StudioError {
    #[error("rune failed: {0}")]
    Rune(String),
    #[error("snapshot was not written to {0}")]
    MissingSnapshot(String),
    #[error("studio store failed: {0}")]
    Store(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudioConfig {
    pub size: (f32, f32),
    pub db_path: Option<PathBuf>,
}

impl Default for StudioConfig {
    fn default() -> Self {
        Self { size: (1280.0, 800.0), db_path: None }
    }
}

fn runtime() -> Result<&'static tokio::runtime::Runtime, StudioError> {
    RUNTIME
        .get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().ok())
        .as_ref()
        .ok_or_else(|| StudioError::Store("tokio runtime could not start".to_owned()))
}

fn block<F>(make: impl FnOnce() -> F + Send) -> Result<F::Output, StudioError>
where
    F: Future,
    F::Output: Send,
{
    let runtime = runtime()?;
    if tokio::runtime::Handle::try_current().is_err() {
        return Ok(runtime.block_on(make()));
    }
    std::thread::scope(|scope| scope.spawn(|| runtime.block_on(make())).join()).map_err(|_| StudioError::Store("studio worker panicked".to_owned()))
}

pub struct StudioStore {
    repository: SqliteStore,
    pub url: String,
}

impl StudioStore {
    pub fn open(db_path: Option<&Path>) -> Result<Self, StudioError> {
        let url = db_path.map_or_else(|| "sqlite::memory:".to_owned(), |path| format!("sqlite://{}", path.display()));
        let target = url.clone();
        let repository = block(move || async move {
            let store = SqliteStore::connect(&target).await?;
            store.migrate().await?;
            Ok::<_, magicklala_store::StoreError>(store)
        })?
        .map_err(|error| StudioError::Store(error.to_string()))?;
        Ok(Self { repository, url })
    }

    fn spell_record(&self, spell: &SpellGraph) -> Result<SpellRecordId, StudioError> {
        let repository = self.repository.clone();
        let spell = spell.clone();
        block(move || async move {
            let project = match repository.list_projects().await?.into_iter().find(|project| project.name == STUDIO_PROJECT) {
                Some(project) => project,
                None => {
                    repository.create_project(NewProject { name: STUDIO_PROJECT.to_owned(), description: "Spells published from Studio".to_owned() }).await?
                }
            };
            let existing = repository.list_spells(project.id).await?.into_iter().find(|record| record.name == spell.name);
            let record = match existing {
                Some(record) => record,
                None => repository.create_spell(NewSpell { project_id: project.id, name: spell.name.clone(), graph: spell.clone() }).await?,
            };
            Ok::<_, magicklala_store::StoreError>(record.id)
        })?
        .map_err(|error| StudioError::Store(error.to_string()))
    }

    pub fn publish(&self, spell: &SpellGraph) -> Result<SpellRelease, StudioError> {
        let id = self.spell_record(spell)?;
        let repository = self.repository.clone();
        let graph = spell.clone();
        block(move || async move {
            repository.save_spell_graph(id, graph).await?;
            repository.create_release(NewSpellRelease { spell_id: id, notes: "Published from Studio".to_owned() }).await
        })?
        .map_err(|error| StudioError::Store(error.to_string()))
    }

    pub fn latest_release(&self) -> Result<Option<(SpellGraph, Vec<SpellRelease>)>, StudioError> {
        let repository = self.repository.clone();
        block(move || async move {
            let Some(project) = repository.list_projects().await?.into_iter().find(|project| project.name == STUDIO_PROJECT) else {
                return Ok(None);
            };
            let mut newest: Option<Vec<SpellRelease>> = None;
            for record in repository.list_spells(project.id).await? {
                let mut releases = repository.list_releases(record.id).await?;
                releases.sort_by_key(|release| release.version);
                let newer = match (releases.last(), newest.as_ref().and_then(|current| current.last())) {
                    (Some(candidate), Some(current)) => candidate.created_at > current.created_at,
                    (Some(_), None) => true,
                    _ => false,
                };
                if newer {
                    newest = Some(releases);
                }
            }
            Ok::<_, magicklala_store::StoreError>(newest.and_then(|releases| releases.last().map(|release| (release.graph.clone(), releases.clone()))))
        })?
        .map_err(|error| StudioError::Store(error.to_string()))
    }
}

#[derive(Default)]
struct TraceCollector(Mutex<Vec<TraceEvent>>);

impl EventEmitter for TraceCollector {
    fn emit(&self, event: TraceEvent) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

impl TraceCollector {
    fn take(&self) -> Vec<TraceEvent> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Finished(RunStatus),
    Invalid,
}

impl RunOutcome {
    pub fn label(self) -> String {
        match self {
            RunOutcome::Finished(status) => format!("{status:?}"),
            RunOutcome::Invalid => "Invalid".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunReport {
    pub outcome: RunOutcome,
    pub replies: Vec<String>,
    pub error: Option<String>,
}

impl RunReport {
    fn invalid(error: String) -> Self {
        Self { outcome: RunOutcome::Invalid, replies: Vec::new(), error: Some(error) }
    }

    fn from_summary(summary: &RunSummary, failure: Option<String>) -> Self {
        let replies = REPLY_OUTPUTS
            .iter()
            .filter_map(|key| summary.outputs.get(*key))
            .chain(summary.outputs.iter().filter(|(name, _)| name.ends_with(SENT_OUTPUT_SUFFIX)).map(|(_, value)| value))
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        let error = summary.error.clone().or(failure).filter(|_| summary.status != RunStatus::Succeeded);
        Self { outcome: RunOutcome::Finished(summary.status), replies, error }
    }
}

pub fn editable_field(kind: &str) -> Option<&'static str> {
    match kind {
        "text/template" => Some("template"),
        kind if kind.starts_with("code/") => Some("script"),
        kind if kind.starts_with("ai/") => Some("prompt"),
        _ => None,
    }
}

fn studio_services() -> EngineServices {
    let config = LlmGatewayConfig::from_env();
    let llm: Option<Arc<dyn magicklala_graph::LlmGateway>> =
        if config.base_url.is_some() || config.api_key.is_some() { Some(Arc::new(HttpLlmGateway::new(config))) } else { None };
    EngineServices { llm, memory: Some(Arc::new(InMemoryMemory::new())), knowledge: Some(Arc::new(InMemoryKnowledge::new())) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftTab {
    Palette,
    Agents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightTab {
    Inspector,
    Variables,
    Playground,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationState {
    Unchecked,
    Valid,
    Invalid(Vec<String>),
}

impl ValidationState {
    pub fn label(&self) -> &'static str {
        match self {
            ValidationState::Unchecked => "unchecked",
            ValidationState::Valid => "valid",
            ValidationState::Invalid(_) => "invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraceLine {
    pub at_ms: u64,
    pub kind: &'static str,
    pub detail: String,
    pub status: ExecutionStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub from_user: bool,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: u32,
    pub nodes: usize,
}

pub struct StudioApp {
    pub spell: SpellGraph,
    pub canvas: CanvasState,
    pub size: (f32, f32),
    pub specs: Vec<NodeSpec>,
    pub theme: Theme,
    pub left_tab: LeftTab,
    pub right_tab: RightTab,
    pub validation: ValidationState,
    pub trace: Vec<TraceLine>,
    pub trace_events: Vec<TraceEvent>,
    pub pending_trace: VecDeque<TraceEvent>,
    pub runs: u32,
    pub prompts: BTreeMap<NodeId, TextEdit>,
    pub chat: Vec<ChatMessage>,
    pub chat_input: TextEdit,
    pub releases: Vec<Release>,
    pub configs: BTreeMap<NodeId, serde_json::Value>,
    pub run_payload: serde_json::Value,
    pub last_run: Option<RunReport>,
    pub last_published: Option<SpellRelease>,
    pub store: Option<StudioStore>,
    pub store_error: Option<String>,
}

impl StudioApp {
    pub fn demo() -> Self {
        let db_path = LAUNCH_DB.lock().ok().and_then(|path| path.clone());
        let mut app = Self::open(&StudioConfig { db_path, ..StudioConfig::default() }).unwrap_or_else(|error| {
            let mut app = Self::from_registry();
            app.store_error = Some(error.to_string());
            app
        });
        if LAUNCH_THEME.lock().ok().and_then(|theme| *theme) == Some(Theme::Light) {
            app.toggle_theme();
        }
        app
    }

    pub fn open(config: &StudioConfig) -> Result<Self, StudioError> {
        let store = StudioStore::open(config.db_path.as_deref())?;
        let mut app = match store.latest_release()? {
            Some((graph, releases)) => {
                let mut app = Self::with_spell(graph, registry_specs());
                app.releases = releases.iter().map(|release| Release { version: release.version, nodes: release.graph.nodes.len() }).collect();
                app.last_published = releases.last().cloned();
                app
            }
            None => Self::from_registry(),
        };
        app.store = Some(store);
        Ok(app)
    }

    pub fn from_registry() -> Self {
        Self::with_spell(demo_spell(), registry_specs())
    }

    pub fn with_spell(spell: SpellGraph, specs: Vec<NodeSpec>) -> Self {
        let canvas = CanvasState::from_graph(&spell, &specs);
        let size = LAUNCH_SIZE.lock().ok().and_then(|size| *size).unwrap_or(StudioConfig::default().size);
        let mut app = Self {
            spell,
            canvas,
            size,
            specs,
            theme: Theme::Dark,
            left_tab: LeftTab::Palette,
            right_tab: RightTab::Inspector,
            validation: ValidationState::Unchecked,
            trace: Vec::new(),
            trace_events: Vec::new(),
            pending_trace: VecDeque::new(),
            runs: 0,
            prompts: BTreeMap::new(),
            chat: vec![ChatMessage { from_user: false, text: "Hi! Send a message to run this spell.".to_owned() }],
            chat_input: TextEdit::new(""),
            releases: Vec::new(),
            configs: BTreeMap::new(),
            run_payload: serde_json::json!({ "channel": STUDIO_CHANNEL, "sender": STUDIO_SENDER, "content": "Hello from Studio" }),
            last_run: None,
            last_published: None,
            store: None,
            store_error: None,
        };
        let nodes: Vec<(NodeId, String, serde_json::Value)> =
            app.spell.nodes.iter().map(|node| (node.id.clone(), node.kind.clone(), node.config.clone())).collect();
        for (id, kind, config) in nodes {
            app.track_config(id, &kind, config);
        }
        let viewport = app.canvas_size();
        app.canvas.fit(viewport);
        app.canvas.select(app.canvas.nodes.iter().find(|card| card.kind == "ai/generate_text").map(|card| card.id.clone()));
        app
    }

    pub fn canvas_size(&self) -> (f32, f32) {
        ((self.size.0 - LEFT_RAIL_WIDTH - INSPECTOR_WIDTH).max(200.0), (self.size.1 - TOOLBAR_HEIGHT - TRACE_HEIGHT).max(200.0))
    }

    pub fn spawn(&mut self, kind: &str) -> Option<NodeId> {
        let view = self.canvas_size();
        let count = self.canvas.nodes.len() as f32;
        self.spawn_at(kind, [view.0 * 0.5 - 100.0 + count * 12.0, view.1 * 0.62 + count * 8.0])
    }

    pub fn spawn_at(&mut self, kind: &str, screen: [f32; 2]) -> Option<NodeId> {
        let spec = self.specs.iter().find(|spec| spec.kind == kind)?.clone();
        let center = self.canvas.camera.screen_to_world(screen);
        let id = self.canvas.add_node_from_spec(&spec, center);
        self.track_config(id.clone(), kind, serde_json::Value::Null);
        self.validation = ValidationState::Unchecked;
        self.sync_spell();
        Some(id)
    }

    fn track_config(&mut self, id: NodeId, kind: &str, config: serde_json::Value) {
        if let Some(field) = editable_field(kind) {
            let text = match config.get(field) {
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
                None if config.is_null() => default_prompt(kind),
                None => String::new(),
            };
            self.prompts.insert(id.clone(), TextEdit::new(text));
        }
        self.configs.insert(id, config);
    }

    pub fn node_config(&self, id: &NodeId, kind: &str) -> serde_json::Value {
        let mut config = self.configs.get(id).cloned().unwrap_or(serde_json::Value::Null);
        let (Some(field), Some(edit)) = (editable_field(kind), self.prompts.get(id)) else { return config };
        let present = config.get(field).is_some();
        if !present && edit.text.is_empty() {
            return config;
        }
        if present
            && config.get(field).and_then(serde_json::Value::as_str).is_none()
            && config.get(field).map(ToString::to_string).as_deref() == Some(edit.text.as_str())
        {
            return config;
        }
        if !config.is_object() {
            config = json!({});
        }
        if let Some(map) = config.as_object_mut() {
            map.insert(field.to_owned(), serde_json::Value::String(edit.text.clone()));
        }
        config
    }

    pub fn sync_spell(&mut self) {
        self.spell.nodes = self
            .canvas
            .nodes
            .iter()
            .map(|card| NodeInstance { id: card.id.clone(), kind: card.kind.clone(), position: card.position, config: self.node_config(&card.id, &card.kind) })
            .collect();
        self.spell.wires = self
            .canvas
            .wires
            .iter()
            .map(|wire| Wire {
                id: wire.id.clone(),
                from_node: wire.from_node.clone(),
                from_socket: wire.from_socket.clone(),
                to_node: wire.to_node.clone(),
                to_socket: wire.to_socket.clone(),
            })
            .collect();
    }

    pub fn validate(&mut self) -> &ValidationState {
        self.sync_spell();
        self.validation = match SpellValidator::new(self.specs.clone()).validate(&self.spell) {
            Ok(()) => ValidationState::Valid,
            Err(errors) => ValidationState::Invalid(errors.iter().map(ToString::to_string).collect()),
        };
        &self.validation
    }

    pub fn run_spell(&mut self) -> usize {
        let payload = self.run_payload.clone();
        self.execute(None, payload);
        self.trace.len()
    }

    fn trigger(&self, prefer: Option<&str>, payload: &serde_json::Value) -> Option<RunTrigger> {
        let spec_of = |node: &NodeInstance| self.specs.iter().find(|spec| spec.kind == node.kind && spec.is_trigger);
        let triggers: Vec<(&NodeInstance, &NodeSpec)> = self.spell.nodes.iter().filter_map(|node| spec_of(node).map(|spec| (node, spec))).collect();
        let (node, spec) = prefer.and_then(|kind| triggers.iter().find(|(node, _)| node.kind == kind)).or_else(|| triggers.first()).copied()?;
        let socket = spec.outputs.first().map_or_else(|| "out".to_owned(), |socket| socket.name.clone());
        Some(RunTrigger { node: node.id.clone(), socket, payload: Value::from(payload.clone()) })
    }

    pub fn execute(&mut self, prefer: Option<&str>, payload: serde_json::Value) -> RunReport {
        self.runs += 1;
        self.trace.clear();
        self.trace_events.clear();
        self.pending_trace.clear();
        self.canvas.nodes.iter_mut().for_each(|card| card.status = ExecutionStatus::Idle);
        self.canvas.wires.iter_mut().for_each(|wire| wire.active = false);
        let report = match self.validate().clone() {
            ValidationState::Invalid(errors) => RunReport::invalid(errors.join("; ")),
            _ => match self.trigger(prefer, &payload) {
                None => RunReport::invalid("spell has no trigger node".to_owned()),
                Some(trigger) => self.run_engine(trigger),
            },
        };
        if report.outcome == RunOutcome::Invalid {
            let detail = report.error.clone().unwrap_or_default();
            self.trace.push(TraceLine { at_ms: 0, kind: "RunRejected", detail, status: ExecutionStatus::Failed });
        }
        self.last_run = Some(report.clone());
        report
    }

    fn run_engine(&mut self, trigger: RunTrigger) -> RunReport {
        let engine = SpellEngine::new(Arc::new(default_registry()), studio_services());
        let collector = Arc::new(TraceCollector::default());
        let emitter: Arc<dyn EventEmitter> = collector.clone();
        let graph = self.spell.clone();
        let outcome = block(move || async move { engine.run(&graph, trigger, emitter).await.map_err(|error| error.to_string()) });
        self.pending_trace = collector.take().into();
        while self.step_trace() {}
        let finished = self.trace_events.iter().rev().find_map(|event| match event {
            TraceEvent::RunFinished { summary, .. } => Some(summary.clone()),
            _ => None,
        });
        match (outcome, finished) {
            (Ok(Ok(summary)), _) => RunReport::from_summary(&summary, None),
            (Ok(Err(error)), Some(summary)) => RunReport::from_summary(&summary, Some(error)),
            (Ok(Err(error)), None) => RunReport::invalid(error),
            (Err(error), _) => RunReport { outcome: RunOutcome::Finished(RunStatus::Failed), replies: Vec::new(), error: Some(error.to_string()) },
        }
    }

    pub fn step_trace(&mut self) -> bool {
        let Some(event) = self.pending_trace.pop_front() else { return false };
        self.canvas.apply_trace_event(&event);
        let at_ms = self.trace.len() as u64 * 140;
        self.trace.push(trace_line(&event, at_ms, &self.canvas));
        self.trace_events.push(event);
        true
    }

    pub fn close_context_menu(&mut self) {
        self.canvas.context_menu = None;
    }

    pub fn quick_add_from_menu(&mut self) {
        let anchor = self.canvas.context_menu.unwrap_or([self.canvas_size().0 * 0.5, self.canvas_size().1 * 0.5]);
        self.close_context_menu();
        self.spawn_at(QUICK_ADD_KIND, anchor);
    }

    pub fn delete_selected_node(&mut self) {
        self.close_context_menu();
        if let Some(removed) = self.canvas.remove_selected_node() {
            self.prompts.remove(&removed.id);
            self.configs.remove(&removed.id);
            self.validation = ValidationState::Unchecked;
            self.sync_spell();
        }
    }

    pub fn fit_view(&mut self) {
        self.close_context_menu();
        let size = self.canvas_size();
        self.canvas.fit(size);
    }

    pub fn compact_toolbar(&self) -> bool {
        self.size.0 < COMPACT_TOOLBAR_BELOW
    }

    pub fn toggle_theme(&mut self) {
        self.theme = if self.theme == Theme::Dark { Theme::Light } else { Theme::Dark };
        self.canvas.light = self.theme == Theme::Light;
    }

    pub fn publish(&mut self) -> Result<u32, StudioError> {
        if let ValidationState::Invalid(errors) = self.validate().clone() {
            let error = StudioError::Store(format!("cannot publish an invalid spell: {}", errors.join("; ")));
            self.store_error = Some(error.to_string());
            return Err(error);
        }
        if self.store.is_none() {
            self.store = Some(StudioStore::open(None)?);
        }
        let store = self.store.as_ref().ok_or_else(|| StudioError::Store("no studio store".to_owned()))?;
        let release = store.publish(&self.spell).inspect_err(|error| self.store_error = Some(error.to_string()))?;
        self.releases.push(Release { version: release.version, nodes: release.graph.nodes.len() });
        self.store_error = None;
        let version = release.version;
        self.last_published = Some(release);
        Ok(version)
    }

    pub fn send_chat(&mut self) {
        let message = self.chat_input.text.trim().to_owned();
        if message.is_empty() {
            return;
        }
        self.chat.push(ChatMessage { from_user: true, text: message.clone() });
        self.chat_input = TextEdit::new("");
        let report = self.execute(Some(CHAT_TRIGGER_KIND), json!({ "channel": STUDIO_CHANNEL, "sender": STUDIO_SENDER, "content": message }));
        let text = match (report.replies.is_empty(), &report.error) {
            (false, _) => report.replies.join("\n"),
            (true, Some(error)) => format!("Run #{} {}: {error}", self.runs, report.outcome.label().to_lowercase()),
            (true, None) => format!("Run #{} finished without a reply.", self.runs),
        };
        self.chat.push(ChatMessage { from_user: false, text });
    }
}

pub fn run_studio(config: StudioConfig) -> Result<(), StudioError> {
    remember_size(config.size);
    if let Ok(mut slot) = LAUNCH_DB.lock() {
        slot.clone_from(&config.db_path);
    }
    remember_theme(Theme::Dark);
    App::new(spec(config)).args(Vec::new()).try_run().map_err(StudioError::Rune)
}

pub fn render_headless_snapshot(out_path: &Path) -> Result<(), StudioError> {
    render_headless_snapshot_sized(out_path, StudioConfig::default().size)
}

pub fn render_headless_snapshot_sized(out_path: &Path, size: (f32, f32)) -> Result<(), StudioError> {
    render_headless_snapshot_themed(out_path, size, Theme::Dark)
}

pub fn render_headless_snapshot_themed(out_path: &Path, size: (f32, f32), theme: Theme) -> Result<(), StudioError> {
    let target = out_path.display().to_string();
    remember_size(size);
    remember_theme(theme);
    let args =
        vec!["--snap".to_owned(), target.clone(), "--size".to_owned(), format!("{}x{}", size.0 as u32, size.1 as u32), "--scale".to_owned(), "1".to_owned()];
    App::new(spec(StudioConfig { size, db_path: None })).args(args).try_run().map_err(StudioError::Rune)?;
    if out_path.is_file() { Ok(()) } else { Err(StudioError::MissingSnapshot(target)) }
}

fn remember_theme(theme: Theme) {
    if let Ok(mut slot) = LAUNCH_THEME.lock() {
        *slot = Some(theme);
    }
}

fn remember_size(size: (f32, f32)) {
    if let Ok(mut slot) = LAUNCH_SIZE.lock() {
        *slot = Some(size);
    }
}

fn spec(config: StudioConfig) -> AppSpec<StudioApp> {
    AppSpec::new(TITLE, config.size, StudioApp::demo, studio_view)
}

struct Tones {
    background: Color,
    panel: Color,
    raised: Color,
    line: Color,
    ink: Color,
    muted: Color,
    accent: Color,
    accent_ink: Color,
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color { r, g, b, a }
}

fn tones(theme: Theme) -> Tones {
    match theme {
        Theme::Dark => Tones {
            background: rgba(0.04, 0.043, 0.052, 1.0),
            panel: rgba(0.078, 0.082, 0.098, 1.0),
            raised: rgba(0.12, 0.126, 0.148, 1.0),
            line: rgba(1.0, 1.0, 1.0, 0.07),
            ink: rgba(0.92, 0.93, 0.95, 1.0),
            muted: rgba(0.55, 0.58, 0.64, 1.0),
            accent: rgba(0.47, 0.4, 0.98, 1.0),
            accent_ink: rgba(1.0, 1.0, 1.0, 1.0),
        },
        Theme::Light => Tones {
            background: rgba(0.93, 0.935, 0.945, 1.0),
            panel: rgba(0.985, 0.987, 0.99, 1.0),
            raised: rgba(0.93, 0.935, 0.95, 1.0),
            line: rgba(0.0, 0.0, 0.0, 0.09),
            ink: rgba(0.1, 0.11, 0.13, 1.0),
            muted: rgba(0.4, 0.42, 0.47, 1.0),
            accent: rgba(0.4, 0.33, 0.93, 1.0),
            accent_ink: rgba(1.0, 1.0, 1.0, 1.0),
        },
    }
}

fn font(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
}

fn label(content: impl Into<String>, size: f32, weight: f32, color: Color) -> El {
    text(content).font(font(size, weight)).color(color)
}

fn button(key: &str, caption: &str, primary: bool, t: &Tones, action: fn(&mut StudioApp)) -> El {
    sized_button(key, caption, primary, 12.0, t, action)
}

fn sized_button(key: &str, caption: &str, primary: bool, padding: f32, t: &Tones, action: fn(&mut StudioApp)) -> El {
    let (fill, ink) = if primary { (t.accent, t.accent_ink) } else { (t.raised, t.ink) };
    row().key(key).h(30.0).px(padding).items_center().radius(7.0).fill(fill).border(1.0, t.line).child(label(caption, 12.5, 600.0, ink)).on_press(action)
}

fn menu_item(key: &str, caption: &str, enabled: bool, t: &Tones, action: fn(&mut StudioApp)) -> El {
    row()
        .key(key)
        .w_full()
        .h(30.0)
        .px(10.0)
        .items_center()
        .radius(6.0)
        .fill(t.raised)
        .child(label(caption, 12.5, 550.0, if enabled { t.ink } else { t.muted }))
        .on_press(action)
}

fn context_menu(app: &StudioApp, t: &Tones) -> Option<El> {
    let anchor = app.canvas.context_menu?;
    let view = app.canvas_size();
    let x = anchor[0].min(view.0 - CONTEXT_MENU_SIZE.0).max(0.0);
    let y = anchor[1].min(view.1 - CONTEXT_MENU_SIZE.1).max(0.0);
    let quick_add = app.specs.iter().find(|spec| spec.kind == QUICK_ADD_KIND).map_or_else(|| QUICK_ADD_KIND.to_owned(), |spec| format!("Add {}", spec.title));
    let delete = app.canvas.selected_card().map_or_else(|| "Delete node (none selected)".to_owned(), |card| format!("Delete {}", card.title));
    Some(
        col()
            .key("context-menu")
            .size(CONTEXT_MENU_SIZE.0, CONTEXT_MENU_SIZE.1)
            .padding(6.0)
            .gap(4.0)
            .radius(9.0)
            .fill(t.panel)
            .border(1.0, t.line)
            .shadow(6.0, 18.0, rgba(0.0, 0.0, 0.0, 0.35))
            .absolute(x, y)
            .child(menu_item(&palette_key(QUICK_ADD_KIND).replacen("palette-", "context-add-", 1), &quick_add, true, t, StudioApp::quick_add_from_menu))
            .child(menu_item("context-delete-node", &delete, app.canvas.selection.is_some(), t, StudioApp::delete_selected_node))
            .child(menu_item("context-fit-view", "Fit view", true, t, StudioApp::fit_view))
            .child(menu_item("context-dismiss", "Dismiss", true, t, StudioApp::close_context_menu)),
    )
}

fn pill(caption: impl Into<String>, color: Color, t: &Tones) -> El {
    row()
        .gap(6.0)
        .h(24.0)
        .px(9.0)
        .items_center()
        .radius(12.0)
        .fill(t.raised)
        .child(el().size(7.0, 7.0).radius(3.5).fill(color))
        .child(label(caption, 11.5, 550.0, t.ink))
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
    col().key("studio").w_full().h_full().fill(t.background).child(toolbar(app, &t)).child(middle).child(trace_strip(app, &t))
}

fn canvas_with_menu(app: &StudioApp, t: &Tones) -> El {
    let board = interactive_canvas_view::<StudioApp>(&app.canvas, app.canvas_size(), |app| &mut app.canvas);
    match context_menu(app, t) {
        Some(menu) => board.child(menu),
        None => board,
    }
}

fn toolbar(app: &StudioApp, t: &Tones) -> El {
    let compact = app.compact_toolbar();
    let (padding, gap, readout) = if compact { (9.0, 6.0, 44.0) } else { (12.0, 8.0, 52.0) };
    let tool = |key: &str, caption: &str, primary: bool, action: fn(&mut StudioApp)| sized_button(key, caption, primary, padding, t, action);
    let brand = row()
        .gap(8.0)
        .items_center()
        .child(el().size(22.0, 22.0).radius(6.0).fill(t.accent).center().child(label("M", 13.0, 800.0, t.accent_ink)))
        .child(label("MagickLaLa", 14.0, 700.0, t.ink));
    let validation_color = match app.validation {
        ValidationState::Unchecked => t.muted,
        ValidationState::Valid => status_color(ExecutionStatus::Succeeded),
        ValidationState::Invalid(_) => status_color(ExecutionStatus::Failed),
    };
    let zoom = row()
        .gap(4.0)
        .items_center()
        .child(tool("zoom-out", "-", false, |app| app.canvas.camera.zoom_by(1.0 / 1.2, [app.canvas_size().0 / 2.0, app.canvas_size().1 / 2.0])))
        .child(row().key("zoom-readout").w(readout).justify_center().child(label(format!("{}%", app.canvas.camera.zoom_percent()), 12.0, 600.0, t.ink)))
        .child(tool("zoom-in", "+", false, |app| app.canvas.camera.zoom_by(1.2, [app.canvas_size().0 / 2.0, app.canvas_size().1 / 2.0])))
        .child(tool("zoom-fit", "Fit", false, StudioApp::fit_view));
    let stats = if compact {
        format!("{}n · {}w", app.canvas.nodes.len(), app.canvas.wires.len())
    } else {
        format!("{} nodes · {} wires", app.canvas.nodes.len(), app.canvas.wires.len())
    };
    let left = row()
        .key("toolbar-left")
        .gap(if compact { 10.0 } else { 14.0 })
        .items_center()
        .child(brand)
        .child(el().w(1.0).h(22.0).fill(t.line))
        .child(label(&app.spell.name, 13.0, 600.0, t.ink))
        .child(pill(stats, t.accent, t));
    let right = row()
        .gap(gap)
        .items_center()
        .child(pill(app.validation.label(), validation_color, t).key("validation-pill"))
        .child(tool("validate", "Validate", false, |app| {
            app.validate();
        }))
        .child(tool("run-spell", if compact { "Run" } else { "Run Spell" }, true, |app| {
            app.run_spell();
        }))
        .child(tool("publish", "Publish", false, |app| {
            app.publish().ok();
        }))
        .child(tool("theme-toggle", if app.theme == Theme::Dark { "Light" } else { "Dark" }, false, StudioApp::toggle_theme))
        .child(zoom);
    row()
        .key("toolbar")
        .w_full()
        .h(TOOLBAR_HEIGHT)
        .px(if compact { 10.0 } else { 14.0 })
        .gap(gap)
        .items_center()
        .justify_between()
        .fill(t.panel)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.line)
        .child(left)
        .child(right)
}

fn tab(key: &str, caption: &str, active: bool, t: &Tones, action: fn(&mut StudioApp)) -> El {
    row()
        .key(key)
        .grow(1.0)
        .h(28.0)
        .justify_center()
        .items_center()
        .radius(6.0)
        .fill(if active { t.raised } else { rgba(0.0, 0.0, 0.0, 0.0) })
        .child(label(caption, 12.0, if active { 650.0 } else { 500.0 }, if active { t.ink } else { t.muted }))
        .on_press(action)
}

fn palette_key(kind: &str) -> String {
    format!("palette-{}", kind.replace('/', "-"))
}

fn left_rail(app: &StudioApp, t: &Tones) -> El {
    let tabs = row()
        .gap(4.0)
        .p_all(4.0, t)
        .child(tab("tab-palette", "Nodes", app.left_tab == LeftTab::Palette, t, |app| app.left_tab = LeftTab::Palette))
        .child(tab("tab-agents", "Agents & Releases", app.left_tab == LeftTab::Agents, t, |app| app.left_tab = LeftTab::Agents));
    let body = match app.left_tab {
        LeftTab::Palette => palette_categories(&app.specs).iter().fold(col().gap(4.0), |list, category| {
            let entries: Vec<&NodeSpec> = app.specs.iter().filter(|spec| spec.category.eq_ignore_ascii_case(category)).collect();
            if entries.is_empty() {
                return list;
            }
            let heading = row().gap(6.0).items_center().pt(8.0).pb(2.0).child(el().size(6.0, 6.0).radius(3.0).fill(category_accent(category))).child(label(
                category.to_uppercase(),
                10.5,
                700.0,
                t.muted,
            ));
            entries.into_iter().fold(list.child(heading), |list, spec| {
                let kind = spec.kind.clone();
                list.child(
                    row()
                        .key(palette_key(&spec.kind))
                        .w_full()
                        .h(30.0)
                        .px(10.0)
                        .gap(8.0)
                        .items_center()
                        .radius(6.0)
                        .fill(t.raised)
                        .child(el().w(3.0).h(14.0).radius(1.5).fill(category_accent(&spec.category)))
                        .child(label(&spec.title, 12.5, 500.0, t.ink))
                        .on_press(move |app: &mut StudioApp| {
                            app.spawn(&kind);
                        }),
                )
            })
        }),
        LeftTab::Agents => {
            let agents = ["Support Agent", "Research Agent"].iter().fold(col().gap(6.0).child(label("AGENTS", 10.5, 700.0, t.muted)), |list, name| {
                list.child(
                    row().h(34.0).px(10.0).items_center().radius(6.0).fill(t.raised).justify_between().child(label(*name, 12.5, 550.0, t.ink)).child(pill(
                        "live",
                        status_color(ExecutionStatus::Succeeded),
                        t,
                    )),
                )
            });
            let heading = agents.child(label("RELEASES", 10.5, 700.0, t.muted));
            let heading = match (&app.store_error, app.releases.is_empty()) {
                (Some(error), _) => heading.child(label(error, 11.5, 500.0, status_color(ExecutionStatus::Failed)).wrap()),
                (None, true) => heading.child(label("Nothing published yet. Press Publish.", 11.5, 500.0, t.muted)),
                (None, false) => heading,
            };
            app.releases
                .iter()
                .rev()
                .fold(heading, |list, release| list.child(label(format!("v{} · {} nodes", release.version, release.nodes), 12.0, 500.0, t.ink)))
        }
    };
    col()
        .key("left-rail")
        .w(LEFT_RAIL_WIDTH)
        .h_full()
        .min_h(0.0)
        .fill(t.panel)
        .border_sides([0.0, 1.0, 0.0, 0.0], t.line)
        .child(tabs)
        .child(col().key("left-rail-scroll").w_full().grow(1.0).min_h(0.0).px(10.0).pb(10.0).scroll(ScrollSpec { stick_bottom: false }).child(body))
}

trait PanelPadding {
    fn p_all(self, amount: f32, t: &Tones) -> Self;
}

impl PanelPadding for El {
    fn p_all(self, amount: f32, t: &Tones) -> Self {
        self.padding(amount).mx(8.0).mt(8.0).mb(4.0).radius(8.0).fill(t.background)
    }
}

fn inspector(app: &StudioApp, t: &Tones) -> El {
    let tabs = row()
        .gap(4.0)
        .p_all(4.0, t)
        .child(tab("tab-inspector", "Inspector", app.right_tab == RightTab::Inspector, t, |app| app.right_tab = RightTab::Inspector))
        .child(tab("tab-variables", "Variables", app.right_tab == RightTab::Variables, t, |app| app.right_tab = RightTab::Variables))
        .child(tab("tab-playground", "Playground", app.right_tab == RightTab::Playground, t, |app| app.right_tab = RightTab::Playground));
    let body = match app.right_tab {
        RightTab::Inspector => inspector_body(app, t),
        RightTab::Variables => app.spell.variables.iter().fold(col().gap(6.0), |list, variable| {
            list.child(
                row()
                    .h(32.0)
                    .px(10.0)
                    .items_center()
                    .justify_between()
                    .radius(6.0)
                    .fill(t.raised)
                    .child(label(&variable.name, 12.5, 600.0, t.ink))
                    .child(label(format!("{:?}", variable.ty), 11.5, 500.0, t.muted)),
            )
        }),
        RightTab::Playground => playground(app, t),
    };
    col()
        .key("inspector")
        .w(INSPECTOR_WIDTH)
        .h_full()
        .min_h(0.0)
        .fill(t.panel)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.line)
        .child(tabs)
        .child(col().key("inspector-scroll").w_full().grow(1.0).min_h(0.0).px(12.0).pb(12.0).scroll(ScrollSpec { stick_bottom: false }).child(body))
}

fn inspector_body(app: &StudioApp, t: &Tones) -> El {
    let Some(card) = app.canvas.selected_card() else {
        return col().pt(12.0).child(label("Select a node on the canvas to inspect it.", 12.5, 500.0, t.muted));
    };
    let sockets = card.inputs.iter().map(|socket| ("in", socket)).chain(card.outputs.iter().map(|socket| ("out", socket))).fold(
        col().gap(4.0),
        |list, (side, socket)| {
            list.child(
                row()
                    .gap(8.0)
                    .items_center()
                    .child(el().size(8.0, 8.0).radius(4.0).fill(magicklala_canvas::socket_color(socket.ty)))
                    .child(label(format!("{side} · {}", socket.name), 12.0, 500.0, t.ink))
                    .child(label(format!("{:?}", socket.ty), 11.0, 500.0, t.muted)),
            )
        },
    );
    let id = card.id.clone();
    let field = editable_field(&card.kind);
    let editor_height = if app.size.1 < 720.0 { 118.0 } else { 150.0 };
    let editor = field.and_then(|field| app.prompts.get(&card.id).map(|edit| (field, edit))).map(|(field, edit)| {
        text_input(edit, font(12.5, 450.0).mono())
            .key("prompt-editor")
            .label(format!("{} {field}", card.title))
            .w_full()
            .h(editor_height)
            .padding(10.0)
            .radius(8.0)
            .fill(t.background)
            .border(1.0, t.line)
            .color(t.ink)
            .on_edit(move |app: &mut StudioApp| app.prompts.entry(id.clone()).or_insert_with(|| TextEdit::new("")))
    });
    let column = col()
        .gap(10.0)
        .pt(10.0)
        .child(row().gap(8.0).items_center().child(el().w(4.0).h(28.0).radius(2.0).fill(category_accent(&card.category))).child(
            col().gap(2.0).child(label(&card.title, 15.0, 700.0, t.ink)).child(label(format!("{} · {}", card.kind, card.status.label()), 11.5, 500.0, t.muted)),
        ))
        .child(label("SOCKETS", 10.5, 700.0, t.muted))
        .child(sockets)
        .pb(14.0);
    match (field, editor) {
        (Some(field), Some(editor)) => column.child(label(editor_caption(field), 10.5, 700.0, t.muted)).child(editor),
        _ => column,
    }
}

fn editor_caption(field: &str) -> &'static str {
    match field {
        "script" => "RHAI SCRIPT",
        "template" => "TEMPLATE",
        _ => "PROMPT",
    }
}

fn playground(app: &StudioApp, t: &Tones) -> El {
    let messages = app.chat.iter().fold(col().gap(8.0).pt(10.0), |list, message| {
        let bubble = el()
            .max_w(230.0)
            .padding(10.0)
            .radius(10.0)
            .fill(if message.from_user { t.accent } else { t.raised })
            .child(label(&message.text, 12.5, 500.0, if message.from_user { t.accent_ink } else { t.ink }).wrap());
        list.child(if message.from_user { row().w_full().justify_end().child(bubble) } else { row().w_full().child(bubble) })
    });
    let composer = row()
        .gap(8.0)
        .pt(10.0)
        .items_center()
        .child(
            text_input(&app.chat_input, font(12.5, 450.0))
                .single_line()
                .key("chat-input")
                .label("Chat message")
                .grow(1.0)
                .h(32.0)
                .px(10.0)
                .radius(8.0)
                .fill(t.background)
                .border(1.0, t.line)
                .color(t.ink)
                .on_edit(|app: &mut StudioApp| &mut app.chat_input)
                .on_submit(|app: &mut StudioApp, _| app.send_chat()),
        )
        .child(button("chat-send", "Send", true, t, StudioApp::send_chat));
    col().child(messages).child(composer)
}

fn trace_strip(app: &StudioApp, t: &Tones) -> El {
    let header = row().gap(10.0).items_center().child(label("TRACE", 10.5, 700.0, t.muted)).child(label(
        if app.runs == 0 { "No runs yet. Press Run Spell.".to_owned() } else { format!("run #{} · {} events", app.runs, app.trace.len()) },
        11.5,
        500.0,
        t.muted,
    ));
    let entries = app.trace.iter().fold(row().gap(6.0).flex_wrap(), |list, line| {
        list.child(
            row()
                .gap(6.0)
                .h(22.0)
                .px(8.0)
                .items_center()
                .radius(5.0)
                .fill(t.raised)
                .child(el().size(6.0, 6.0).radius(3.0).fill(status_color(line.status)))
                .child(label(format!("+{}ms", line.at_ms), 10.5, 500.0, t.muted).font(font(10.5, 500.0).mono()))
                .child(label(line.kind, 11.0, 650.0, t.ink))
                .child(label(&line.detail, 11.0, 450.0, t.muted)),
        )
    });
    col()
        .key("trace-strip")
        .w_full()
        .h(TRACE_HEIGHT)
        .min_h(0.0)
        .padding(10.0)
        .gap(6.0)
        .fill(t.panel)
        .border_sides([0.0, 0.0, 0.0, 1.0], t.line)
        .child(header)
        .child(col().key("trace-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: true }).child(entries))
}

fn trace_line(event: &TraceEvent, at_ms: u64, canvas: &CanvasState) -> TraceLine {
    let title = |id: &NodeId| canvas.card(id).map_or_else(|| id.0.clone(), |card| card.title.clone());
    let (kind, detail, status) = match event {
        TraceEvent::RunStarted { .. } => ("RunStarted", String::new(), ExecutionStatus::Running),
        TraceEvent::NodeStarted { node_id, .. } => ("NodeStarted", title(node_id), ExecutionStatus::Running),
        TraceEvent::NodeFinished { node_id, duration_ms, .. } => ("NodeFinished", format!("{} {duration_ms}ms", title(node_id)), ExecutionStatus::Succeeded),
        TraceEvent::NodeFailed { node_id, error, .. } => ("NodeFailed", format!("{} {error}", title(node_id)), ExecutionStatus::Failed),
        TraceEvent::WireActivated { wire_id, .. } => ("WireActivated", wire_id.0.clone(), ExecutionStatus::Succeeded),
        TraceEvent::StreamChunk { node_id, .. } => ("StreamChunk", title(node_id), ExecutionStatus::Running),
        TraceEvent::VariableSet { name, .. } => ("VariableSet", name.clone(), ExecutionStatus::Succeeded),
        TraceEvent::RunFinished { summary, .. } => (
            "RunFinished",
            format!("{:?}", summary.status),
            if summary.status == RunStatus::Succeeded { ExecutionStatus::Succeeded } else { ExecutionStatus::Failed },
        ),
    };
    TraceLine { at_ms, kind, detail, status }
}

fn default_prompt(kind: &str) -> String {
    match kind {
        "ai/generate_text" => "You are a helpful agent.\n\nMemory:\n{{memory}}\n\nKnowledge:\n{{knowledge}}\n\nUser: {{content}}".to_owned(),
        "memory/search" => "Recall the 5 most relevant memories for {{content}}".to_owned(),
        "knowledge/search" => "Search the knowledge base for {{content}}".to_owned(),
        "code/rhai_script" => "let reply = input.content;\nreply.to_upper()".to_owned(),
        _ => String::new(),
    }
}

pub fn registry_specs() -> Vec<NodeSpec> {
    magicklala_nodes::default_registry().specs().into_iter().filter(|spec| !spec.kind.starts_with(LEGACY_KIND_PREFIX)).collect()
}

pub fn palette_categories(specs: &[NodeSpec]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for spec in specs {
        let category = spec.category.to_ascii_lowercase();
        if !found.contains(&category) {
            found.push(category);
        }
    }
    let rank = |category: &String| PALETTE_CATEGORY_ORDER.iter().position(|known| known == category).unwrap_or(PALETTE_CATEGORY_ORDER.len());
    found.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    found
}

fn demo_spell() -> SpellGraph {
    let mut spell = SpellGraph::new("Support Agent");
    let place =
        |id: &str, kind: &str, position: [f32; 2]| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position, config: serde_json::Value::Null };
    spell.nodes = vec![
        place("trigger", "event/on_message", [40.0, 170.0]),
        place("recall", "memory/search", [310.0, 40.0]),
        place("search", "knowledge/search", [310.0, 290.0]),
        place("llm", "ai/generate_text", [590.0, 140.0]),
        place("reply", "io/send_message", [870.0, 210.0]),
    ];
    let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
        id: WireId::from(id),
        from_node: NodeId::from(from.0),
        from_socket: from.1.to_owned(),
        to_node: NodeId::from(to.0),
        to_socket: to.1.to_owned(),
    };
    spell.wires = vec![
        wire("w1", ("trigger", "out"), ("recall", "in")),
        wire("w2", ("trigger", "content"), ("recall", "query")),
        wire("w3", ("recall", "out"), ("search", "in")),
        wire("w4", ("trigger", "content"), ("search", "query")),
        wire("w5", ("search", "out"), ("llm", "in")),
        wire("w6", ("trigger", "content"), ("llm", "prompt")),
        wire("w7", ("recall", "text"), ("llm", "system")),
        wire("w8", ("llm", "out"), ("reply", "in")),
        wire("w9", ("llm", "text"), ("reply", "content")),
    ];
    spell.variables = variables();
    spell
}
fn variables() -> Vec<VariableDef> {
    serde_json::from_value(json!([
        { "name": "agent_name", "ty": "string", "default": "Support Agent" },
        { "name": "max_memories", "ty": "int", "default": 5 },
        { "name": "temperature", "ty": "float", "default": 0.4 }
    ]))
    .unwrap_or_default()
}
