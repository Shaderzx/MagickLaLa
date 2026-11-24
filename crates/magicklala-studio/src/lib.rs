use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use magicklala_canvas::{CanvasState, ExecutionStatus, NodeCardModel, SocketRef, WireModel, WorkbenchMode};
use magicklala_graph::{
    EngineServices, EventEmitter, NodeCatalog, NodeId, NodeInstance, NodeSpec, RunStatus, RunSummary, RunTrigger, SUBGRAPH_KIND, SocketType, SpellEngine,
    SpellGraph, SpellId, SpellValidator, SubgraphKey, SubgraphRef, SubgraphResolver, TraceEvent, ValidationError, Value, VariableDef, Wire, WireId,
    resolve_subgraphs_from,
};
use magicklala_nodes::{HttpLlmGateway, InMemoryKnowledge, InMemoryMemory, LlmGatewayConfig, default_registry};
use magicklala_store::{NewProject, NewSpell, NewSpellRelease, ProjectId, Repository, SpellRecord, SpellRecordId, SpellRelease, SqliteStore, StoreError};
use rune::text::TextSystem;
use rune::{App, AppSpec, TextEdit, Waker};
use serde_json::json;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

const TITLE: &str = "MagickLaLa Studio";
pub const TOOLBAR_HEIGHT: f32 = 52.0;
pub const LEFT_RAIL_WIDTH: f32 = 228.0;
pub const INSPECTOR_WIDTH: f32 = 296.0;
pub const TRACE_HEIGHT: f32 = 102.0;
pub const IDLE_TRACE_HEIGHT: f32 = 46.0;
pub const PALETTE_CATEGORY_ORDER: [&str; 13] =
    ["events", "flow", "ai", "memory", "knowledge", "io", "code", "variables", "text", "logic", "json", "network", "time"];
pub const LEGACY_KIND_PREFIX: &str = "core/";
pub const CONTEXT_MENU_SIZE: (f32, f32) = (248.0, 158.0);
pub const QUICK_ADD_KIND: &str = "ai/generate_text";
pub const COMPACT_TOOLBAR_BELOW: f32 = 1120.0;
pub const STUDIO_PROJECT: &str = "Studio";
pub const STUDIO_CHANNEL: &str = "studio";
pub const STUDIO_SENDER: &str = "studio-user";
pub const CHAT_TRIGGER_KIND: &str = "event/on_message";
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(60);
pub const PROMPT_EDITOR_PADDING: f32 = 10.0;
pub const PROMPT_EDITOR_HEIGHT: f32 = 172.0;
pub const COMPACT_PROMPT_EDITOR_HEIGHT: f32 = 152.0;
pub const COMPACT_INSPECTOR_BELOW: f32 = 720.0;
pub const RUN_SLOT_WIDTH: f32 = 90.0;
pub const COMPACT_RUN_SLOT_WIDTH: f32 = 64.0;
const PUBLISH_NOTES: &str = "Published from Studio";

static BRICOLAGE_GROTESK: &[u8] = include_bytes!("../assets/fonts/BricolageGrotesque-Variable.ttf");
static JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");
pub const STUDIO_FONTS: &[&[u8]] = &[BRICOLAGE_GROTESK, JETBRAINS_MONO];
pub const STUDIO_FAMILIES: (&str, &str) = ("Bricolage Grotesque, system-ui, sans-serif", "JetBrains Mono, ui-monospace, monospace");

pub fn studio_text_system() -> TextSystem {
    TextSystem::for_window(STUDIO_FONTS, STUDIO_FAMILIES)
}

static LAUNCH_SIZE: Mutex<Option<(f32, f32)>> = Mutex::new(None);
static LAUNCH_THEME: Mutex<Option<Theme>> = Mutex::new(None);
static LAUNCH_DB: Mutex<Option<PathBuf>> = Mutex::new(None);
static LAUNCH_RUN_TIMEOUT: Mutex<Option<Duration>> = Mutex::new(None);
static LAUNCH_PREPARED: Mutex<Option<StudioApp>> = Mutex::new(None);
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
    pub run_timeout: Duration,
}

impl Default for StudioConfig {
    fn default() -> Self {
        Self { size: (1280.0, 800.0), db_path: None, run_timeout: DEFAULT_RUN_TIMEOUT }
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
    spell_record_id: Option<SpellRecordId>,
    project_id: Option<ProjectId>,
    persistent: bool,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct StudioWorkspace {
    pub record: SpellRecordId,
    pub project: ProjectId,
    pub graph: SpellGraph,
    pub releases: Vec<SpellRelease>,
    pub active_release: Option<u32>,
}

impl StudioWorkspace {
    fn of(record: SpellRecord, releases: Vec<SpellRelease>) -> Self {
        let (graph, active_release) = match releases.last() {
            Some(release) if release.created_at > record.updated_at => (release.graph.clone(), Some(release.version)),
            _ => (record.graph, None),
        };
        Self { record: record.id, project: record.project_id, graph, releases, active_release }
    }
}

pub const IN_MEMORY_DATABASE_URL: &str = "sqlite::memory:";

fn escaped_file_path(path: &str) -> String {
    path.chars()
        .map(|character| match character {
            '%' => "%25".to_owned(),
            '?' => "%3F".to_owned(),
            '#' => "%23".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

fn studio_database_url(db_path: Option<&Path>) -> String {
    let text = db_path.map(|path| path.to_string_lossy().into_owned()).unwrap_or_default();
    if matches!(text.trim(), "" | ":memory:") {
        return IN_MEMORY_DATABASE_URL.to_owned();
    }
    if text.starts_with("sqlite:") {
        return text;
    }
    if text.starts_with("file:") {
        return format!("sqlite:{text}");
    }
    format!("sqlite://{}", escaped_file_path(&text))
}

async fn studio_project_record(repository: &SqliteStore, graph: SpellId) -> Result<Option<SpellRecord>, StoreError> {
    let Some(project) = repository.list_projects().await?.into_iter().find(|project| project.name == STUDIO_PROJECT) else { return Ok(None) };
    Ok(repository.list_spells(project.id).await?.into_iter().find(|record| record.graph.id == graph))
}

async fn sorted_releases(repository: &SqliteStore, record: SpellRecordId) -> Result<Vec<SpellRelease>, StoreError> {
    let mut releases = repository.list_releases(record).await?;
    releases.sort_by_key(|release| release.version);
    Ok(releases)
}

async fn scoped_records(repository: &SqliteStore, project: Option<ProjectId>) -> Result<Vec<SpellRecord>, StoreError> {
    if let Some(project) = project {
        return repository.list_spells(project).await;
    }
    let Some(studio) = repository.list_projects().await?.into_iter().find(|project| project.name == STUDIO_PROJECT) else {
        return Ok(Vec::new());
    };
    repository.list_spells(studio.id).await
}

impl StudioStore {
    pub fn open(db_path: Option<&Path>) -> Result<Self, StudioError> {
        let url = studio_database_url(db_path);
        let target = url.clone();
        let (repository, persistent) = block(move || async move {
            let store = SqliteStore::connect(&target).await?;
            store.migrate().await?;
            let persistent = store.is_on_disk().await?;
            Ok::<_, magicklala_store::StoreError>((store, persistent))
        })?
        .map_err(|error| StudioError::Store(error.to_string()))?;
        Ok(Self { repository, spell_record_id: None, project_id: None, persistent, url })
    }

    pub fn project_id(&self) -> Option<ProjectId> {
        self.project_id
    }

    pub fn releases_of(&self, spell: &SpellGraph) -> Result<Vec<SpellRelease>, StudioError> {
        let repository = self.repository.clone();
        let known = self.spell_record_id;
        let graph = spell.id;
        block(move || async move {
            let record = match known {
                Some(id) => Some(id),
                None => studio_project_record(&repository, graph).await?.map(|record| record.id),
            };
            match record {
                Some(id) => sorted_releases(&repository, id).await,
                None => Ok(Vec::new()),
            }
        })?
        .map_err(|error| StudioError::Store(error.to_string()))
    }

    pub fn next_release_version(&self, spell: &SpellGraph) -> Result<u32, StudioError> {
        Ok(self.releases_of(spell)?.iter().map(|release| release.version).max().unwrap_or(0).saturating_add(1))
    }

    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    pub fn spell_record_id(&self) -> Option<SpellRecordId> {
        self.spell_record_id
    }

    pub fn detach_record(&mut self) {
        self.spell_record_id = None;
    }

    fn resolve_spell_record(&mut self, spell: &SpellGraph) -> Result<SpellRecordId, StudioError> {
        let repository = self.repository.clone();
        let spell = spell.clone();
        let record = block(move || async move {
            let project = match repository.list_projects().await?.into_iter().find(|project| project.name == STUDIO_PROJECT) {
                Some(project) => project,
                None => {
                    repository.create_project(NewProject { name: STUDIO_PROJECT.to_owned(), description: "Spells published from Studio".to_owned() }).await?
                }
            };
            let existing = repository.list_spells(project.id).await?.into_iter().find(|record| record.graph.id == spell.id);
            let record = match existing {
                Some(record) => record,
                None => repository.create_spell(NewSpell { project_id: project.id, name: spell.name.clone(), graph: spell.clone() }).await?,
            };
            Ok::<_, StoreError>(record)
        })?
        .map_err(|error| StudioError::Store(error.to_string()))?;
        self.project_id = Some(record.project_id);
        Ok(record.id)
    }

    pub fn publish(&mut self, spell: &SpellGraph) -> Result<SpellRelease, StudioError> {
        let id = match self.spell_record_id {
            Some(id) => id,
            None => self.resolve_spell_record(spell)?,
        };
        let repository = self.repository.clone();
        let graph = spell.clone();
        let release = block(move || async move {
            repository.create_release_with_graph(NewSpellRelease { spell_id: id, notes: PUBLISH_NOTES.to_owned() }, Some(graph)).await
        })?
        .map_err(|error| StudioError::Store(error.to_string()))?;
        self.spell_record_id = Some(release.spell_id);
        Ok(release)
    }

    pub fn save_draft(&mut self, spell: &SpellGraph) -> Result<SpellRecord, StudioError> {
        let id = match self.spell_record_id {
            Some(id) => id,
            None => self.resolve_spell_record(spell)?,
        };
        let repository = self.repository.clone();
        let graph = spell.clone();
        let record = block(move || async move { repository.save_spell_graph(id, graph).await })?.map_err(|error| StudioError::Store(error.to_string()))?;
        self.spell_record_id = Some(record.id);
        self.project_id = Some(record.project_id);
        Ok(record)
    }

    pub fn latest_workspace(&self) -> Result<Option<StudioWorkspace>, StudioError> {
        let repository = self.repository.clone();
        block(move || async move {
            let (studio, others): (Vec<_>, Vec<_>) = repository.list_projects().await?.into_iter().partition(|project| project.name == STUDIO_PROJECT);
            let mut records = Vec::new();
            for project in &studio {
                records.extend(repository.list_spells(project.id).await?);
            }
            if records.is_empty() {
                for project in &others {
                    records.extend(repository.list_spells(project.id).await?);
                }
            }
            let touched = |record: &SpellRecord, releases: &[SpellRelease]| {
                releases.last().map_or(record.updated_at, |release| release.created_at.max(record.updated_at))
            };
            let mut newest: Option<(SpellRecord, Vec<SpellRelease>)> = None;
            for record in records {
                let releases = sorted_releases(&repository, record.id).await?;
                if newest.as_ref().is_none_or(|(current, current_releases)| touched(&record, &releases) > touched(current, current_releases)) {
                    newest = Some((record, releases));
                }
            }
            Ok::<_, StoreError>(newest.map(|(record, releases)| StudioWorkspace::of(record, releases)))
        })?
        .map_err(|error| StudioError::Store(error.to_string()))
    }

    fn adopt_latest_workspace(&mut self) -> Result<Option<StudioWorkspace>, StudioError> {
        let workspace = self.latest_workspace()?;
        if let Some(workspace) = &workspace {
            self.spell_record_id = Some(workspace.record);
            self.project_id = Some(workspace.project);
        }
        Ok(workspace)
    }
}

pub const STUDIO_SUBGRAPH_LOOKUPS: usize = 256;
pub const SUBGRAPH_SPELL_FIELD: &str = "spell_id";
pub const SUBGRAPH_VERSION_FIELD: &str = "release_version";
pub const DRAFT_SAVED: &str = "Draft saved";

#[derive(Debug, Clone, Default)]
pub struct StudioSubgraphResolver {
    graphs: HashMap<SubgraphKey, (SpellId, SpellGraph)>,
}

fn subgraph_keys(graph: &SpellGraph) -> impl Iterator<Item = SubgraphKey> + '_ {
    graph.subgraphs.iter().map(|reference| (reference.spell_id, reference.release_version))
}

async fn released_graph(repository: &SqliteStore, record: &SpellRecord, version: Option<u32>) -> Result<Option<SpellGraph>, magicklala_store::StoreError> {
    Ok(match version {
        None => Some(record.graph.clone()),
        Some(version) => repository.list_releases(record.id).await?.into_iter().find(|release| release.version == version).map(|release| release.graph),
    })
}

impl StudioSubgraphResolver {
    pub fn load(store: Option<&StudioStore>, root: &SpellGraph) -> Result<Self, StudioError> {
        let Some(store) = store.filter(|_| !root.subgraphs.is_empty()) else {
            return Ok(Self::default());
        };
        let repository = store.repository.clone();
        let scope = store.project_id;
        let root = root.clone();
        let graphs = block(move || async move {
            let records = scoped_records(&repository, scope).await?;
            let mut graphs = HashMap::new();
            let mut pending: VecDeque<SubgraphKey> = subgraph_keys(&root).collect();
            while let Some(key) = pending.pop_front() {
                if graphs.contains_key(&key) || graphs.len() >= STUDIO_SUBGRAPH_LOOKUPS {
                    continue;
                }
                let by_record = records.iter().find(|record| record.id.0 == key.0.0);
                let Some(record) = by_record.or_else(|| records.iter().find(|record| record.graph.id == key.0)) else { continue };
                if let Some(graph) = released_graph(&repository, record, key.1).await? {
                    pending.extend(subgraph_keys(&graph));
                    graphs.insert(key, (SpellId(record.id.0), graph));
                }
            }
            Ok::<_, StoreError>(graphs)
        })?
        .map_err(|error| StudioError::Store(error.to_string()))?;
        Ok(Self { graphs })
    }
}

impl SubgraphResolver for StudioSubgraphResolver {
    fn resolve(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<SpellGraph> {
        self.resolve_entry(spell_id, release_version).map(|(_, graph)| graph)
    }

    fn resolve_entry(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<(SpellId, SpellGraph)> {
        self.graphs.get(&(spell_id, release_version)).cloned()
    }
}

pub fn parse_release_version(raw: &str) -> Result<Option<u32>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed.parse::<u32>().ok().filter(|version| *version > 0).map(Some).ok_or_else(|| format!("release version {trimmed:?} must be a positive whole number"))
}

fn whole_release_version(number: &serde_json::Number) -> Option<u32> {
    let whole_float = || number.as_f64().filter(|value| value.is_finite() && value.fract() == 0.0 && *value >= 1.0 && *value <= f64::from(u32::MAX));
    number.as_u64().or_else(|| whole_float().map(|value| value as u64)).filter(|version| *version > 0).and_then(|version| u32::try_from(version).ok())
}

fn configured_release_version(node: &NodeInstance) -> Result<Option<u32>, String> {
    match node.config.get(SUBGRAPH_VERSION_FIELD) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(raw)) => parse_release_version(raw),
        Some(serde_json::Value::Number(number)) => {
            whole_release_version(number).map(Some).ok_or_else(|| format!("release version {number} must be a positive whole number"))
        }
        Some(other) => parse_release_version(&other.to_string()),
    }
}

fn release_version_text(config: &serde_json::Value, kept: Option<&SubgraphRef>) -> String {
    match config.get(SUBGRAPH_VERSION_FIELD) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Null) | None => kept.and_then(|reference| reference.release_version).map(|version| version.to_string()).unwrap_or_default(),
        Some(serde_json::Value::Number(number)) => whole_release_version(number).map_or_else(|| number.to_string(), |version| version.to_string()),
        Some(other) => other.to_string(),
    }
}

fn subgraph_target(node: &NodeInstance, previous: &[SubgraphRef], version_edited: bool) -> Option<SubgraphRef> {
    let kept = previous.iter().find(|reference| reference.node_id == node.id);
    let configured = node.config.get(SUBGRAPH_SPELL_FIELD).and_then(serde_json::Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let spell_id: SpellId = match configured {
        Some(text) => serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()?,
        None => kept?.spell_id,
    };
    let version_from_config = configured.is_some() || version_edited || node.config.get(SUBGRAPH_VERSION_FIELD).is_some();
    let release_version = match (version_from_config, configured_release_version(node)) {
        (false, _) => kept.and_then(|reference| reference.release_version),
        (true, Ok(version)) => version,
        (true, Err(_)) if configured.is_none() => return kept.cloned(),
        (true, Err(_)) => return None,
    };
    Some(SubgraphRef { node_id: node.id.clone(), spell_id, release_version })
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundMessage {
    Trace(TraceEvent),
    Finished(Result<RunSummary, String>),
    TimedOut,
}

#[derive(Clone)]
struct BackgroundChannel {
    sender: Sender<BackgroundMessage>,
    waker: Option<Waker>,
}

impl BackgroundChannel {
    fn deliver(&self, message: BackgroundMessage) {
        if self.sender.send(message).is_ok()
            && let Some(wake) = &self.waker
        {
            wake();
        }
    }
}

impl EventEmitter for BackgroundChannel {
    fn emit(&self, event: TraceEvent) {
        self.deliver(BackgroundMessage::Trace(event));
    }
}

struct ActiveRun {
    token: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
    receiver: Receiver<BackgroundMessage>,
    reply_to_chat: bool,
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

    fn stopped(status: RunStatus, error: String) -> Self {
        Self { outcome: RunOutcome::Finished(status), replies: Vec::new(), error: Some(error) }
    }

    fn chat_reply(&self, number: u32) -> String {
        match (self.replies.is_empty(), self.outcome, &self.error) {
            (false, _, _) => self.replies.join("\n"),
            (true, RunOutcome::Finished(RunStatus::Cancelled), _) => format!("Run #{number} cancelled."),
            (true, RunOutcome::Finished(RunStatus::TimedOut), Some(error)) => format!("Run #{number} stopped: {error}."),
            (true, _, Some(error)) => format!("Run #{number} {}: {error}", self.outcome.label().to_lowercase()),
            (true, _, None) => format!("Run #{number} finished without a reply."),
        }
    }

    fn from_summary(summary: &RunSummary, failure: Option<String>) -> Self {
        let replies = summary.replies.clone();
        let error = summary.error.clone().or(failure).filter(|_| summary.status != RunStatus::Succeeded);
        Self { outcome: RunOutcome::Finished(summary.status), replies, error }
    }
}

fn with_release_version(mut config: serde_json::Value, text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        if let Some(map) = config.as_object_mut() {
            map.remove(SUBGRAPH_VERSION_FIELD);
        }
        return config;
    }
    let value = match parse_release_version(trimmed) {
        Ok(Some(version)) => json!(version),
        _ => serde_json::Value::String(trimmed.to_owned()),
    };
    if !config.is_object() {
        config = json!({});
    }
    if let Some(map) = config.as_object_mut() {
        map.insert(SUBGRAPH_VERSION_FIELD.to_owned(), value);
    }
    config
}

pub fn editable_field(kind: &str) -> Option<&'static str> {
    match kind {
        "text/template" => Some("template"),
        SUBGRAPH_KIND => Some(SUBGRAPH_SPELL_FIELD),
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
    Trace,
    Variables,
    Playground,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexFinding {
    pub node: Option<NodeId>,
    pub socket: Option<String>,
    pub message: String,
    pub readable: String,
}

impl HexFinding {
    fn loose(message: String) -> Self {
        Self { node: None, socket: None, readable: message.clone(), message }
    }

    fn from_validation(error: &ValidationError, spell: &SpellGraph, title_of: &dyn Fn(&NodeId) -> String) -> Self {
        let message = error.to_string();
        let readable = match error {
            ValidationError::MissingRequiredInput { node, socket } => format!("{}: missing required input \"{socket}\"", title_of(node)),
            ValidationError::MultipleValueInputs { node, socket } => format!("{}: input \"{socket}\" has more than one wire", title_of(node)),
            ValidationError::MissingSocket { node, socket, .. } => format!("{}: has no socket \"{socket}\"", title_of(node)),
            ValidationError::UnknownNodeKind { node, kind } => format!("{}: unknown kind {kind}", title_of(node)),
            ValidationError::TypeMismatch { wire, from, to } => match spell.wires.iter().find(|candidate| &candidate.id == wire) {
                Some(found) => format!("{}: input \"{}\" needs {to:?} but gets {from:?}", title_of(&found.to_node), found.to_socket),
                None => message.clone(),
            },
            _ => spell.nodes.iter().fold(message.clone(), |text, node| text.replace(&format!("{:?}", node.id), &title_of(&node.id))),
        };
        let (node, socket) = match error {
            ValidationError::MissingRequiredInput { node, socket }
            | ValidationError::MultipleValueInputs { node, socket }
            | ValidationError::MissingSocket { node, socket, .. } => (Some(node.clone()), Some(socket.clone())),
            ValidationError::TypeMismatch { wire, .. } => match spell.wires.iter().find(|candidate| &candidate.id == wire) {
                Some(found) => (Some(found.to_node.clone()), Some(found.to_socket.clone())),
                None => (None, None),
            },
            ValidationError::UnknownNodeKind { node, .. }
            | ValidationError::ExclusiveJoin { node, .. }
            | ValidationError::SubgraphCycle { node, .. }
            | ValidationError::SubgraphResolverUnavailable { node, .. }
            | ValidationError::OrphanSubgraph { node, .. }
            | ValidationError::SubgraphTargetMissing { node }
            | ValidationError::SubgraphTargetConflict { node, .. }
            | ValidationError::DuplicateSubgraphTarget(node)
            | ValidationError::DuplicateNode(node) => (Some(node.clone()), None),
            ValidationError::ValueCycle(nodes) => (nodes.first().cloned(), None),
            _ => (None, None),
        };
        Self { node, socket, message, readable }
    }
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
    pub node: Option<NodeId>,
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

#[derive(Debug, Clone)]
pub enum UndoEntry {
    Node { card: NodeCardModel, prompt: Option<TextEdit>, version: Option<TextEdit>, config: Option<serde_json::Value>, wires: Vec<WireModel> },
    Variable { index: usize, variable: VariableDef, name_edit: TextEdit, default_edit: TextEdit },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingSwitch {
    NewPotion,
    Sample(String),
}

pub struct StudioApp {
    pub spell: SpellGraph,
    pub hex_findings: Vec<HexFinding>,
    pub canvas: CanvasState,
    pub size: (f32, f32),
    pub specs: Vec<NodeSpec>,
    pub theme: Theme,
    pub left_tab: LeftTab,
    pub right_tab: RightTab,
    pub left_rail_pinned: bool,
    pub trace_expanded: bool,
    pub model_menu_open: bool,
    pub open_var_type: Option<String>,
    pub pending_type_reset: Option<(String, SocketType)>,
    pub pending_restore_version: Option<u32>,
    pub pending_switch: Option<PendingSwitch>,
    pub var_name_edits: Vec<TextEdit>,
    pub var_default_edits: Vec<TextEdit>,
    pub undo_stack: Vec<UndoEntry>,
    pub undo_banner: Option<String>,
    pub dirty: bool,
    pub palette_query: TextEdit,
    pub validation: ValidationState,
    pub trace: Vec<TraceLine>,
    pub trace_events: Vec<TraceEvent>,
    pub pending_trace: VecDeque<TraceEvent>,
    pub runs: u32,
    pub prompts: BTreeMap<NodeId, TextEdit>,
    pub versions: BTreeMap<NodeId, TextEdit>,
    pub chat: Vec<ChatMessage>,
    pub chat_input: TextEdit,
    pub releases: Vec<Release>,
    pub release_graphs: BTreeMap<u32, SpellGraph>,
    pub release_notes: BTreeMap<u32, String>,
    pub configs: BTreeMap<NodeId, serde_json::Value>,
    pub run_payload: serde_json::Value,
    pub last_run: Option<RunReport>,
    pub last_published: Option<SpellRelease>,
    pub store: Option<StudioStore>,
    pub store_error: Option<String>,
    pub open_error: Option<String>,
    pub draft_status: Option<String>,
    pub publish_status: Option<String>,
    pub active_release_version: Option<u32>,
    pub run_timeout: Duration,
    run_started: Option<Instant>,
    active: Option<ActiveRun>,
    stopping: Vec<tokio::task::JoinHandle<()>>,
    waker: Option<Waker>,
}

impl StudioApp {
    pub fn demo() -> Self {
        if let Ok(mut prepared) = LAUNCH_PREPARED.lock()
            && let Some(app) = prepared.take()
        {
            return app;
        }
        let db_path = LAUNCH_DB.lock().ok().and_then(|path| path.clone());
        let run_timeout = LAUNCH_RUN_TIMEOUT.lock().ok().and_then(|timeout| *timeout).unwrap_or(DEFAULT_RUN_TIMEOUT);
        let mut app = Self::launch(&StudioConfig { db_path, run_timeout, ..StudioConfig::default() });
        if LAUNCH_THEME.lock().ok().and_then(|theme| *theme) == Some(Theme::Dark) {
            app.toggle_theme();
        }
        app
    }

    pub fn launch(config: &StudioConfig) -> Self {
        Self::open(config).unwrap_or_else(|error| {
            let mut app = Self::from_registry();
            app.open_error = Some(error.to_string());
            app.store_error = Some(error.to_string());
            app.run_timeout = config.run_timeout;
            app
        })
    }

    pub fn open(config: &StudioConfig) -> Result<Self, StudioError> {
        let mut store = StudioStore::open(config.db_path.as_deref())?;
        let workspace = store.adopt_latest_workspace()?;
        let mut app = match &workspace {
            Some(workspace) => {
                let mut app = Self::with_spell(workspace.graph.clone(), registry_specs());
                app.releases = workspace.releases.iter().map(|release| Release { version: release.version, nodes: release.graph.nodes.len() }).collect();
                app.release_graphs = workspace.releases.iter().map(|release| (release.version, release.graph.clone())).collect();
                app.release_notes =
                    workspace.releases.iter().map(|release| (release.version, release.created_at.format("%b %d · %H:%M").to_string())).collect();
                app.last_published = workspace.releases.last().cloned();
                app
            }
            None => Self::from_registry(),
        };
        app.store = Some(store);
        app.run_timeout = config.run_timeout;
        app.sync_spell();
        app.active_release_version = workspace.and_then(|workspace| workspace.active_release);
        app.canvas.release_badge = app.active_release_version.map(|v| format!("v{v}"));
        app.dirty = false;
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
            theme: Theme::Light,
            left_tab: LeftTab::Palette,
            right_tab: RightTab::Inspector,
            left_rail_pinned: false,
            trace_expanded: false,
            model_menu_open: false,
            open_var_type: None,
            pending_type_reset: None,
            pending_restore_version: None,
            pending_switch: None,
            var_name_edits: Vec::new(),
            var_default_edits: Vec::new(),
            undo_stack: Vec::new(),
            undo_banner: None,
            dirty: false,
            palette_query: TextEdit::new(""),
            validation: ValidationState::Unchecked,
            hex_findings: Vec::new(),
            trace: Vec::new(),
            trace_events: Vec::new(),
            pending_trace: VecDeque::new(),
            runs: 0,
            prompts: BTreeMap::new(),
            versions: BTreeMap::new(),
            chat: vec![ChatMessage { from_user: false, text: "Send a message to run this spell.".to_owned() }],
            chat_input: TextEdit::new(""),
            releases: Vec::new(),
            release_graphs: BTreeMap::new(),
            release_notes: BTreeMap::new(),
            configs: BTreeMap::new(),
            run_payload: serde_json::json!({ "channel": STUDIO_CHANNEL, "sender": STUDIO_SENDER, "content": "Hello from Studio" }),
            last_run: None,
            last_published: None,
            store: None,
            store_error: None,
            open_error: None,
            draft_status: None,
            publish_status: None,
            active_release_version: None,
            run_timeout: DEFAULT_RUN_TIMEOUT,
            run_started: None,
            active: None,
            stopping: Vec::new(),
            waker: None,
        };
        let nodes: Vec<(NodeId, String, serde_json::Value)> =
            app.spell.nodes.iter().map(|node| (node.id.clone(), node.kind.clone(), node.config.clone())).collect();
        for (id, kind, config) in nodes {
            app.track_config(id, &kind, config);
        }
        app.sync_var_edits_from_spell();
        let viewport = app.canvas_size();
        app.canvas.fit(viewport);
        app.canvas.select(app.canvas.nodes.iter().find(|card| card.kind == "ai/generate_text").map(|card| card.id.clone()));
        app
    }

    pub fn left_rail_collapsed(&self) -> bool {
        self.compact_toolbar() && !self.left_rail_pinned
    }

    pub fn left_rail_width(&self) -> f32 {
        if self.left_rail_collapsed() {
            68.0
        } else if self.compact_toolbar() {
            196.0
        } else {
            LEFT_RAIL_WIDTH
        }
    }

    pub fn inspector_width(&self) -> f32 {
        if self.compact_toolbar() { 228.0 } else { INSPECTOR_WIDTH }
    }

    pub fn toggle_left_rail(&mut self) {
        self.left_rail_pinned = !self.left_rail_pinned;
        let viewport = self.canvas_size();
        self.canvas.fit(viewport);
    }

    pub fn hex_mend_source(&self, card_id: &NodeId) -> Option<(SocketRef, String)> {
        let card = self.canvas.card(card_id)?;
        let socket_name = self.missing_socket_of(card_id)?;
        let socket = card.inputs.iter().find(|socket| socket.name == socket_name)?;
        let mut downstream: Vec<NodeId> = vec![card_id.clone()];
        let mut cursor = 0;
        while let Some(node) = downstream.get(cursor).cloned() {
            for wire in self.canvas.wires.iter().filter(|wire| wire.from_node == node) {
                if !downstream.contains(&wire.to_node) {
                    downstream.push(wire.to_node.clone());
                }
            }
            cursor += 1;
        }
        let mut ranked: Vec<&NodeCardModel> = self.canvas.nodes.iter().filter(|other| !downstream.contains(&other.id)).collect();
        ranked.sort_by_key(|other| if other.category == "events" { 0 } else { 1 });
        ranked.into_iter().find_map(|other| {
            other
                .outputs
                .iter()
                .find(|output| output.ty == socket.ty && !output.ty.is_flow())
                .map(|output| (SocketRef::new(other.id.clone(), output.name.clone()), format!("Pour {} · {}", other.title, output.name)))
        })
    }

    fn missing_socket_of(&self, card_id: &NodeId) -> Option<String> {
        self.hex_findings.iter().filter(|finding| finding.node.as_ref() == Some(card_id)).find_map(|finding| finding.socket.clone())
    }

    pub fn mend_selected_hex(&mut self) {
        let Some(card) = self.canvas.selected_card().cloned() else { return };
        let Some(socket) = self.missing_socket_of(&card.id) else { return };
        let Some((source, _)) = self.hex_mend_source(&card.id) else { return };
        if self.canvas.connect_sockets(&source, &SocketRef::new(card.id.clone(), socket)).is_ok() {
            self.sync_spell();
            self.validate();
        }
    }

    pub fn fill_default_prompt_for_selected(&mut self) {
        let Some(card) = self.canvas.selected_card().cloned() else { return };
        let prompt = default_prompt(&card.kind);
        if !prompt.is_empty() {
            self.prompts.insert(card.id.clone(), TextEdit::new(prompt));
        }
        if let Some(card_mut) = self.canvas.card_mut(&card.id) {
            card_mut.status = ExecutionStatus::Idle;
            card_mut.inputs.iter_mut().for_each(|socket| socket.flagged = false);
        }
        self.validate();
    }

    pub fn trace_height(&self) -> f32 {
        if self.trace.is_empty() && !self.is_running() { IDLE_TRACE_HEIGHT } else { TRACE_HEIGHT }
    }

    pub fn canvas_size(&self) -> (f32, f32) {
        ((self.size.0 - self.left_rail_width() - self.inspector_width()).max(200.0), (self.size.1 - TOOLBAR_HEIGHT - self.trace_height()).max(200.0))
    }

    fn format_value(value: Option<&Value>) -> String {
        match value {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Int(n)) => n.to_string(),
            Some(Value::Float(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(other) => serde_json::to_string(other).unwrap_or_default(),
            None => String::new(),
        }
    }

    fn try_parse_value(raw: &str, ty: SocketType) -> Option<Value> {
        let trimmed = raw.trim();
        match ty {
            SocketType::String | SocketType::Any => Some(Value::String(raw.to_owned())),
            SocketType::Int => trimmed.parse::<i64>().ok().map(Value::Int),
            SocketType::Float => trimmed.parse::<f64>().ok().filter(|n| n.is_finite()).map(Value::Float),
            SocketType::Bool => match trimmed.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Some(Value::Bool(true)),
                "false" | "0" | "no" => Some(Value::Bool(false)),
                _ => None,
            },
            _ => Some(Value::String(raw.to_owned())),
        }
    }

    fn zero_value(ty: SocketType) -> Value {
        match ty {
            SocketType::Int => Value::Int(0),
            SocketType::Float => Value::Float(0.0),
            SocketType::Bool => Value::Bool(false),
            _ => Value::String(String::new()),
        }
    }

    fn sync_var_edits_from_spell(&mut self) {
        self.var_name_edits = self.spell.variables.iter().map(|v| TextEdit::new(&v.name)).collect();
        self.var_default_edits = self.spell.variables.iter().map(|v| TextEdit::new(Self::format_value(v.default.as_ref()))).collect();
    }

    pub fn variable_binding_hint(&self, name: &str) -> (String, bool) {
        if name.trim().is_empty() {
            return ("Unused".to_owned(), false);
        }
        let template_token = format!("{{{{{name}}}}}");
        for card in &self.canvas.nodes {
            let in_prompt = self.prompts.get(&card.id).is_some_and(|e| e.text.contains(&template_token));
            let in_config = self.configs.get(&card.id).is_some_and(|cfg| {
                cfg.as_object().is_some_and(|map| map.contains_key(name) || map.values().any(|v| v.as_str().is_some_and(|s| s.contains(&template_token))))
            });
            if in_prompt || in_config {
                return (format!("→ {}", card.title), true);
            }
        }
        ("Unused".to_owned(), false)
    }

    pub fn add_variable(&mut self) {
        let mut index = self.spell.variables.len() + 1;
        let name = loop {
            let candidate = format!("var_{index}");
            if self.spell.variables.iter().all(|v| v.name != candidate) {
                break candidate;
            }
            index += 1;
        };
        self.spell.variables.push(VariableDef { name: name.clone(), ty: SocketType::String, default: Some(Value::String(String::new())), exposed: true });
        self.var_name_edits.push(TextEdit::new(&name));
        self.var_default_edits.push(TextEdit::new(""));
        self.validation = ValidationState::Unchecked;
        self.canvas.validated = false;
        self.dirty = true;
    }

    pub fn remove_variable(&mut self, name: &str) {
        let Some(index) = self.spell.variables.iter().position(|v| v.name == name) else { return };
        let variable = self.spell.variables.remove(index);
        let name_edit = if index < self.var_name_edits.len() { self.var_name_edits.remove(index) } else { TextEdit::new(&variable.name) };
        let default_edit = if index < self.var_default_edits.len() {
            self.var_default_edits.remove(index)
        } else {
            TextEdit::new(Self::format_value(variable.default.as_ref()))
        };
        self.undo_stack.push(UndoEntry::Variable { index, variable, name_edit, default_edit });
        self.undo_banner = Some(format!("Removed spice {name}"));
        if self.open_var_type.as_deref() == Some(name) {
            self.open_var_type = None;
        }
        if self.pending_type_reset.as_ref().is_some_and(|(n, _)| n == name) {
            self.pending_type_reset = None;
        }
        self.validation = ValidationState::Unchecked;
        self.canvas.validated = false;
        self.dirty = true;
        self.sync_spell();
    }

    pub fn toggle_var_type_menu(&mut self, name: &str) {
        self.pending_type_reset = None;
        if self.open_var_type.as_deref() == Some(name) {
            self.open_var_type = None;
        } else {
            self.open_var_type = Some(name.to_owned());
        }
    }

    pub fn set_variable_type(&mut self, name: &str, ty: SocketType) {
        self.open_var_type = None;
        let Some(index) = self.spell.variables.iter().position(|v| v.name == name) else { return };
        if self.spell.variables[index].ty == ty {
            self.pending_type_reset = None;
            return;
        }
        let raw = self.var_default_edits.get(index).map(|e| e.text.clone()).unwrap_or_else(|| Self::format_value(self.spell.variables[index].default.as_ref()));
        if raw.trim().is_empty() {
            self.spell.variables[index].ty = ty;
            self.spell.variables[index].default = Some(Self::zero_value(ty));
            if let Some(edit) = self.var_default_edits.get_mut(index) {
                *edit = TextEdit::new(Self::format_value(self.spell.variables[index].default.as_ref()));
            }
            self.pending_type_reset = None;
            self.dirty = true;
            return;
        }
        if let Some(parsed) = Self::try_parse_value(&raw, ty) {
            self.spell.variables[index].ty = ty;
            self.spell.variables[index].default = Some(parsed);
            if let Some(edit) = self.var_default_edits.get_mut(index) {
                *edit = TextEdit::new(Self::format_value(self.spell.variables[index].default.as_ref()));
            }
            self.pending_type_reset = None;
            self.dirty = true;
        } else {
            self.pending_type_reset = Some((name.to_owned(), ty));
        }
    }

    pub fn confirm_variable_type_reset(&mut self) {
        let Some((name, ty)) = self.pending_type_reset.take() else { return };
        let Some(index) = self.spell.variables.iter().position(|v| v.name == name) else { return };
        self.spell.variables[index].ty = ty;
        let val = Self::zero_value(ty);
        self.spell.variables[index].default = Some(val);
        if let Some(edit) = self.var_default_edits.get_mut(index) {
            *edit = TextEdit::new(Self::format_value(self.spell.variables[index].default.as_ref()));
        }
        self.dirty = true;
    }

    pub fn cancel_variable_type_reset(&mut self) {
        self.pending_type_reset = None;
    }

    pub fn selected_model_name(&self) -> String {
        let Some(id) = self.canvas.selection.as_ref() else { return "gpt-4o-mini".to_owned() };
        self.configs.get(id).and_then(|c| c.get("model")).and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).unwrap_or("gpt-4o-mini").to_owned()
    }

    pub fn toggle_model_menu(&mut self) {
        self.model_menu_open = !self.model_menu_open;
    }

    pub fn select_model(&mut self, provider: &str, model: &str) {
        self.model_menu_open = false;
        let Some(id) = self.canvas.selection.clone() else { return };
        let entry = self.configs.entry(id).or_insert_with(|| json!({}));
        if !entry.is_object() {
            *entry = json!({});
        }
        if let Some(map) = entry.as_object_mut() {
            map.insert("provider".to_owned(), serde_json::Value::String(provider.to_owned()));
            map.insert("model".to_owned(), serde_json::Value::String(model.to_owned()));
        }
        self.sync_spell();
    }

    pub fn load_graph(&mut self, graph: SpellGraph) {
        let mode = self.canvas.mode;
        let light = self.theme == Theme::Light;
        self.spell = graph;
        self.canvas = CanvasState::from_graph(&self.spell, &self.specs);
        self.canvas.mode = mode;
        self.canvas.light = light;
        self.prompts.clear();
        self.versions.clear();
        self.configs.clear();
        self.trace.clear();
        self.trace_events.clear();
        self.pending_trace.clear();
        self.model_menu_open = false;
        self.open_var_type = None;
        self.pending_type_reset = None;
        self.pending_restore_version = None;
        self.pending_switch = None;
        self.undo_stack.clear();
        self.undo_banner = None;
        self.validation = ValidationState::Unchecked;
        let nodes: Vec<(NodeId, String, serde_json::Value)> =
            self.spell.nodes.iter().map(|node| (node.id.clone(), node.kind.clone(), node.config.clone())).collect();
        for (id, kind, config) in nodes {
            self.track_config(id, &kind, config);
        }
        self.sync_var_edits_from_spell();
        let viewport = self.canvas_size();
        self.canvas.fit(viewport);
        self.canvas.select(self.canvas.nodes.iter().find(|card| card.kind == "ai/generate_text").map(|card| card.id.clone()));
    }

    pub fn request_new_spell(&mut self) {
        if self.dirty && !self.canvas.nodes.is_empty() {
            self.pending_switch = Some(PendingSwitch::NewPotion);
        } else {
            self.new_empty_spell();
        }
    }

    pub fn request_open_sample(&mut self, name: &str) {
        if self.spell.name == name {
            return;
        }
        if self.dirty && !self.canvas.nodes.is_empty() {
            self.pending_switch = Some(PendingSwitch::Sample(name.to_owned()));
        } else {
            self.open_sample_spell(name);
        }
    }

    pub fn confirm_switch(&mut self) {
        let Some(target) = self.pending_switch.take() else { return };
        match target {
            PendingSwitch::NewPotion => self.new_empty_spell(),
            PendingSwitch::Sample(name) => self.open_sample_spell(&name),
        }
    }

    pub fn cancel_switch(&mut self) {
        self.pending_switch = None;
    }

    fn detach_spell_history(&mut self) {
        if let Some(store) = self.store.as_mut() {
            store.detach_record();
        }
        self.releases.clear();
        self.release_graphs.clear();
        self.release_notes.clear();
        self.last_published = None;
        self.active_release_version = None;
        self.draft_status = None;
        self.publish_status = None;
        self.store_error = None;
        self.canvas.release_badge = None;
        self.canvas.brew_summary = None;
        self.dirty = false;
    }

    pub fn open_sample_spell(&mut self, name: &str) {
        let graph = if name == "Research Agent" {
            let mut spell = SpellGraph::new("Research Agent");
            let place = |id: &str, kind: &str, pos: [f32; 2], config: serde_json::Value| NodeInstance {
                id: NodeId::from(id),
                kind: kind.to_owned(),
                position: pos,
                config,
            };
            spell.nodes = vec![
                place("trigger", "event/on_message", [0.0, 90.0], serde_json::Value::Null),
                place("search", "knowledge/search", [260.0, 40.0], json!({ "limit": 5 })),
                place("llm", "ai/generate_text", [530.0, 90.0], json!({ "prompt": "Summarize research findings:\n{{knowledge}}\n\nQuestion: {{content}}" })),
                place("reply", "io/send_message", [800.0, 90.0], serde_json::Value::Null),
            ];
            let wire = |id: &str, from: (&str, &str), to: (&str, &str)| Wire {
                id: WireId::from(id),
                from_node: NodeId::from(from.0),
                from_socket: from.1.to_owned(),
                to_node: NodeId::from(to.0),
                to_socket: to.1.to_owned(),
            };
            spell.wires = vec![
                wire("w1", ("trigger", "out"), ("search", "in")),
                wire("w2", ("trigger", "content"), ("search", "query")),
                wire("w3", ("search", "out"), ("llm", "in")),
                wire("w4", ("search", "results"), ("llm", "prompt")),
                wire("w5", ("llm", "out"), ("reply", "in")),
                wire("w6", ("llm", "text"), ("reply", "content")),
            ];
            spell
        } else {
            demo_spell()
        };
        self.detach_spell_history();
        self.load_graph(graph);
    }

    pub fn request_restore_release(&mut self, version: u32) {
        self.pending_restore_version = Some(version);
    }

    pub fn cancel_restore_release(&mut self) {
        self.pending_restore_version = None;
    }

    pub fn confirm_restore_to_draft(&mut self) {
        let Some(version) = self.pending_restore_version.take() else { return };
        let graph = self
            .store
            .as_ref()
            .and_then(|store| store.releases_of(&self.spell).ok())
            .and_then(|list| list.into_iter().find(|r| r.version == version).map(|r| r.graph))
            .or_else(|| self.release_graphs.get(&version).cloned());
        if let Some(graph) = graph {
            self.load_graph(graph);
        }
        self.active_release_version = None;
        self.canvas.release_badge = None;
        self.dirty = true;
        self.draft_status = Some(format!("Loaded v{version} into draft"));
    }

    pub fn selected_temperature(&self) -> f64 {
        if let Some(id) = self.canvas.selection.as_ref()
            && let Some(val) = self.configs.get(id).and_then(|c| c.get("temperature")).and_then(serde_json::Value::as_f64)
        {
            return val;
        }
        self.spell.variables.iter().find(|v| v.name == "temperature").and_then(|v| v.default.as_ref()).map_or(0.4, |v| match v {
            Value::Float(n) => *n,
            Value::Int(n) => *n as f64,
            _ => 0.4,
        })
    }

    pub fn adjust_temperature(&mut self, delta: f64) {
        let current = self.selected_temperature();
        let next = ((current + delta).clamp(0.0, 2.0) * 10.0).round() / 10.0;
        if let Some(id) = self.canvas.selection.clone() {
            let entry = self.configs.entry(id).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            if let Some(map) = entry.as_object_mut() {
                map.insert("temperature".to_owned(), serde_json::json!(next));
            }
        }
        if let Some(index) = self.spell.variables.iter().position(|v| v.name == "temperature") {
            self.spell.variables[index].default = Some(Value::Float(next));
            if let Some(edit) = self.var_default_edits.get_mut(index) {
                *edit = TextEdit::new(next.to_string());
            }
        }
        self.sync_spell();
    }

    pub fn clear_chat(&mut self) {
        self.chat.clear();
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
        if self.canvas.mode == WorkbenchMode::Cauldron && kind != SUBGRAPH_KIND {
            self.auto_wire_cauldron_ingredient(&id, &spec);
        }
        self.validation = ValidationState::Unchecked;
        self.sync_spell();
        Some(id)
    }

    fn auto_wire_cauldron_ingredient(&mut self, id: &NodeId, spec: &NodeSpec) {
        if spec.is_trigger {
            return;
        }
        let has_flow_in = spec.inputs.iter().any(|s| s.ty.is_flow());
        let has_flow_out = spec.outputs.iter().any(|s| s.ty.is_flow());
        if has_flow_in {
            let source = self
                .canvas
                .nodes
                .iter()
                .find(|card| card.id != *id && card.kind == "event/on_message")
                .or_else(|| self.canvas.nodes.iter().find(|card| card.id != *id && card.outputs.iter().any(|s| s.ty.is_flow())))
                .and_then(|card| card.outputs.iter().find(|s| s.ty.is_flow()).map(|s| (card.id.clone(), s.name.clone())));
            if let (Some((from_id, from_sock)), Some(to_sock)) = (source, spec.inputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone())) {
                let _ = self
                    .canvas
                    .connect_sockets(&magicklala_canvas::SocketRef::new(from_id, from_sock), &magicklala_canvas::SocketRef::new(id.clone(), to_sock));
            }
        }
        if has_flow_out && spec.kind != "io/send_message" {
            let target = self
                .canvas
                .nodes
                .iter()
                .find(|card| card.id != *id && (card.kind == "ai/generate_text" || card.kind == "io/send_message"))
                .and_then(|card| card.inputs.iter().find(|s| s.ty.is_flow()).map(|s| (card.id.clone(), s.name.clone())));
            if let (Some(from_sock), Some((to_id, to_sock))) = (spec.outputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone()), target) {
                let _ =
                    self.canvas.connect_sockets(&magicklala_canvas::SocketRef::new(id.clone(), from_sock), &magicklala_canvas::SocketRef::new(to_id, to_sock));
            }
        }
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
        if kind == SUBGRAPH_KIND {
            let kept = self.spell.subgraphs.iter().find(|reference| reference.node_id == id);
            self.versions.insert(id.clone(), TextEdit::new(release_version_text(&config, kept)));
        }
        self.configs.insert(id, config);
    }

    pub fn node_config(&self, id: &NodeId, kind: &str) -> serde_json::Value {
        let config = self.prompted_config(id, kind);
        match self.versions.get(id).filter(|_| kind == SUBGRAPH_KIND) {
            Some(edit) => with_release_version(config, &edit.text),
            None => config,
        }
    }

    fn prompted_config(&self, id: &NodeId, kind: &str) -> serde_json::Value {
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
        self.sync_edits();
    }

    pub fn sync_edits(&mut self) -> bool {
        let mut vars_changed = false;
        for (idx, var) in self.spell.variables.iter_mut().enumerate() {
            if let Some(edit) = self.var_name_edits.get(idx) {
                let next_name = edit.text.trim();
                if !next_name.is_empty() && next_name != var.name {
                    var.name = next_name.to_owned();
                    vars_changed = true;
                }
            }
            if let Some(edit) = self.var_default_edits.get(idx)
                && let Some(parsed) = Self::try_parse_value(&edit.text, var.ty)
                && var.default.as_ref() != Some(&parsed)
            {
                var.default = Some(parsed);
                vars_changed = true;
            }
        }
        let (nodes, wires, subgraphs) = self.rebuilt_parts();
        if !vars_changed && nodes == self.spell.nodes && wires == self.spell.wires && subgraphs == self.spell.subgraphs {
            return false;
        }
        self.spell.nodes = nodes;
        self.spell.wires = wires;
        self.spell.subgraphs = subgraphs;
        self.draft_status = None;
        self.publish_status = None;
        self.store_error = None;
        self.active_release_version = None;
        self.dirty = true;
        self.canvas.spell_name = self.spell.name.clone();
        self.canvas.release_badge = None;
        self.canvas.validated = matches!(self.validation, ValidationState::Valid);
        self.canvas.invalid_brew = matches!(self.validation, ValidationState::Invalid(_));
        self.canvas.running_brew = self.is_running();
        true
    }

    fn rebuilt_parts(&self) -> (Vec<NodeInstance>, Vec<Wire>, Vec<SubgraphRef>) {
        let nodes: Vec<NodeInstance> = self
            .canvas
            .nodes
            .iter()
            .map(|card| NodeInstance { id: card.id.clone(), kind: card.kind.clone(), position: card.position, config: self.node_config(&card.id, &card.kind) })
            .collect();
        let wires = self
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
        let subgraphs = nodes
            .iter()
            .filter(|node| node.kind == SUBGRAPH_KIND)
            .filter_map(|node| subgraph_target(node, &self.spell.subgraphs, self.versions.contains_key(&node.id)))
            .collect();
        (nodes, wires, subgraphs)
    }

    fn root_spell(&self) -> SpellId {
        self.spell_record_id().map_or(self.spell.id, |record| SpellId(record.0))
    }

    pub fn root_subgraph(&self) -> SubgraphKey {
        (self.root_spell(), self.active_release_version)
    }

    pub fn subgraph_resolver(&self) -> Result<StudioSubgraphResolver, StudioError> {
        StudioSubgraphResolver::load(self.store.as_ref(), &self.spell)
    }

    fn release_version_errors(&self) -> Vec<String> {
        self.spell
            .nodes
            .iter()
            .filter(|node| node.kind == SUBGRAPH_KIND)
            .filter_map(|node| configured_release_version(node).err().map(|error| format!("node {:?}: {error}", node.id.0)))
            .collect()
    }

    fn hex_findings_for_spell(&self) -> Vec<HexFinding> {
        let version_findings: Vec<HexFinding> = self
            .spell
            .nodes
            .iter()
            .filter(|node| node.kind == SUBGRAPH_KIND)
            .filter_map(|node| {
                configured_release_version(node).err().map(|error| {
                    let message = format!("node {:?}: {error}", node.id.0);
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: message.clone(), message }
                })
            })
            .collect();
        if !version_findings.is_empty() {
            return version_findings;
        }
        let validator = SpellValidator::new(self.specs.clone());
        if let Err(errors) = validator.validate(&self.spell) {
            let title_of = |id: &NodeId| self.canvas.card(id).map_or_else(|| id.0.clone(), |card| card.title.clone());
            return errors.iter().map(|error| HexFinding::from_validation(error, &self.spell, &title_of)).collect();
        }
        if self.spell.subgraphs.is_empty() {
            return Vec::new();
        }
        let resolver = match self.subgraph_resolver() {
            Ok(resolver) => resolver,
            Err(error) => return vec![HexFinding::loose(error.to_string())],
        };
        match resolve_subgraphs_from(&self.spell, self.root_subgraph(), &resolver, &validator) {
            Ok(_) => Vec::new(),
            Err(error) => vec![HexFinding::loose(error.to_string())],
        }
    }

    fn check_spell_with_root(&self, root: SubgraphKey) -> Result<(), Vec<String>> {
        let version_errors = self.release_version_errors();
        if !version_errors.is_empty() {
            return Err(version_errors);
        }
        let validator = SpellValidator::new(self.specs.clone());
        validator.validate(&self.spell).map_err(|errors| errors.iter().map(ToString::to_string).collect::<Vec<_>>())?;
        if self.spell.subgraphs.is_empty() {
            return Ok(());
        }
        let resolver = self.subgraph_resolver().map_err(|error| vec![error.to_string()])?;
        resolve_subgraphs_from(&self.spell, root, &resolver, &validator).map(drop).map_err(|error| vec![error.to_string()])
    }

    pub fn validate(&mut self) -> &ValidationState {
        self.sync_spell();
        for card in &mut self.canvas.nodes {
            if card.status == ExecutionStatus::Failed {
                card.status = ExecutionStatus::Idle;
            }
            for socket in &mut card.inputs {
                socket.flagged = false;
            }
        }
        let findings = self.hex_findings_for_spell();
        self.hex_findings = findings.clone();
        self.validation = match if findings.is_empty() { Ok(()) } else { Err(findings) } {
            Ok(()) => {
                self.trace.retain(|line| line.kind != "RunRejected");
                if self.canvas.brew_summary.as_deref().is_some_and(|s| s.starts_with("Hexed")) {
                    self.canvas.brew_summary = None;
                }
                ValidationState::Valid
            }
            Err(findings) => {
                let mut rejected: Vec<TraceLine> = Vec::new();
                for finding in &findings {
                    if let Some(card) = finding.node.as_ref().and_then(|id| self.canvas.nodes.iter_mut().find(|card| &card.id == id)) {
                        card.status = ExecutionStatus::Failed;
                        if let Some(socket) = finding.socket.as_ref().and_then(|name| card.inputs.iter_mut().find(|socket| &socket.name == name)) {
                            socket.flagged = true;
                        }
                    }
                    rejected.push(TraceLine {
                        at_ms: 0,
                        kind: "RunRejected",
                        detail: finding.readable.clone(),
                        status: ExecutionStatus::Failed,
                        node: finding.node.clone(),
                    });
                }
                self.canvas.brew_summary = Some(hex_plaque(&findings, &self.canvas.nodes));
                let errors: Vec<String> = findings.into_iter().map(|finding| finding.message).collect();
                if self.trace.is_empty() || self.trace.iter().all(|line| line.kind == "RunRejected") {
                    self.trace = rejected;
                }
                ValidationState::Invalid(errors)
            }
        };
        self.canvas.validated = matches!(self.validation, ValidationState::Valid);
        self.canvas.invalid_brew = matches!(self.validation, ValidationState::Invalid(_));
        &self.validation
    }

    pub fn is_unchanged_from_live_release(&self) -> bool {
        self.active_release_version.is_some()
            && !self.canvas.has_run()
            && !self.dirty
            && self.last_published.as_ref().is_none_or(|release| release.graph == self.spell)
    }

    pub fn run_spell(&mut self) -> bool {
        if self.canvas.nodes.is_empty() || matches!(self.validation, ValidationState::Invalid(_)) {
            return false;
        }
        let payload = self.run_payload.clone();
        self.start_run(None, payload, false)
    }

    fn trigger(&self, prefer: Option<&str>, payload: &serde_json::Value) -> Option<RunTrigger> {
        let spec_of = |node: &NodeInstance| self.specs.iter().find(|spec| spec.kind == node.kind && spec.is_trigger);
        let triggers: Vec<(&NodeInstance, &NodeSpec)> = self.spell.nodes.iter().filter_map(|node| spec_of(node).map(|spec| (node, spec))).collect();
        let (node, spec) = prefer.and_then(|kind| triggers.iter().find(|(node, _)| node.kind == kind)).or_else(|| triggers.first()).copied()?;
        let socket = spec.outputs.first().map_or_else(|| "out".to_owned(), |socket| socket.name.clone());
        Some(RunTrigger { node: node.id.clone(), socket, payload: Value::from(payload.clone()) })
    }

    pub fn is_running(&self) -> bool {
        self.active.is_some()
    }

    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn start_run(&mut self, prefer: Option<&str>, payload: serde_json::Value, reply_to_chat: bool) -> bool {
        if self.is_running() {
            return false;
        }
        self.runs += 1;
        self.trace.clear();
        self.trace_events.clear();
        self.pending_trace.clear();
        self.last_run = None;
        self.run_started = Some(Instant::now());
        self.canvas.nodes.iter_mut().for_each(|card| card.status = ExecutionStatus::Idle);
        self.canvas.wires.iter_mut().for_each(|wire| wire.active = false);
        let trigger = match self.validate().clone() {
            ValidationState::Invalid(errors) => Err(errors.join("; ")),
            _ => self.trigger(prefer, &payload).ok_or_else(|| "spell has no trigger node".to_owned()),
        };
        match trigger.and_then(|trigger| self.spawn_engine(trigger, reply_to_chat)) {
            Ok(active) => {
                self.active = Some(active);
                true
            }
            Err(error) => {
                if self.trace.is_empty() {
                    self.trace.push(TraceLine { at_ms: 0, kind: "RunRejected", detail: error.clone(), status: ExecutionStatus::Failed, node: None });
                }
                self.conclude(RunReport::invalid(error), reply_to_chat);
                false
            }
        }
    }

    fn spawn_engine(&self, trigger: RunTrigger, reply_to_chat: bool) -> Result<ActiveRun, String> {
        let runtime = runtime().map_err(|error| error.to_string())?;
        let (sender, receiver) = mpsc::channel();
        let channel = BackgroundChannel { sender, waker: self.waker.clone() };
        let engine = SpellEngine::new(Arc::new(default_registry()), studio_services()).with_root_subgraph(self.root_subgraph());
        let engine = if self.spell.subgraphs.is_empty() {
            engine
        } else {
            engine.with_subgraph_resolver(Arc::new(self.subgraph_resolver().map_err(|error| error.to_string())?))
        };
        let emitter: Arc<dyn EventEmitter> = Arc::new(channel.clone());
        let graph = self.spell.clone();
        let deadline = self.run_timeout;
        let token = CancellationToken::new();
        let cancellation = token.clone();
        let handle = runtime.spawn(async move {
            let run = engine.run_with_cancellation(&graph, trigger, emitter, cancellation.clone());
            let message = match tokio::time::timeout(deadline, run).await {
                Ok(result) => BackgroundMessage::Finished(result.map_err(|error| error.to_string())),
                Err(_) => {
                    cancellation.cancel();
                    BackgroundMessage::TimedOut
                }
            };
            channel.deliver(message);
        });
        Ok(ActiveRun { token, handle, receiver, reply_to_chat })
    }

    pub fn poll_background(&mut self) -> bool {
        self.stopping.retain(|handle| !handle.is_finished());
        let mut changed = false;
        while let Some(active) = &self.active {
            let message = match active.receiver.try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => BackgroundMessage::Finished(Err("background run stopped without a result".to_owned())),
            };
            self.apply_background(message);
            changed = true;
        }
        changed
    }

    pub fn wait_background(&mut self, timeout: Duration) -> bool {
        self.wait_until(timeout, |app| !app.is_running())
    }

    pub fn wait_until(&mut self, timeout: Duration, done: impl Fn(&Self) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while !done(self) {
            let Some(active) = &self.active else { return false };
            let message = match active.receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => return false,
                Err(RecvTimeoutError::Disconnected) => BackgroundMessage::Finished(Err("background run stopped without a result".to_owned())),
            };
            self.apply_background(message);
        }
        self.canvas.spell_name = self.spell.name.clone();
        self.canvas.release_badge = self.active_release_version.map(|v| format!("v{v}"));
        self.canvas.validated = matches!(self.validation, ValidationState::Valid);
        self.canvas.invalid_brew = matches!(self.validation, ValidationState::Invalid(_));
        self.canvas.running_brew = self.is_running();
        true
    }

    pub fn wait_stopped(&mut self, timeout: Duration) -> bool {
        let handles = std::mem::take(&mut self.stopping);
        block(move || async move {
            tokio::time::timeout(timeout, async move {
                for handle in handles {
                    handle.await.ok();
                }
            })
            .await
            .is_ok()
        })
        .unwrap_or(false)
    }

    pub fn background_tasks(&self) -> usize {
        self.stopping.iter().filter(|handle| !handle.is_finished()).count() + usize::from(self.active.is_some())
    }

    pub fn cancel_run(&mut self) {
        self.poll_background();
        let Some(active) = &self.active else { return };
        active.token.cancel();
        self.close_run("RunCancelled", "cancelled from Studio".to_owned(), RunReport::stopped(RunStatus::Cancelled, "run cancelled".to_owned()));
    }

    fn apply_background(&mut self, message: BackgroundMessage) {
        match message {
            BackgroundMessage::Trace(event) => {
                self.pending_trace.push_back(event);
                while self.step_trace() {}
            }
            BackgroundMessage::Finished(result) => {
                let report = self.report_for(result);
                let reply_to_chat = self.release_active();
                self.conclude(report, reply_to_chat);
            }
            BackgroundMessage::TimedOut => {
                let error = format!("run timed out after {}ms", self.run_timeout.as_millis());
                self.close_run("RunTimedOut", error.clone(), RunReport::stopped(RunStatus::TimedOut, error));
            }
        }
    }

    fn close_run(&mut self, kind: &'static str, detail: String, report: RunReport) {
        let at_ms = self.elapsed_ms();
        self.trace.push(TraceLine { at_ms, kind, detail, status: ExecutionStatus::Failed, node: None });
        self.canvas.nodes.iter_mut().filter(|card| card.status == ExecutionStatus::Running).for_each(|card| card.status = ExecutionStatus::Failed);
        let reply_to_chat = self.release_active();
        self.conclude(report, reply_to_chat);
    }

    fn release_active(&mut self) -> bool {
        let Some(active) = self.active.take() else { return false };
        if !active.handle.is_finished() {
            self.stopping.push(active.handle);
        }
        active.reply_to_chat
    }

    fn conclude(&mut self, report: RunReport, reply_to_chat: bool) {
        if reply_to_chat {
            self.chat.push(ChatMessage { from_user: false, text: report.chat_reply(self.runs) });
        }
        self.last_run = Some(report);
    }

    fn report_for(&self, result: Result<RunSummary, String>) -> RunReport {
        let finished = self.trace_events.iter().rev().find_map(|event| match event {
            TraceEvent::RunFinished { summary, .. } => Some(summary.clone()),
            _ => None,
        });
        match (result, finished) {
            (Ok(summary), _) => RunReport::from_summary(&summary, None),
            (Err(error), Some(summary)) => RunReport::from_summary(&summary, Some(error)),
            (Err(error), None) => RunReport::invalid(error),
        }
    }

    fn elapsed_ms(&self) -> u64 {
        self.run_started.map_or(0, |started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    pub fn step_trace(&mut self) -> bool {
        let Some(event) = self.pending_trace.pop_front() else { return false };
        self.canvas.apply_trace_event(&event);
        let at_ms = self.elapsed_ms();
        self.trace.push(trace_line(&event, at_ms, &self.canvas));
        self.trace_events.push(event);
        self.canvas.spell_name = self.spell.name.clone();
        self.canvas.release_badge = self.active_release_version.map(|v| format!("v{v}"));
        self.canvas.validated = matches!(self.validation, ValidationState::Valid);
        self.canvas.invalid_brew = matches!(self.validation, ValidationState::Invalid(_));
        self.canvas.running_brew = self.is_running();
        true
    }

    pub fn close_context_menu(&mut self) {
        self.canvas.context_menu = None;
    }

    pub fn open_grimoire(&mut self) {
        self.canvas.mode = WorkbenchMode::Grimoire;
        self.close_context_menu();
        let viewport = self.canvas_size();
        self.canvas.fit(viewport);
    }

    pub fn new_empty_spell(&mut self) {
        self.detach_spell_history();
        self.spell = SpellGraph::new("New Potion");
        self.canvas = CanvasState::from_graph(&self.spell, &self.specs);
        self.canvas.light = self.theme == Theme::Light;
        self.prompts.clear();
        self.versions.clear();
        self.configs.clear();
        self.trace.clear();
        self.trace_events.clear();
        self.pending_trace.clear();
        self.open_var_type = None;
        self.pending_type_reset = None;
        self.pending_restore_version = None;
        self.pending_switch = None;
        self.undo_stack.clear();
        self.undo_banner = None;
        self.validation = ValidationState::Unchecked;
        self.sync_var_edits_from_spell();
    }

    pub fn quick_add_from_menu(&mut self) {
        let anchor = self.canvas.context_menu.unwrap_or([self.canvas_size().0 * 0.5, self.canvas_size().1 * 0.5]);
        let selected_clone = self.canvas.selected_card().filter(|card| card.kind == QUICK_ADD_KIND).map(|card| {
            (self.configs.get(&card.id).cloned(), self.prompts.get(&card.id).map(|e| e.text.clone()), self.versions.get(&card.id).map(|e| e.text.clone()))
        });
        self.close_context_menu();
        if let Some(new_id) = self.spawn_at(QUICK_ADD_KIND, anchor)
            && let Some((cfg, prompt, version)) = selected_clone
        {
            if let Some(cfg) = cfg {
                self.configs.insert(new_id.clone(), cfg);
            }
            if let Some(prompt) = prompt {
                self.prompts.insert(new_id.clone(), TextEdit::new(prompt));
            }
            if let Some(version) = version {
                self.versions.insert(new_id.clone(), TextEdit::new(version));
            }
            self.sync_spell();
        }
    }

    pub fn delete_selected_node(&mut self) {
        self.close_context_menu();
        let Some(selected_id) = self.canvas.selection.clone() else { return };
        let attached_wires: Vec<WireModel> = self.canvas.wires.iter().filter(|w| w.from_node == selected_id || w.to_node == selected_id).cloned().collect();
        if let Some(removed) = self.canvas.remove_selected_node() {
            let prompt = self.prompts.remove(&removed.id);
            let version = self.versions.remove(&removed.id);
            let config = self.configs.remove(&removed.id);
            let title = removed.title.clone();
            self.undo_stack.push(UndoEntry::Node { card: removed, prompt, version, config, wires: attached_wires });
            self.undo_banner = Some(format!("Removed {title}"));
            self.validation = ValidationState::Unchecked;
            self.dirty = true;
            self.sync_spell();
        }
    }

    pub fn undo(&mut self) {
        let Some(entry) = self.undo_stack.pop() else { return };
        self.undo_banner = None;
        match entry {
            UndoEntry::Node { card, prompt, version, config, wires } => {
                let id = card.id.clone();
                if let Some(prompt) = prompt {
                    self.prompts.insert(id.clone(), prompt);
                }
                if let Some(version) = version {
                    self.versions.insert(id.clone(), version);
                }
                if let Some(config) = config {
                    self.configs.insert(id.clone(), config);
                }
                self.canvas.restore_node(card, wires);
                self.validation = ValidationState::Unchecked;
                self.dirty = true;
                self.sync_spell();
            }
            UndoEntry::Variable { index, variable, name_edit, default_edit } => {
                let at = index.min(self.spell.variables.len());
                self.spell.variables.insert(at, variable);
                let name_at = at.min(self.var_name_edits.len());
                self.var_name_edits.insert(name_at, name_edit);
                let def_at = at.min(self.var_default_edits.len());
                self.var_default_edits.insert(def_at, default_edit);
                self.validation = ValidationState::Unchecked;
                self.canvas.validated = false;
                self.dirty = true;
                self.sync_spell();
            }
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

    fn refuse(&mut self, message: String) -> StudioError {
        self.store_error = Some(message.clone());
        StudioError::Store(message)
    }

    fn require_disk(&mut self, action: &str) -> Result<(), StudioError> {
        if let Some(error) = &self.open_error {
            let message = format!("the database passed with --db could not be opened: {error}");
            return Err(self.refuse(message));
        }
        if self.store.as_ref().is_some_and(StudioStore::is_persistent) {
            return Ok(());
        }
        Err(self.refuse(format!("pass --db <file.sqlite> to {action} on disk")))
    }

    pub fn next_release_version(&self) -> Result<u32, StudioError> {
        match &self.store {
            Some(store) => store.next_release_version(&self.spell),
            None => Ok(self.releases.iter().map(|release| release.version).max().unwrap_or(0).saturating_add(1)),
        }
    }

    fn refresh_releases(&mut self, published: &SpellRelease) {
        let stored = self.store.as_ref().map(|store| store.releases_of(&self.spell));
        match stored {
            Some(Ok(releases)) => {
                self.release_graphs = releases.iter().map(|release| (release.version, release.graph.clone())).collect();
                self.release_notes = releases.iter().map(|release| (release.version, release.created_at.format("%b %d · %H:%M").to_string())).collect();
                self.releases = releases.iter().map(|release| Release { version: release.version, nodes: release.graph.nodes.len() }).collect();
            }
            _ => {
                self.release_graphs.insert(published.version, published.graph.clone());
                self.release_notes.insert(published.version, published.created_at.format("%b %d · %H:%M").to_string());
                let mut releases = std::mem::take(&mut self.releases);
                releases.push(Release { version: published.version, nodes: published.graph.nodes.len() });
                self.releases = releases;
            }
        }
    }

    pub fn explain_publish_blocked(&mut self) {
        if let ValidationState::Invalid(errors) = &self.validation {
            let message = format!("cannot publish an invalid spell: {}", errors.join("; "));
            self.store_error = Some(message);
        }
    }

    pub fn publish(&mut self) -> Result<u32, StudioError> {
        self.sync_spell();
        if self.canvas.nodes.is_empty() {
            return Err(self.refuse("cannot publish an empty potion".to_owned()));
        }
        if self.is_unchanged_from_live_release() {
            return Err(self.refuse("no changes since the live release".to_owned()));
        }
        self.publish_status = None;
        self.require_disk("publish releases")?;
        let next = match self.next_release_version() {
            Ok(next) => next,
            Err(error) => return Err(self.refuse(error.to_string())),
        };
        let target = (self.root_spell(), Some(next));
        if let Err(errors) = self.check_spell_with_root(target) {
            let message = format!("cannot publish an invalid spell: {}", errors.join("; "));
            self.validation = ValidationState::Invalid(errors);
            return Err(self.refuse(message));
        }
        self.validation = ValidationState::Valid;
        let store = self.store.as_mut().ok_or_else(|| StudioError::Store("no studio store".to_owned()))?;
        let published = store.publish(&self.spell);
        let release = published.inspect_err(|error| self.store_error = Some(error.to_string()))?;
        self.refresh_releases(&release);
        self.store_error = None;
        self.draft_status = None;
        self.dirty = false;
        self.active_release_version = Some(release.version);
        let version = release.version;
        let ingr_count = self.canvas.nodes.iter().filter(|card| card.kind != "io/send_message").count();
        let word = if ingr_count == 1 { "ingredient" } else { "ingredients" };
        self.canvas.release_badge = Some(format!("v{version}"));
        self.canvas.brew_summary = Some(format!("Published v{version} · {ingr_count} {word}"));
        self.publish_status = Some(format!("Published v{version}"));
        self.last_published = Some(release);
        Ok(version)
    }

    pub fn save_draft(&mut self) -> Result<SpellRecordId, StudioError> {
        self.sync_spell();
        self.draft_status = None;
        self.publish_status = None;
        self.require_disk("save drafts")?;
        let version_errors = self.release_version_errors();
        if !version_errors.is_empty() {
            return Err(self.refuse(format!("cannot save the draft: {}", version_errors.join("; "))));
        }
        let store = self.store.as_mut().ok_or_else(|| StudioError::Store("no studio store".to_owned()))?;
        let saved = store.save_draft(&self.spell);
        let record = saved.inspect_err(|error| self.store_error = Some(error.to_string()))?;
        self.store_error = None;
        self.dirty = false;
        self.active_release_version = None;
        self.canvas.release_badge = None;
        self.draft_status = Some(DRAFT_SAVED.to_owned());
        Ok(record.id)
    }

    pub fn spell_record_id(&self) -> Option<SpellRecordId> {
        self.store.as_ref().and_then(StudioStore::spell_record_id)
    }

    pub fn send_chat(&mut self) {
        let message = self.chat_input.text.trim().to_owned();
        if message.is_empty() || self.is_running() {
            return;
        }
        self.chat.push(ChatMessage { from_user: true, text: message.clone() });
        self.chat_input = TextEdit::new("");
        self.start_run(Some(CHAT_TRIGGER_KIND), json!({ "channel": STUDIO_CHANNEL, "sender": STUDIO_SENDER, "content": message }), true);
    }
}

impl Drop for StudioApp {
    fn drop(&mut self) {
        if let Some(active) = &self.active {
            active.token.cancel();
        }
    }
}

pub fn run_studio(config: StudioConfig) -> Result<(), StudioError> {
    remember_launch(&config, Theme::Light);
    App::new(spec(config)).args(Vec::new()).try_run().map_err(StudioError::Rune)
}

pub fn render_headless_snapshot(out_path: &Path) -> Result<(), StudioError> {
    render_headless_snapshot_sized(out_path, StudioConfig::default().size)
}

pub fn render_headless_snapshot_sized(out_path: &Path, size: (f32, f32)) -> Result<(), StudioError> {
    render_headless_snapshot_themed(out_path, size, Theme::Light)
}

pub fn render_headless_snapshot_themed(out_path: &Path, size: (f32, f32), theme: Theme) -> Result<(), StudioError> {
    render_headless_snapshot_with_config(out_path, StudioConfig { size, ..StudioConfig::default() }, theme)
}

pub fn render_headless_snapshot_with_config(out_path: &Path, config: StudioConfig, theme: Theme) -> Result<(), StudioError> {
    let target = out_path.display().to_string();
    let (width, height) = config.size;
    remember_launch(&config, theme);
    let args =
        vec!["--snap".to_owned(), target.clone(), "--size".to_owned(), format!("{}x{}", width as u32, height as u32), "--scale".to_owned(), "1".to_owned()];
    App::new(spec(config)).args(args).try_run().map_err(StudioError::Rune)?;
    if out_path.is_file() { Ok(()) } else { Err(StudioError::MissingSnapshot(target)) }
}

pub fn render_headless_snapshot_state(out_path: &Path, app: StudioApp) -> Result<(), StudioError> {
    let size = app.size;
    let theme = app.theme;
    let config = StudioConfig { size, ..StudioConfig::default() };
    remember_launch(&config, theme);
    if let Ok(mut slot) = LAUNCH_PREPARED.lock() {
        *slot = Some(app);
    }
    let target = out_path.display().to_string();
    let args =
        vec!["--snap".to_owned(), target.clone(), "--size".to_owned(), format!("{}x{}", size.0 as u32, size.1 as u32), "--scale".to_owned(), "1".to_owned()];
    let result = App::new(spec(config)).args(args).try_run().map_err(StudioError::Rune);
    if let Ok(mut slot) = LAUNCH_PREPARED.lock() {
        *slot = None;
    }
    result?;
    if out_path.is_file() { Ok(()) } else { Err(StudioError::MissingSnapshot(target)) }
}

fn remember_launch(config: &StudioConfig, theme: Theme) {
    remember_size(config.size);
    if let Ok(mut slot) = LAUNCH_DB.lock() {
        slot.clone_from(&config.db_path);
    }
    if let Ok(mut slot) = LAUNCH_PREPARED.lock() {
        *slot = None;
    }
    remember_theme(theme);
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
    remember_run_timeout(config.run_timeout);
    AppSpec {
        fonts: STUDIO_FONTS,
        families: STUDIO_FAMILIES,
        setup: Some(StudioApp::set_waker),
        poll: Some(|app, _, _| app.poll_background() | app.sync_edits()),
        ..AppSpec::new(TITLE, config.size, StudioApp::demo, studio_view)
    }
}

fn remember_run_timeout(timeout: Duration) {
    if let Ok(mut slot) = LAUNCH_RUN_TIMEOUT.lock() {
        *slot = Some(timeout);
    }
}

mod view;
pub use view::{prompt_editor_height, prompt_font, studio_status, studio_view};

fn trace_line(event: &TraceEvent, at_ms: u64, canvas: &CanvasState) -> TraceLine {
    let path = event.subgraph_path();
    let along_path = |leaf: &str| path.iter().map(|step| step.0.as_str()).chain(std::iter::once(leaf)).collect::<Vec<_>>().join(" › ");
    let title = |id: &NodeId| {
        if !path.is_empty() {
            return along_path(&id.0);
        }
        let Some(card) = canvas.card(id) else {
            return id.0.clone();
        };
        let dup = canvas.nodes.iter().filter(|other| other.title == card.title).count() > 1;
        if dup {
            let branch = id.0.strip_suffix("_reply").unwrap_or(&id.0);
            let head = card.title.split_whitespace().next().unwrap_or(&card.title);
            format!("{head} · {branch}")
        } else {
            card.title.clone()
        }
    };
    let wire_label = |wire_id: &WireId| {
        if !path.is_empty() {
            return along_path(&wire_id.0);
        }
        canvas
            .wires
            .iter()
            .find(|wire| &wire.id == wire_id)
            .map_or_else(|| wire_id.0.clone(), |wire| format!("{}.{} → {}.{}", title(&wire.from_node), wire.from_socket, title(&wire.to_node), wire.to_socket))
    };
    let (kind, detail, status, node) = match event {
        TraceEvent::RunStarted { .. } => ("RunStarted", String::new(), ExecutionStatus::Running, None),
        TraceEvent::NodeStarted { node_id, .. } => ("NodeStarted", title(node_id), ExecutionStatus::Running, Some(node_id.clone())),
        TraceEvent::NodeFinished { node_id, duration_ms, .. } => {
            let timing = if *duration_ms == 0 { "<1ms".to_owned() } else { format!("{duration_ms}ms") };
            ("NodeFinished", format!("{} · {timing}", title(node_id)), ExecutionStatus::Succeeded, Some(node_id.clone()))
        }
        TraceEvent::NodeFailed { node_id, error, .. } => ("NodeFailed", format!("{}: {error}", title(node_id)), ExecutionStatus::Failed, Some(node_id.clone())),
        TraceEvent::WireActivated { wire_id, .. } => ("WireActivated", wire_label(wire_id), ExecutionStatus::Succeeded, None),
        TraceEvent::StreamChunk { node_id, .. } => ("StreamChunk", title(node_id), ExecutionStatus::Running, Some(node_id.clone())),
        TraceEvent::VariableSet { name, .. } => ("VariableSet", name.clone(), ExecutionStatus::Succeeded, None),
        TraceEvent::RunFinished { summary, .. } => (
            "RunFinished",
            format!("{:?}", summary.status),
            if summary.status == RunStatus::Succeeded { ExecutionStatus::Succeeded } else { ExecutionStatus::Failed },
            None,
        ),
    };
    TraceLine { at_ms, kind, detail, status, node }
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

fn hex_plaque(findings: &[HexFinding], cards: &[NodeCardModel]) -> String {
    let title_of = |id: &NodeId| cards.iter().find(|card| &card.id == id).map(|card| card.title.clone());
    match findings.first() {
        Some(HexFinding { socket: Some(socket), .. }) => format!("Hexed · missing \"{socket}\""),
        Some(HexFinding { node: Some(node), .. }) => format!("Hexed · {}", title_of(node).unwrap_or_else(|| node.0.clone())),
        _ => "Hexed".to_owned(),
    }
}

fn demo_spell() -> SpellGraph {
    let mut spell = SpellGraph::new("Support Agent");
    let place =
        |id: &str, kind: &str, position: [f32; 2], config: serde_json::Value| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position, config };
    spell.nodes = vec![
        place("trigger", "event/on_message", [0.0, 108.0], serde_json::Value::Null),
        place("recall", "memory/search", [228.0, 12.0], json!({ "max_memories": 5 })),
        place("search", "knowledge/search", [228.0, 208.0], serde_json::Value::Null),
        place(
            "llm",
            "ai/generate_text",
            [468.0, 86.0],
            json!({ "provider": "openai", "model": "gpt-4o-mini", "temperature": 0.4, "prompt": default_prompt("ai/generate_text") }),
        ),
        place("reply", "io/send_message", [692.0, 128.0], json!({ "agent_name": "Support Agent" })),
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
    vec![
        VariableDef { name: "agent_name".to_owned(), ty: SocketType::String, default: Some(Value::String("Support Agent".to_owned())), exposed: true },
        VariableDef { name: "max_memories".to_owned(), ty: SocketType::Int, default: Some(Value::Int(5)), exposed: true },
        VariableDef { name: "temperature".to_owned(), ty: SocketType::Float, default: Some(Value::Float(0.4)), exposed: true },
    ]
}
