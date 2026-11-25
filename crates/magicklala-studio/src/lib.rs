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
pub const OPEN_TRACE_HEIGHT: f32 = 196.0;
pub const PALETTE_CATEGORY_ORDER: [&str; 13] =
    ["events", "flow", "ai", "memory", "knowledge", "io", "code", "variables", "text", "logic", "json", "network", "time"];
pub const LEGACY_KIND_PREFIX: &str = "core/";
pub const CONTEXT_MENU_SIZE: (f32, f32) = (200.0, 197.0);
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

fn stored_wires(prefix: &str, wires: &[(SocketRef, SocketRef)]) -> Vec<Wire> {
    wires
        .iter()
        .map(|(from, to)| Wire {
            id: WireId::from(format!("{prefix}-{}-{}-{}-{}", from.node.0, from.socket, to.node.0, to.socket).as_str()),
            from_node: from.node.clone(),
            from_socket: from.socket.clone(),
            to_node: to.node.clone(),
            to_socket: to.socket.clone(),
        })
        .collect()
}

fn socket_pairs(wires: &[Wire]) -> Vec<(SocketRef, SocketRef)> {
    wires
        .iter()
        .map(|wire| (SocketRef::new(wire.from_node.clone(), wire.from_socket.clone()), SocketRef::new(wire.to_node.clone(), wire.to_socket.clone())))
        .collect()
}

/// Whether a Set step for `name` runs before `node`: some step upstream of it, through any wire, sets that variable.
fn set_earlier_on_route(graph: &SpellGraph, node: &NodeId, name: &str) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut queue = vec![node.clone()];
    while let Some(current) = queue.pop() {
        for wire in graph.wires.iter().filter(|wire| wire.to_node == current) {
            if !seen.insert(wire.from_node.clone()) {
                continue;
            }
            if graph
                .node(&wire.from_node)
                .is_some_and(|step| step.kind == "var/set" && step.config.get("name").and_then(|value| value.as_str()).map(str::trim) == Some(name))
            {
                return true;
            }
            queue.push(wire.from_node.clone());
        }
    }
    false
}

fn same_recipe(a: &SpellGraph, b: &SpellGraph) -> bool {
    let steps = |graph: &SpellGraph| {
        let mut steps: Vec<(String, String, String)> = graph.nodes.iter().map(|node| (node.id.0.clone(), node.kind.clone(), node.config.to_string())).collect();
        steps.sort();
        steps
    };
    let doors = |graph: &SpellGraph| -> Vec<String> {
        graph.nodes.iter().filter(|node| node.kind.starts_with("event/") || node.kind.starts_with("core/on_")).map(|node| node.id.0.clone()).collect()
    };
    let links = |graph: &SpellGraph| {
        let mut links: Vec<(String, String, String, String)> =
            graph.wires.iter().map(|wire| (wire.from_node.0.clone(), wire.from_socket.clone(), wire.to_node.0.clone(), wire.to_socket.clone())).collect();
        links.sort();
        links
    };
    a.name == b.name
        && a.description == b.description
        && a.variables == b.variables
        && a.subgraphs == b.subgraphs
        && steps(a) == steps(b)
        && doors(a) == doors(b)
        && links(a) == links(b)
}

fn calls_only_pinned_spells(graph: &SpellGraph) -> bool {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == SUBGRAPH_KIND)
        .all(|node| graph.subgraphs.iter().any(|reference| reference.node_id == node.id && reference.release_version.is_some()))
}

fn recipe_from(spell: &SpellGraph, canvas: &CanvasState, specs: &[NodeSpec]) -> SpellGraph {
    let mut graph = spell.clone();
    graph.shelf.clear();
    let recipe = magicklala_canvas::recipe_nodes(canvas);
    let known = |kind: &str| specs.iter().any(|spec| spec.kind == kind);
    let open_trigger = |node: &NodeInstance| !canvas.is_shelved(&node.id) && specs.iter().any(|spec| spec.kind == node.kind && spec.is_trigger);
    graph.nodes.retain(|node| recipe.contains(&node.id) || open_trigger(node) || !known(&node.kind));
    let kept: Vec<NodeId> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    graph.wires.retain(|wire| kept.contains(&wire.from_node) && kept.contains(&wire.to_node));
    graph.subgraphs.retain(|reference| kept.contains(&reference.node_id));
    graph
}

fn recipe_of(spell: &SpellGraph, specs: &[NodeSpec]) -> SpellGraph {
    let mut canvas = CanvasState::from_graph(spell, specs);
    for entry in &spell.shelf {
        canvas.shelve(&entry.node);
    }
    recipe_from(spell, &canvas, specs)
}

impl StudioWorkspace {
    fn of(record: SpellRecord, releases: Vec<SpellRelease>) -> Self {
        let recipe = recipe_of(&record.graph, &registry_specs());
        let (graph, active_release) = match releases.last() {
            Some(release) if same_recipe(&release.graph, &recipe) => (record.graph, Some(release.version)),
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

    pub fn attach_record(&mut self, id: SpellRecordId) {
        self.spell_record_id = Some(id);
    }

    pub fn saved_spells(&self) -> Vec<SpellRecord> {
        let repository = self.repository.clone();
        block(move || async move {
            let Some(project) = repository.list_projects().await?.into_iter().find(|project| project.name == STUDIO_PROJECT) else { return Ok(Vec::new()) };
            repository.list_spells(project.id).await
        })
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
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

    pub fn releases_of_record(&self, record: SpellRecordId) -> Vec<SpellRelease> {
        let repository = self.repository.clone();
        block(move || async move { sorted_releases(&repository, record).await }).ok().and_then(Result::ok).unwrap_or_default()
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
        None => Some(recipe_of(&record.graph, &registry_specs())),
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
        "var/get" | "var/set" | "var/reset" => Some("name"),
        "flow/switch" => Some("cases"),
        "flow/split" => Some("outputs"),
        "json/destructure" => Some("keys"),
        "ai/embedding" => Some("text"),
        kind if kind.starts_with("code/") => Some("script"),
        kind if kind.starts_with("ai/") => Some("prompt"),
        _ => None,
    }
}

fn card_note(kind: &str, config: &serde_json::Value, spec: Option<&NodeSpec>, variables: &[VariableDef]) -> Option<String> {
    let read = |key: &str| config.get(key).or_else(|| spec?.config.iter().find(|field| field.key == key)?.default.as_ref());
    let bound = |name: &str| {
        let value = variables.iter().find(|variable| variable.name == name).and_then(|variable| variable.default.as_ref()).map(|value| value.to_json());
        match value {
            Some(serde_json::Value::String(text)) => format!("{text} · {name}"),
            Some(value) if !value.is_null() => format!("{value} · {name}"),
            _ => name.to_owned(),
        }
    };
    let setting = |key: &str| {
        read(key).and_then(|value| match value {
            serde_json::Value::String(text) => Some(magicklala_graph::bound_variable(value).map_or_else(|| text.clone(), bound)),
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
    };
    let count = |key: &str, one: &str, many: &str| {
        read(key).and_then(serde_json::Value::as_array).filter(|items| !items.is_empty()).map(|items| crate::view::plural(items.len(), one, many))
    };
    match kind {
        "ai/generate_text" | "ai/stream_text" | "ai/structured_json" => setting("model").filter(|model| !model.is_empty()),
        "memory/search" | "knowledge/search" => Some(format!("top {}", setting("limit").unwrap_or_else(|| "5".to_owned()))),
        "flow/switch" => count("cases", "case", "cases"),
        "flow/split" => count("outputs", "output", "outputs"),
        "json/destructure" => count("keys", "key", "keys"),
        "time/delay" => setting("ms").map(|ms| if ms.chars().all(|c| c.is_ascii_digit()) { format!("{ms} ms") } else { format!("wait {ms}") }),
        "var/get" | "var/set" | "var/reset" => setting("name").filter(|name| !name.is_empty()),
        _ => None,
    }
}

pub fn listed_field(kind: &str) -> bool {
    matches!(kind, "flow/switch" | "flow/split" | "json/destructure")
}

pub fn listed_names(text: &str) -> Vec<String> {
    let names: Vec<String> = serde_json::from_str::<Vec<String>>(text).unwrap_or_else(|_| split_listed(text));
    names.into_iter().map(|name| name.trim().to_owned()).filter(|name| !name.is_empty()).fold(Vec::new(), |mut kept, name| {
        if !kept.contains(&name) {
            kept.push(name);
        }
        kept
    })
}

fn split_listed(text: &str) -> Vec<String> {
    let mut names = vec![String::new()];
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if quoted && chars.peek().is_some() => names.last_mut().into_iter().for_each(|name| name.extend(chars.next())),
            '"' => quoted = !quoted,
            ',' | '\n' if !quoted => names.push(String::new()),
            _ => names.last_mut().into_iter().for_each(|name| name.push(ch)),
        }
    }
    names
}

pub fn fixed_outputs(spec: &NodeSpec) -> Vec<String> {
    let listed_by_default: Vec<&str> =
        spec.config.iter().filter_map(|field| field.default.as_ref()?.as_array()).flatten().filter_map(serde_json::Value::as_str).collect();
    spec.outputs
        .iter()
        .filter(|socket| socket.name != magicklala_graph::DYNAMIC_SOCKET && !listed_by_default.contains(&socket.name.as_str()))
        .map(|socket| socket.name.clone())
        .collect()
}

pub fn listed_text(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| {
            if name.contains([',', '"', '\n']) || name.trim() != *name { serde_json::Value::String((*name).to_owned()).to_string() } else { (*name).to_owned() }
        })
        .collect::<Vec<_>>()
        .join(", ")
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
    pub mistyped: bool,
}

impl HexFinding {
    fn loose(message: String) -> Self {
        Self { node: None, socket: None, readable: message.clone(), message, mistyped: false }
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
        Self { node, socket, message, readable, mistyped: matches!(error, ValidationError::TypeMismatch { .. }) }
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
    pub path: Vec<NodeId>,
}

impl TraceLine {
    pub fn canvas_node(&self) -> Option<NodeId> {
        self.path.first().cloned().or_else(|| self.node.clone())
    }
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
pub struct ParkedShelf {
    pub parked: Vec<(SocketRef, SocketRef)>,
    pub bridges: Vec<(SocketRef, SocketRef)>,
    pub refused: Vec<(SocketRef, SocketRef)>,
}

#[derive(Debug, Clone)]
pub enum UndoEntry {
    Aside {
        id: NodeId,
        wires: Vec<(SocketRef, SocketRef)>,
        bridges: Vec<(SocketRef, SocketRef)>,
    },
    Node {
        at: usize,
        card: Box<NodeCardModel>,
        prompt: Option<TextEdit>,
        version: Option<TextEdit>,
        config: Option<serde_json::Value>,
        wires: Vec<WireModel>,
        bridges: Vec<(SocketRef, SocketRef)>,
        shelf: Option<ParkedShelf>,
        call: Option<SubgraphRef>,
    },
    Variable {
        index: usize,
        variable: VariableDef,
        name_edit: TextEdit,
        default_edit: TextEdit,
    },
    VariableAdded {
        index: usize,
    },
    VariableType {
        index: usize,
        variable: VariableDef,
        default_edit: TextEdit,
    },
    Recipe {
        nodes: Vec<NodeCardModel>,
        wires: Vec<WireModel>,
        shelved: Vec<NodeId>,
    },
    Edit(Box<EditSnapshot>),
}

#[derive(Debug, Clone)]
pub struct EditSnapshot {
    nodes: Vec<NodeCardModel>,
    wires: Vec<WireModel>,
    shelved: Vec<NodeId>,
    configs: BTreeMap<NodeId, serde_json::Value>,
    prompts: BTreeMap<NodeId, TextEdit>,
    versions: BTreeMap<NodeId, TextEdit>,
    subgraphs: Vec<SubgraphRef>,
    variables: Vec<VariableDef>,
    var_name_edits: Vec<TextEdit>,
    var_default_edits: Vec<TextEdit>,
    aside_wires: HashMap<NodeId, Vec<(SocketRef, SocketRef)>>,
    aside_bridges: HashMap<NodeId, Vec<(SocketRef, SocketRef)>>,
    aside_refused: HashMap<NodeId, Vec<(SocketRef, SocketRef)>>,
    wire_homes: Vec<((SocketRef, SocketRef), (WireId, usize))>,
    wire_ledger: Vec<WireId>,
    typing: Option<String>,
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
    pub frozen_edit: TextEdit,
    pub trace_expanded: bool,
    pub trace_stale: bool,
    pub variables_touched: bool,
    pub model_menu_open: bool,
    pub docked_menu_seen: Option<[f32; 2]>,
    pub open_var_type: Option<usize>,
    pub pending_type_reset: Option<(usize, SocketType)>,
    pub pending_restore_version: Option<u32>,
    pub pending_switch: Option<PendingSwitch>,
    pub var_name_edits: Vec<TextEdit>,
    pub var_default_edits: Vec<TextEdit>,
    pub undo_stack: Vec<UndoEntry>,
    undo_seen: usize,
    last_synced: Option<Box<EditSnapshot>>,
    redo_stack: Vec<EditSnapshot>,
    restoring: bool,
    pub aside_wires: HashMap<NodeId, Vec<(SocketRef, SocketRef)>>,
    pub aside_bridges: HashMap<NodeId, Vec<(SocketRef, SocketRef)>>,
    pub aside_refused: HashMap<NodeId, Vec<(SocketRef, SocketRef)>>,
    pub wire_error: Option<String>,
    wire_homes: Vec<((SocketRef, SocketRef), (WireId, usize))>,
    wire_ledger: Vec<WireId>,
    discrete_edit: bool,
    var_problems: Vec<String>,
    pub saved_potions: Vec<(String, String)>,
    pub saved_live: Vec<(String, u32)>,
    pub pantry_drag: Option<PantryDrag>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct PantryDrag {
    pub kind: String,
    pub at: [f32; 2],
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
                let mut app = Self::from_registry();
                app.load_graph(workspace.graph.clone());
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
        app.refresh_saved_potions();
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
            frozen_edit: TextEdit::new(""),
            trace_expanded: false,
            trace_stale: false,
            variables_touched: false,
            model_menu_open: false,
            docked_menu_seen: None,
            open_var_type: None,
            pending_type_reset: None,
            pending_restore_version: None,
            pending_switch: None,
            var_name_edits: Vec::new(),
            var_default_edits: Vec::new(),
            undo_stack: Vec::new(),
            undo_seen: 0,
            last_synced: None,
            redo_stack: Vec::new(),
            restoring: false,
            aside_wires: HashMap::new(),
            aside_bridges: HashMap::new(),
            aside_refused: HashMap::new(),
            wire_error: None,
            wire_homes: Vec::new(),
            wire_ledger: Vec::new(),
            discrete_edit: false,
            var_problems: Vec::new(),
            saved_potions: Vec::new(),
            saved_live: Vec::new(),
            pantry_drag: None,
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
        app.last_synced = Some(Box::new(app.edit_snapshot(None)));
        app.seed_wire_ledger();
        app.canvas.notes = app.card_notes();
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
        let route = self.flow_ancestors(card_id);
        let mut ranked: Vec<&NodeCardModel> = self
            .canvas
            .nodes
            .iter()
            .filter(|other| !downstream.contains(&other.id) && !self.canvas.is_shelved(&other.id) && self.runs_on(&other.id, &route))
            .collect();
        ranked.sort_by_key(|other| if other.category == "events" { 0 } else { 1 });
        ranked.into_iter().find_map(|other| {
            other.outputs.iter().find(|output| !output.ty.is_flow() && socket.ty.accepts(output.ty)).map(|output| {
                (SocketRef::new(other.id.clone(), output.name.clone()), format!("Pour {} · {}", self.canvas.display_title(&other.id), output.name))
            })
        })
    }

    fn missing_socket_of(&self, card_id: &NodeId) -> Option<String> {
        self.hex_findings.iter().filter(|finding| finding.node.as_ref() == Some(card_id)).find_map(|finding| finding.socket.clone())
    }

    pub fn mend_selected_hex(&mut self) {
        if self.is_running() {
            return;
        }
        let Some(card) = self.canvas.selected_card().cloned() else { return };
        let Some(socket) = self.missing_socket_of(&card.id) else { return };
        let Some((source, _)) = self.hex_mend_source(&card.id) else { return };
        if self.canvas.connect_sockets(&source, &SocketRef::new(card.id.clone(), socket)).is_ok() {
            self.sync_spell();
            self.validate();
        }
    }

    pub fn fill_default_prompt_for_selected(&mut self) {
        if self.is_running() {
            return;
        }
        let Some(card) = self.canvas.selected_card().cloned() else { return };
        let prompt = default_prompt(&card.kind);
        if !prompt.is_empty() {
            self.prompts.insert(card.id.clone(), TextEdit::new(prompt));
        }
        if let Some(card_mut) = self.canvas.card_mut(&card.id) {
            card_mut.status = ExecutionStatus::Idle;
            card_mut.inputs.iter_mut().for_each(|socket| {
                socket.flagged = false;
                socket.mistyped = false;
            });
        }
        self.validate();
    }

    pub fn trace_height(&self) -> f32 {
        if self.trace.is_empty() && !self.is_running() {
            IDLE_TRACE_HEIGHT
        } else if self.trace_expanded {
            OPEN_TRACE_HEIGHT
        } else {
            TRACE_HEIGHT
        }
    }

    pub fn follow_window(&mut self, size: (f32, f32)) {
        if size.0 < 1.0 || size.1 < 1.0 || ((size.0 - self.size.0).abs() < 0.5 && (size.1 - self.size.1).abs() < 0.5) {
            return;
        }
        self.size = size;
        let viewport = self.canvas_size();
        self.canvas.fit(viewport);
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
            Some(Value::Json(json)) => json.to_string(),
            Some(other) => serde_json::to_string(other).unwrap_or_default(),
            None => String::new(),
        }
    }

    fn try_parse_value(raw: &str, ty: SocketType) -> Option<Value> {
        let trimmed = raw.trim();
        match ty {
            SocketType::String | SocketType::Any => Some(Value::String(raw.to_owned())),
            SocketType::Json => serde_json::from_str::<serde_json::Value>(trimmed).ok().map(Value::Json),
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
        self.var_problems = self.variable_errors();
    }

    pub fn var_name_error(&self, idx: usize) -> Option<&'static str> {
        let name = self.var_name_edits.get(idx)?.text.trim();
        if name.is_empty() {
            Some("Needs a name")
        } else if self.var_name_edits[..idx].iter().any(|edit| edit.text.trim() == name) {
            Some("Duplicate name")
        } else {
            None
        }
    }

    pub fn var_default_error(&self, idx: usize) -> Option<&'static str> {
        let variable = self.spell.variables.get(idx)?;
        let edit = self.var_default_edits.get(idx)?;
        if edit.text.trim().is_empty() || Self::try_parse_value(&edit.text, variable.ty).is_some() {
            return None;
        }
        Some(match variable.ty {
            SocketType::Int => "Needs a whole number",
            SocketType::Float => "Needs a number",
            SocketType::Bool => "Needs true or false",
            _ => "Needs valid JSON",
        })
    }

    pub fn variable_binding_hint(&self, name: &str) -> (String, bool) {
        if name.trim().is_empty() {
            return ("Unused".to_owned(), false);
        }
        let template_token = format!("{{{{{name}}}}}");
        for card in self.canvas.nodes.iter().filter(|card| !self.canvas.is_shelved(&card.id)) {
            let renders = card.kind == "text/template";
            let in_prompt = renders && self.prompts.get(&card.id).is_some_and(|e| e.text.contains(&template_token));
            let in_config = self.configs.get(&card.id).is_some_and(|cfg| {
                cfg.as_object().is_some_and(|map| {
                    (renders && map.values().any(|v| v.as_str().is_some_and(|s| s.contains(&template_token))))
                        || map.iter().any(|(key, value)| {
                            !magicklala_graph::UNBOUND_SETTINGS.contains(&key.as_str()) && magicklala_graph::bound_variable(value) == Some(name)
                        })
                        || (card.kind.starts_with("var/") && map.get("name").and_then(serde_json::Value::as_str) == Some(name))
                })
            });
            if in_prompt || in_config {
                return (format!("→ {}", self.canvas.display_title(&card.id)), true);
            }
        }
        ("Unused".to_owned(), false)
    }

    pub fn add_variable(&mut self) {
        if self.is_running() {
            return;
        }
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
        self.undo_stack.push(UndoEntry::VariableAdded { index: self.spell.variables.len() - 1 });
        self.undo_banner = Some(format!("Added variable {name}"));
        self.validation = ValidationState::Unchecked;
        self.canvas.validated = false;
        self.dirty = true;
        self.variables_touched = true;
        self.sync_spell();
    }

    pub fn remove_variable_at(&mut self, index: usize) {
        if self.is_running() || index >= self.spell.variables.len() {
            return;
        }
        let variable = self.spell.variables.remove(index);
        let name = variable.name.clone();
        let name_edit = if index < self.var_name_edits.len() { self.var_name_edits.remove(index) } else { TextEdit::new(&variable.name) };
        let default_edit = if index < self.var_default_edits.len() {
            self.var_default_edits.remove(index)
        } else {
            TextEdit::new(Self::format_value(variable.default.as_ref()))
        };
        self.undo_stack.push(UndoEntry::Variable { index, variable, name_edit, default_edit });
        self.undo_banner = Some(format!("Removed variable {name}"));
        self.variables_touched = true;
        self.open_var_type = self.open_var_type.filter(|open| *open != index).map(|open| if open > index { open - 1 } else { open });
        self.pending_type_reset = self.pending_type_reset.filter(|(open, _)| *open != index).map(|(open, ty)| (if open > index { open - 1 } else { open }, ty));
        self.validation = ValidationState::Unchecked;
        self.canvas.validated = false;
        self.dirty = true;
        self.sync_spell();
    }

    pub fn toggle_var_type_menu_at(&mut self, index: usize) {
        if self.is_running() {
            return;
        }
        self.pending_type_reset = None;
        self.open_var_type = if self.open_var_type == Some(index) { None } else { Some(index) };
    }

    fn reset_value(ty: SocketType) -> Option<Value> {
        (ty != SocketType::Json).then(|| Self::zero_value(ty))
    }

    pub fn set_variable_type_at(&mut self, index: usize, ty: SocketType) {
        if self.is_running() || index >= self.spell.variables.len() {
            return;
        }
        self.open_var_type = None;
        if self.spell.variables[index].ty == ty {
            self.pending_type_reset = None;
            return;
        }
        let raw = self.var_default_edits.get(index).map(|e| e.text.clone()).unwrap_or_else(|| Self::format_value(self.spell.variables[index].default.as_ref()));
        if raw.trim().is_empty() {
            self.remember_variable_type(index);
            self.spell.variables[index].ty = ty;
            self.spell.variables[index].default = Self::reset_value(ty);
            if let Some(edit) = self.var_default_edits.get_mut(index) {
                *edit = TextEdit::new(Self::format_value(self.spell.variables[index].default.as_ref()));
            }
            self.pending_type_reset = None;
            self.dirty = true;
            return;
        }
        if let Some(parsed) = Self::try_parse_value(&raw, ty) {
            self.remember_variable_type(index);
            self.spell.variables[index].ty = ty;
            self.spell.variables[index].default = Some(parsed);
            if let Some(edit) = self.var_default_edits.get_mut(index) {
                *edit = TextEdit::new(Self::format_value(self.spell.variables[index].default.as_ref()));
            }
            self.pending_type_reset = None;
            self.dirty = true;
        } else {
            self.pending_type_reset = Some((index, ty));
        }
    }

    pub fn confirm_variable_type_reset(&mut self) {
        if self.is_running() {
            return;
        }
        let Some((index, ty)) = self.pending_type_reset.take() else { return };
        if index >= self.spell.variables.len() {
            return;
        }
        self.remember_variable_type(index);
        self.spell.variables[index].ty = ty;
        self.spell.variables[index].default = Self::reset_value(ty);
        if let Some(edit) = self.var_default_edits.get_mut(index) {
            *edit = TextEdit::new(Self::format_value(self.spell.variables[index].default.as_ref()));
        }
        self.dirty = true;
    }

    fn remember_variable_type(&mut self, index: usize) {
        let variable = self.spell.variables[index].clone();
        let default_edit = self.var_default_edits.get(index).cloned().unwrap_or_else(|| TextEdit::new(Self::format_value(variable.default.as_ref())));
        self.undo_banner = Some(format!("Changed the type of {}", variable.name));
        self.undo_stack.push(UndoEntry::VariableType { index, variable, default_edit });
        self.variables_touched = true;
        self.validation = ValidationState::Unchecked;
        self.canvas.validated = false;
    }

    pub fn cancel_variable_type_reset(&mut self) {
        self.pending_type_reset = None;
    }

    pub fn selected_model_name(&self) -> String {
        let Some(id) = self.canvas.selection.as_ref() else { return "gpt-4o-mini".to_owned() };
        self.configs.get(id).and_then(|c| c.get("model")).and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).unwrap_or("gpt-4o-mini").to_owned()
    }

    pub fn toggle_model_menu(&mut self) {
        if self.is_running() {
            return;
        }
        self.model_menu_open = !self.model_menu_open;
    }

    pub fn select_model(&mut self, provider: &str, model: &str) {
        if self.is_running() {
            return;
        }
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
        self.discrete_edit = true;
        self.sync_spell();
    }

    pub fn load_graph(&mut self, graph: SpellGraph) {
        self.wire_error = None;
        self.wire_homes.clear();
        let mode = self.canvas.mode;
        let light = self.theme == Theme::Light;
        self.stop_active_run();
        self.aside_wires.clear();
        self.aside_bridges.clear();
        self.aside_refused.clear();
        self.spell = graph;
        self.canvas = CanvasState::from_graph(&self.spell, &self.specs);
        self.canvas.mode = mode;
        self.canvas.light = light;
        self.prompts.clear();
        self.versions.clear();
        self.configs.clear();
        self.trace.clear();
        self.trace_stale = false;
        self.trace_events.clear();
        self.pending_trace.clear();
        self.model_menu_open = false;
        self.open_var_type = None;
        self.pending_type_reset = None;
        self.pending_restore_version = None;
        self.pending_switch = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_banner = None;
        self.validation = ValidationState::Unchecked;
        let nodes: Vec<(NodeId, String, serde_json::Value)> =
            self.spell.nodes.iter().map(|node| (node.id.clone(), node.kind.clone(), node.config.clone())).collect();
        for (id, kind, config) in nodes {
            self.track_config(id, &kind, config);
        }
        self.sync_var_edits_from_spell();
        for entry in self.spell.shelf.clone() {
            self.canvas.shelve(&entry.node);
            let parked = socket_pairs(&entry.parked);
            for (from, to) in &parked {
                self.canvas.adopt_sockets(from, to, &self.specs);
            }
            for ((pair, wire), at) in parked.iter().zip(&entry.parked).zip(entry.parked_at.iter().map(|at| Some(*at as usize)).chain(std::iter::repeat(None))) {
                if !wire.id.0.starts_with("parked-") {
                    self.wire_homes.push((pair.clone(), (wire.id.clone(), at.unwrap_or(usize::MAX))));
                }
            }
            if !parked.is_empty() {
                self.aside_wires.insert(entry.node.clone(), parked);
            }
            let bridges = socket_pairs(&entry.bridges);
            if !bridges.is_empty() {
                self.aside_bridges.insert(entry.node.clone(), bridges);
            }
            let refused = socket_pairs(&entry.refused);
            if !refused.is_empty() {
                self.aside_refused.insert(entry.node.clone(), refused);
            }
        }
        self.rebuild_bridges();
        let viewport = self.canvas_size();
        self.canvas.fit(viewport);
        self.canvas.select(self.canvas.nodes.iter().find(|card| card.kind == "ai/generate_text").map(|card| card.id.clone()));
        self.last_synced = Some(Box::new(self.edit_snapshot(None)));
        self.seed_wire_ledger();
        self.canvas.notes = self.card_notes();
    }

    fn has_unsaved_work(&self) -> bool {
        self.dirty
            && (!self.canvas.nodes.is_empty() || !self.spell.variables.is_empty() || self.saved_potions.iter().any(|(saved, _)| saved == &self.spell.name))
    }

    pub fn request_new_spell(&mut self) {
        if self.has_unsaved_work() {
            self.pending_switch = Some(PendingSwitch::NewPotion);
        } else {
            self.new_empty_spell();
        }
    }

    pub fn request_open_sample(&mut self, name: &str) {
        if self.spell.name == name {
            return;
        }
        if self.has_unsaved_work() {
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

    fn stop_active_run(&mut self) {
        if let Some(active) = self.active.take() {
            active.token.cancel();
            if !active.handle.is_finished() {
                self.stopping.push(active.handle);
            }
        }
        self.trace.clear();
        self.trace_events.clear();
        self.pending_trace.clear();
        self.last_run = None;
        self.run_started = None;
        self.canvas.running_brew = false;
        self.canvas.last_brew = None;
        self.canvas.brew_total_ms = None;
    }

    fn drop_active_run(&mut self) {
        self.stop_active_run();
        self.chat.clear();
    }

    fn detach_spell_history(&mut self) {
        self.drop_active_run();
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

    pub fn recipe_summary(&self, graph: &SpellGraph) -> String {
        let mut canvas = CanvasState::from_graph(graph, &self.specs);
        for entry in &graph.shelf {
            canvas.shelve(&entry.node);
        }
        let plan = magicklala_canvas::brew_plan(&canvas, self.canvas_size());
        let ingredients = plan.with_role(magicklala_canvas::BrewRole::Ingredient).count();
        let aside = plan.with_role(magicklala_canvas::BrewRole::Aside).count();
        let word = if ingredients == 1 { "ingredient" } else { "ingredients" };
        if aside > 0 { format!("{ingredients} {word} · {aside} aside") } else { format!("{ingredients} {word}") }
    }

    pub fn refresh_saved_potions(&mut self) {
        self.saved_potions = self
            .store
            .as_ref()
            .map(|store| store.saved_spells().into_iter().map(|record| (record.name.clone(), self.recipe_summary(&record.graph))).collect())
            .unwrap_or_default();
        self.saved_live = self
            .store
            .as_ref()
            .map(|store| {
                store
                    .saved_spells()
                    .into_iter()
                    .filter_map(|record| store.releases_of_record(record.id).last().map(|release| (record.name.clone(), release.version)))
                    .collect()
            })
            .unwrap_or_default();
    }

    fn adopt_releases(&mut self, workspace: &StudioWorkspace) {
        self.releases = workspace.releases.iter().map(|release| Release { version: release.version, nodes: release.graph.nodes.len() }).collect();
        self.release_graphs = workspace.releases.iter().map(|release| (release.version, release.graph.clone())).collect();
        self.release_notes = workspace.releases.iter().map(|release| (release.version, release.created_at.format("%b %d · %H:%M").to_string())).collect();
        self.last_published = workspace.releases.last().cloned();
        self.active_release_version = workspace.active_release;
        self.canvas.release_badge = self.active_release_version.map(|v| format!("v{v}"));
        self.dirty = false;
    }

    pub fn open_sample_spell(&mut self, name: &str) {
        if let Some(record) = self.store.as_ref().and_then(|store| store.saved_spells().into_iter().find(|record| record.name == name)) {
            self.detach_spell_history();
            let releases = self.store.as_ref().map(|store| store.releases_of_record(record.id)).unwrap_or_default();
            let workspace = StudioWorkspace::of(record.clone(), releases);
            self.load_graph(workspace.graph.clone());
            if let Some(store) = self.store.as_mut() {
                store.attach_record(record.id);
            }
            self.adopt_releases(&workspace);
            self.refresh_saved_potions();
            return;
        }
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
                place("search", "knowledge/search", [250.0, 40.0], json!({ "limit": 5 })),
                place("llm", "ai/generate_text", [500.0, 90.0], json!({ "prompt": "Summarize the research findings and answer the question." })),
                place("reply", "io/send_message", [750.0, 90.0], serde_json::Value::Null),
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
                wire("w4", ("trigger", "content"), ("llm", "prompt")),
                wire("w7", ("search", "text"), ("llm", "system")),
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
        if self.is_running() {
            return;
        }
        self.pending_restore_version = Some(version);
    }

    pub fn cancel_restore_release(&mut self) {
        self.pending_restore_version = None;
    }

    pub fn confirm_restore_to_draft(&mut self) {
        if self.is_running() {
            return;
        }
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
        self.active_release_version = self.live_release_matching_recipe();
        self.canvas.release_badge = self.active_release_version.map(|version| format!("v{version}"));
        self.dirty = true;
        self.draft_status = Some(format!("Loaded v{version} into draft"));
    }

    pub fn selected_temperature(&self) -> Option<f64> {
        let id = self.canvas.selection.as_ref()?;
        let setting = self.configs.get(id).and_then(|c| c.get("temperature"))?;
        match self.temperature_variable(setting) {
            Some(index) => self.var_default_edits.get(index).and_then(|edit| edit.text.trim().parse::<f64>().ok()),
            None => setting.as_f64(),
        }
    }

    pub fn selected_temperature_source(&self) -> Option<String> {
        let id = self.canvas.selection.as_ref()?;
        let setting = self.configs.get(id)?.get("temperature")?;
        self.temperature_variable(setting).map(|index| self.spell.variables[index].name.clone())
    }

    fn temperature_variable(&self, setting: &serde_json::Value) -> Option<usize> {
        let name = magicklala_graph::bound_variable(setting)?;
        self.spell.variables.iter().position(|variable| variable.name == name)
    }

    pub fn selected_provider(&self) -> String {
        let Some(id) = self.canvas.selection.as_ref() else { return "openai".to_owned() };
        self.configs.get(id).and_then(|c| c.get("provider")).and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).unwrap_or("openai").to_owned()
    }

    pub fn adjust_temperature(&mut self, delta: f64) {
        if self.is_running() {
            return;
        }
        let current = self.selected_temperature().unwrap_or(0.7);
        let next = ((current + delta).clamp(0.0, 2.0) * 10.0).round() / 10.0;
        self.discrete_edit = true;
        let bound = self
            .canvas
            .selection
            .as_ref()
            .and_then(|id| self.configs.get(id))
            .and_then(|c| c.get("temperature"))
            .and_then(|setting| self.temperature_variable(setting));
        if let Some(index) = bound {
            if let Some(edit) = self.var_default_edits.get_mut(index) {
                *edit = TextEdit::new(next.to_string());
            }
            self.sync_spell();
            return;
        }
        if let Some(id) = self.canvas.selection.clone() {
            let entry = self.configs.entry(id).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            if let Some(map) = entry.as_object_mut() {
                map.insert("temperature".to_owned(), serde_json::json!(next));
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
        let wire_in = self.canvas.mode == WorkbenchMode::Cauldron;
        self.place_ingredient(kind, screen, wire_in)
    }

    pub fn unplug_wire(&mut self, wire: &magicklala_graph::WireId) {
        if self.is_running() {
            return;
        }
        let cut = self
            .canvas
            .wires
            .iter()
            .find(|candidate| &candidate.id == wire)
            .map(|cut| (SocketRef::new(cut.from_node.clone(), cut.from_socket.clone()), SocketRef::new(cut.to_node.clone(), cut.to_socket.clone())));
        if let Some(pair) = cut {
            for (id, bridges) in &mut self.aside_bridges {
                if let Some(index) = bridges.iter().position(|bridge| bridge == &pair) {
                    bridges.remove(index);
                    self.aside_refused.entry(id.clone()).or_default().push(pair.clone());
                }
            }
            self.aside_bridges.retain(|_, bridges| !bridges.is_empty());
        }
        self.canvas.wires.retain(|candidate| &candidate.id != wire);
        self.validation = ValidationState::Unchecked;
        self.sync_spell();
    }

    fn place_ingredient(&mut self, kind: &str, screen: [f32; 2], wire_in: bool) -> Option<NodeId> {
        if self.is_running() {
            return None;
        }
        let spec = self.specs.iter().find(|spec| spec.kind == kind)?.clone();
        let mark = self.undo_stack.len();
        self.undo_stack.push(UndoEntry::Edit(Box::new(self.edit_snapshot(None))));
        self.undo_banner = Some(format!("Added {}", spec.title));
        let center = self.canvas.camera.screen_to_world(screen);
        let id = self.canvas.add_node_from_spec(&spec, center);
        self.track_config(id.clone(), kind, serde_json::Value::Null);
        if editable_field(kind) == Some("name")
            && let Some(first) = self.spell.variables.first()
        {
            self.prompts.insert(id.clone(), TextEdit::new(first.name.clone()));
        }
        if wire_in && kind != SUBGRAPH_KIND {
            self.auto_wire_cauldron_ingredient(&id, &spec);
            self.canvas.toss_count += 1;
        } else if self.canvas.mode == WorkbenchMode::Cauldron {
            self.canvas.shelve(&id);
        }
        let source = self
            .canvas
            .wires
            .iter()
            .find(|wire| wire.to_node == id && wire.ty.is_flow())
            .or_else(|| self.canvas.wires.iter().find(|wire| wire.to_node == id))
            .map(|wire| wire.from_node.clone());
        let near = match source.and_then(|from| self.canvas.card(&from).map(|card| [card.position[0] + card.size[0] + 40.0, card.position[1]])) {
            Some(after) => after,
            None => self.canvas.card(&id).map(|card| card.position).unwrap_or(center),
        };
        let spot = self.canvas.free_spot(&id, near);
        self.canvas.move_node(&id, spot);
        self.validation = ValidationState::Unchecked;
        self.sync_spell();
        self.fold_undo_since(mark);
        Some(id)
    }

    fn fold_undo_since(&mut self, mark: usize) {
        self.undo_stack.truncate(mark + 1);
        self.undo_seen = self.undo_stack.len();
    }

    pub fn canvas_origin(&self) -> [f32; 2] {
        [self.left_rail_width(), TOOLBAR_HEIGHT]
    }

    pub fn pantry_drag_point(&self) -> Option<[f32; 2]> {
        let drag = self.pantry_drag.as_ref()?;
        let origin = self.canvas_origin();
        let local = [drag.at[0] - origin[0], drag.at[1] - origin[1]];
        let size = self.canvas_size();
        (local[0] >= 0.0 && local[1] >= 0.0 && local[0] <= size.0 && local[1] <= size.1).then_some(local)
    }

    pub fn pantry_drag_over_pot(&self) -> bool {
        self.canvas.mode == WorkbenchMode::Cauldron
            && self.pantry_drag_point().is_some_and(|point| magicklala_canvas::brew_plan(&self.canvas, self.canvas_size()).is_over_pot(point))
    }

    pub fn pantry_drag_move(&mut self, kind: &str, at: [f32; 2]) {
        if self.is_running() {
            self.pantry_drag = None;
            return;
        }
        self.pantry_drag = Some(PantryDrag { kind: kind.to_owned(), at });
        self.canvas.pot_hover = self.pantry_drag_over_pot();
    }

    pub fn pantry_drop(&mut self, cancelled: bool) -> Option<NodeId> {
        if self.is_running() {
            self.pantry_drag = None;
            self.canvas.pot_hover = false;
            return None;
        }
        let point = self.pantry_drag_point();
        let over_pot = self.pantry_drag_over_pot();
        let drag = self.pantry_drag.take()?;
        self.canvas.pot_hover = false;
        if cancelled {
            return None;
        }
        let point = point?;
        match self.canvas.mode {
            WorkbenchMode::Cauldron => {
                let view = self.canvas_size();
                let count = self.canvas.nodes.len() as f32;
                self.place_ingredient(&drag.kind, [view.0 * 0.5 - 100.0 + count * 12.0, view.1 * 0.62 + count * 8.0], over_pot)
            }
            WorkbenchMode::Grimoire => self.place_ingredient(&drag.kind, [point[0] - magicklala_canvas::CARD_WIDTH * 0.5, point[1] - 14.0], false),
        }
    }

    pub fn toss(&mut self, toss: magicklala_canvas::Toss) {
        if self.is_running() {
            return;
        }
        match toss {
            magicklala_canvas::Toss::Aside(id) => {
                let shortcuts: Vec<(SocketRef, SocketRef)> = self.aside_bridges.values().flatten().cloned().collect();
                let removed: Vec<(SocketRef, SocketRef)> = self
                    .canvas
                    .wires
                    .iter()
                    .filter(|wire| wire.from_node == id || wire.to_node == id)
                    .map(|wire| {
                        (SocketRef::new(wire.from_node.clone(), wire.from_socket.clone()), SocketRef::new(wire.to_node.clone(), wire.to_socket.clone()))
                    })
                    .filter(|pair| !shortcuts.contains(pair))
                    .collect();
                self.order_wires();
                for wire in &self.canvas.wires {
                    let pair = (SocketRef::new(wire.from_node.clone(), wire.from_socket.clone()), SocketRef::new(wire.to_node.clone(), wire.to_socket.clone()));
                    if removed.contains(&pair) {
                        let index = self.wire_ledger.iter().position(|known| known == &wire.id).unwrap_or(usize::MAX);
                        let home = (pair, (wire.id.clone(), index));
                        self.wire_homes.retain(|(known, _)| known != &home.0);
                        self.wire_homes.push(home);
                    }
                }
                if removed.is_empty() {
                    let title = self.canvas.display_title(&id);
                    self.canvas.set_aside(&id);
                    self.rebuild_bridges();
                    self.undo_stack.push(UndoEntry::Aside { id, wires: Vec::new(), bridges: Vec::new() });
                    self.undo_banner = Some(format!("Set aside {title}"));
                    self.validation = ValidationState::Unchecked;
                    self.sync_spell();
                    return;
                }
                self.canvas.set_aside(&id);
                self.aside_wires.insert(id.clone(), removed.clone());
                self.rebuild_bridges();
                let title = self.canvas.display_title(&id);
                let bridges = self.aside_bridges.get(&id).cloned().unwrap_or_default();
                self.undo_stack.push(UndoEntry::Aside { id, wires: removed, bridges });
                self.undo_banner = Some(format!("Set aside {title}"));
            }
            magicklala_canvas::Toss::IntoPot(id) => {
                self.canvas.unshelve(&id);
                let stash = self.aside_wires.remove(&id).unwrap_or_default();
                let stale: Vec<(SocketRef, SocketRef)> = self.aside_bridges.drain().flat_map(|(_, wires)| wires).collect();
                self.drop_bridges(&stale);
                let (waiting, stash): (Vec<_>, Vec<_>) = stash.into_iter().partition(|(from, to)| {
                    let other = if from.node == id { &to.node } else { &from.node };
                    self.canvas.is_shelved(other)
                });
                for (from, to) in waiting {
                    let other = if from.node == id { to.node.clone() } else { from.node.clone() };
                    let parked = self.aside_wires.entry(other).or_default();
                    if !parked.contains(&(from.clone(), to.clone())) {
                        parked.push((from, to));
                    }
                }
                let mut restored = 0;
                for (from, to) in &stash {
                    let flow = self
                        .canvas
                        .card(&to.node)
                        .and_then(|card| card.inputs.iter().find(|socket| socket.name == to.socket))
                        .is_some_and(|socket| socket.ty.is_flow());
                    let open = (flow || !self.is_fed(&to.node, &to.socket)) && self.canvas.card(&from.node).is_some() && self.canvas.card(&to.node).is_some();
                    if open && self.canvas.connect_sockets(from, to).is_ok() {
                        restored += 1;
                    }
                }
                let mut homed: Vec<((SocketRef, SocketRef), usize)> =
                    stash.iter().filter_map(|pair| self.wire_home(pair).map(|(_, at)| (pair.clone(), at))).collect();
                homed.sort_by_key(|(_, at)| *at);
                for (pair, _) in homed {
                    self.rehome_wire(&pair);
                }
                if restored == 0
                    && let Some(kind) = self.canvas.card(&id).map(|card| card.kind.clone())
                    && let Some(spec) = self.specs.iter().find(|spec| spec.kind == kind).cloned()
                {
                    self.auto_wire_cauldron_ingredient(&id, &spec);
                }
                self.rebuild_bridges();
                self.canvas.toss_count += 1;
            }
        }
        self.validation = ValidationState::Unchecked;
        self.sync_spell();
    }

    fn spec_of(&self, node: &NodeId) -> Option<NodeSpec> {
        let kind = &self.canvas.card(node)?.kind;
        self.specs.iter().find(|spec| &spec.kind == kind).cloned()
    }

    fn flow_ancestors(&self, id: &NodeId) -> std::collections::HashSet<NodeId> {
        let mut seen = std::collections::HashSet::new();
        let mut queue = VecDeque::from([id.clone()]);
        while let Some(node) = queue.pop_front() {
            for wire in self.canvas.wires.iter().filter(|wire| wire.to_node == node && wire.ty.is_flow()) {
                if seen.insert(wire.from_node.clone()) {
                    queue.push_back(wire.from_node.clone());
                }
            }
        }
        seen
    }

    fn runs_on(&self, node: &NodeId, route: &std::collections::HashSet<NodeId>) -> bool {
        self.runs_on_seen(node, route, &mut std::collections::HashSet::new())
    }

    fn runs_on_seen(&self, node: &NodeId, route: &std::collections::HashSet<NodeId>, seen: &mut std::collections::HashSet<NodeId>) -> bool {
        if route.contains(node) {
            return true;
        }
        if !seen.insert(node.clone()) {
            return false;
        }
        let flowless = self.canvas.card(node).is_some_and(|card| card.inputs.iter().chain(card.outputs.iter()).all(|socket| !socket.ty.is_flow()));
        flowless && self.canvas.wires.iter().filter(|wire| &wire.to_node == node).all(|wire| self.runs_on_seen(&wire.from_node, route, seen))
    }

    fn downstream_of(&self, id: &NodeId) -> std::collections::HashSet<NodeId> {
        let mut seen = std::collections::HashSet::from([id.clone()]);
        let mut queue = VecDeque::from([id.clone()]);
        while let Some(node) = queue.pop_front() {
            for wire in self.canvas.wires.iter().filter(|wire| wire.from_node == node) {
                if seen.insert(wire.to_node.clone()) {
                    queue.push_back(wire.to_node.clone());
                }
            }
        }
        seen
    }

    fn feeds(&self, node: &NodeId, socket: &str) -> bool {
        self.canvas.wires.iter().any(|wire| &wire.from_node == node && wire.from_socket == socket)
    }

    fn is_fed(&self, node: &NodeId, socket: &str) -> bool {
        self.canvas.wires.iter().any(|wire| &wire.to_node == node && wire.to_socket == socket)
    }

    fn connect(&mut self, from: (&NodeId, &str), to: (&NodeId, &str)) -> bool {
        self.canvas.connect_sockets(&SocketRef::new(from.0.clone(), from.1.to_owned()), &SocketRef::new(to.0.clone(), to.1.to_owned())).is_ok()
    }

    fn feed_from(&mut self, sources: &[NodeId], target: &NodeId, socket: &str, ty: SocketType) -> bool {
        let banned = self.downstream_of(target);
        for exact in [true, false] {
            for source in sources.iter().filter(|source| !banned.contains(*source)) {
                let Some(card) = self.canvas.card(source) else { continue };
                let outs: Vec<String> = card
                    .outputs
                    .iter()
                    .filter(|out| !out.ty.is_flow() && if exact { out.ty == ty } else { out.ty != ty })
                    .map(|out| out.name.clone())
                    .collect();
                for out in outs {
                    if self.connect((source, &out), (target, socket)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn season_reply(&mut self, id: &NodeId, spec: &NodeSpec) -> bool {
        let Some(out) = spec.outputs.iter().find(|socket| !socket.ty.is_flow()) else { return false };
        let bottles: Vec<NodeId> =
            self.canvas.nodes.iter().filter(|card| magicklala_canvas::brew::is_bottle(card) && &card.id != id).map(|card| card.id.clone()).collect();
        for bottle in bottles {
            let Some(bottle_spec) = self.spec_of(&bottle) else { continue };
            let Some(words) = bottle_spec.inputs.iter().find(|input| input.required && !input.ty.is_flow()) else { continue };
            let Some(feed) = self.canvas.wires.iter().find(|wire| wire.to_node == bottle && wire.to_socket == words.name).cloned() else { continue };
            let Some(intake) = spec.inputs.iter().filter(|input| !input.ty.is_flow()).find(|input| input.ty == feed.ty || input.ty == SocketType::Any) else {
                continue;
            };
            if !(out.ty == words.ty || words.ty == SocketType::Any) {
                continue;
            }
            self.canvas.wires.retain(|wire| wire.id != feed.id);
            let fed = self.connect((&feed.from_node, &feed.from_socket), (id, &intake.name));
            let poured = self.connect((id, &out.name), (&bottle, &words.name));
            if fed && poured {
                return true;
            }
            let _ = self.canvas.connect_sockets(
                &SocketRef::new(feed.from_node.clone(), feed.from_socket.clone()),
                &SocketRef::new(feed.to_node.clone(), feed.to_socket.clone()),
            );
        }
        false
    }

    fn auto_wire_cauldron_ingredient(&mut self, id: &NodeId, spec: &NodeSpec) {
        let widened;
        let spec = match spec.inputs.iter().position(|input| input.name == magicklala_graph::DYNAMIC_SOCKET) {
            Some(open) => {
                let mut wide = spec.clone();
                wide.inputs[open].name = "content".to_owned();
                wide.inputs[open].label = "content".to_owned();
                wide.inputs[open].required = true;
                if let Some(card) = self.canvas.nodes.iter_mut().find(|card| &card.id == id) {
                    card.ensure_socket("content", wide.inputs[open].ty, false);
                }
                widened = wide;
                &widened
            }
            None => spec,
        };
        let my_flow_in_of = |spec: &NodeSpec| spec.inputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone());
        let my_flow_out_of = |spec: &NodeSpec| spec.outputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone());
        let mut without = self.canvas.clone();
        without.nodes.retain(|card| &card.id != id);
        without.wires.retain(|wire| &wire.from_node != id && &wire.to_node != id);
        let in_pot = magicklala_canvas::brewing_nodes(&without);
        let waiting: Vec<NodeId> = self
            .canvas
            .nodes
            .iter()
            .filter(|card| &card.id != id && !self.aside_wires.contains_key(&card.id))
            .filter(|card| card.inputs.iter().chain(card.outputs.iter()).all(|socket| !socket.ty.is_flow()))
            .filter(|card| self.canvas.wires.iter().any(|wire| wire.to_node == card.id) && !self.canvas.wires.iter().any(|wire| wire.from_node == card.id))
            .map(|card| card.id.clone())
            .collect();
        let order: Vec<NodeId> = magicklala_canvas::recipe_order(&self.canvas)
            .into_iter()
            .filter(|node| node != id)
            .filter(|node| !self.canvas.is_shelved(node))
            .filter(|node| {
                in_pot.contains(node) || waiting.contains(node) || self.canvas.card(node).is_some_and(|card| card.category.eq_ignore_ascii_case("events"))
            })
            .collect();
        if my_flow_in_of(spec).is_none() && my_flow_out_of(spec).is_none() && !spec.is_trigger && self.season_reply(id, spec) {
            return;
        }
        let bottle = |app: &Self, node: &NodeId| app.canvas.card(node).is_some_and(magicklala_canvas::brew::is_bottle);
        let flow_out_of =
            |app: &Self, node: &NodeId| app.canvas.card(node).and_then(|card| card.outputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone()));
        let flow_in_of = |app: &Self, node: &NodeId| app.canvas.card(node).and_then(|card| card.inputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone()));
        let free_flow_out_of = |app: &Self, node: &NodeId| {
            app.canvas.card(node).and_then(|card| card.outputs.iter().find(|s| s.ty.is_flow() && !app.feeds(node, &s.name)).map(|s| s.name.clone()))
        };
        let my_flow_in = spec.inputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone());
        let my_flow_out = spec.outputs.iter().find(|s| s.ty.is_flow()).map(|s| s.name.clone());
        let mut sources: Vec<NodeId> = Vec::new();

        if spec.is_trigger {
            if let Some(out) = &my_flow_out {
                let head = order.iter().find(|node| flow_in_of(self, node).is_some_and(|input| !self.is_fed(node, &input))).cloned();
                if let Some(head) = head {
                    let input = flow_in_of(self, &head).unwrap_or_default();
                    self.connect((id, out), (&head, &input));
                    let wanted: Vec<(String, SocketType)> = self
                        .spec_of(&head)
                        .map(|spec| spec.inputs.iter().filter(|s| s.required && !s.ty.is_flow()).map(|s| (s.name.clone(), s.ty)).collect())
                        .unwrap_or_default();
                    for (name, ty) in wanted {
                        if !self.is_fed(&head, &name) {
                            self.feed_from(std::slice::from_ref(id), &head, &name, ty);
                        }
                    }
                }
            }
            return;
        }

        if let Some(input) = &my_flow_in {
            let free_tail = order.iter().rev().filter(|node| !bottle(self, node)).find(|node| free_flow_out_of(self, node).is_some()).cloned();
            let before_bottle = order.iter().rev().find_map(|node| {
                let out = flow_out_of(self, node)?;
                self.canvas
                    .wires
                    .iter()
                    .find(|wire| &wire.from_node == node && wire.from_socket == out && bottle(self, &wire.to_node))
                    .map(|wire| (node.clone(), wire.id.clone(), wire.to_node.clone(), wire.to_socket.clone()))
            });
            let splice = before_bottle.filter(|_| !bottle(self, id) && my_flow_out.is_some());
            match (free_tail.filter(|_| splice.is_none()), splice) {
                (Some(tail), _) => {
                    let out = free_flow_out_of(self, &tail).unwrap_or_default();
                    if self.connect((&tail, &out), (id, input)) {
                        sources.push(tail);
                    }
                }
                (None, Some((prev, wire, next, next_in))) => {
                    let out = flow_out_of(self, &prev).unwrap_or_default();
                    self.canvas.wires.retain(|candidate| candidate.id != wire);
                    if self.connect((&prev, &out), (id, input)) {
                        sources.push(prev.clone());
                    }
                    let mine = my_flow_out.clone().unwrap_or_default();
                    self.connect((id, &mine), (&next, &next_in));
                    let makes_content = spec.category.eq_ignore_ascii_case("ai") || spec.category.eq_ignore_ascii_case("text");
                    let reply_words = self
                        .spec_of(&next)
                        .and_then(|bottle| bottle.inputs.iter().find(|input| input.required && !input.ty.is_flow()).map(|input| input.name.clone()));
                    let handed: Vec<(WireModel, String)> = self
                        .canvas
                        .wires
                        .iter()
                        .filter(|wire| makes_content && wire.from_node == prev && wire.to_node == next && !wire.ty.is_flow())
                        .filter(|wire| reply_words.as_deref() == Some(wire.to_socket.as_str()))
                        .filter_map(|wire| spec.outputs.iter().find(|out| out.ty == wire.ty).map(|out| (wire.clone(), out.name.clone())))
                        .collect();
                    for (wire, out) in handed {
                        self.canvas.wires.retain(|candidate| candidate.id != wire.id);
                        if !self.connect((id, &out), (&next, &wire.to_socket)) {
                            let _ = self.canvas.connect_sockets(
                                &SocketRef::new(wire.from_node.clone(), wire.from_socket.clone()),
                                &SocketRef::new(wire.to_node.clone(), wire.to_socket.clone()),
                            );
                        }
                    }
                }
                _ => {
                    let any = order.iter().rev().filter(|node| !bottle(self, node)).find(|node| flow_out_of(self, node).is_some()).cloned();
                    if let Some(prev) = any {
                        let out = flow_out_of(self, &prev).unwrap_or_default();
                        if self.connect((&prev, &out), (id, input)) {
                            sources.push(prev);
                        }
                    }
                }
            }
        }

        if let Some(out) = &my_flow_out
            && !bottle(self, id)
            && !self.feeds(id, out)
            && let Some(next) = order.iter().find(|node| bottle(self, node) && flow_in_of(self, node).is_some_and(|input| !self.is_fed(node, &input))).cloned()
        {
            let input = flow_in_of(self, &next).unwrap_or_default();
            self.connect((id, out), (&next, &input));
        }

        let rest: Vec<NodeId> = order.iter().rev().filter(|node| !sources.contains(node) && !waiting.contains(node)).cloned().collect();
        let mut fed_first: Vec<NodeId> = waiting.iter().rev().cloned().collect();
        fed_first.extend(sources);
        sources = fed_first;
        sources.extend(rest);
        let route = self.flow_ancestors(id);
        if !route.is_empty() {
            sources.retain(|node| self.runs_on(node, &route));
        }
        for socket in spec.inputs.iter().filter(|s| s.required && !s.ty.is_flow()) {
            if !self.is_fed(id, &socket.name) {
                self.feed_from(&sources, id, &socket.name, socket.ty);
            }
        }
        if bottle(self, id) && spec.inputs.iter().any(|s| s.required && !s.ty.is_flow() && !self.is_fed(id, &s.name)) {
            self.toss(magicklala_canvas::Toss::Aside(id.clone()));
        }

        if my_flow_in.is_none() && my_flow_out.is_none() {
            let downstream = self.downstream_of(id);
            let consumers: Vec<NodeId> = order.iter().filter(|node| !downstream.contains(*node) && flow_in_of(self, node).is_some()).cloned().collect();
            let outs: Vec<(String, SocketType)> = spec.outputs.iter().filter(|s| !s.ty.is_flow()).map(|s| (s.name.clone(), s.ty)).collect();
            'outer: for required_first in [true, false] {
                for exact in [true, false] {
                    for consumer in consumers.iter().rev() {
                        let open: Vec<(String, SocketType)> = self
                            .spec_of(consumer)
                            .map(|spec| {
                                spec.inputs
                                    .iter()
                                    .filter(|s| !s.ty.is_flow() && s.required == required_first && !self.is_fed(consumer, &s.name))
                                    .map(|s| (s.name.clone(), s.ty))
                                    .collect()
                            })
                            .unwrap_or_default();
                        for (input, in_ty) in open {
                            for (out, out_ty) in &outs {
                                if (*out_ty == in_ty) == exact && self.connect((id, out), (consumer, &input)) {
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn track_config(&mut self, id: NodeId, kind: &str, config: serde_json::Value) {
        if let Some(field) = editable_field(kind) {
            let text = match config.get(field) {
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(serde_json::Value::Array(items)) if listed_field(kind) => {
                    listed_text(&items.iter().filter_map(serde_json::Value::as_str).collect::<Vec<_>>())
                }
                Some(other) => other.to_string(),
                None if listed_field(kind) => self
                    .specs
                    .iter()
                    .find(|spec| spec.kind == kind)
                    .and_then(|spec| spec.config.iter().find(|entry| entry.key == field))
                    .and_then(|entry| entry.default.as_ref())
                    .and_then(serde_json::Value::as_array)
                    .map(|items| listed_text(&items.iter().filter_map(serde_json::Value::as_str).collect::<Vec<_>>()))
                    .unwrap_or_default(),
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
        let mut config = self.prompted_config(id, kind);
        if kind == "code/rhai_script" && config.get("inputs").is_none() {
            let fed: Vec<serde_json::Value> = self
                .canvas
                .wires
                .iter()
                .filter(|wire| &wire.to_node == id && !wire.ty.is_flow())
                .map(|wire| serde_json::Value::String(wire.to_socket.clone()))
                .collect();
            if !fed.is_empty() {
                if !config.is_object() {
                    config = json!({});
                }
                if let Some(map) = config.as_object_mut() {
                    map.insert("inputs".to_owned(), serde_json::Value::Array(fed));
                }
            }
        }
        match self.versions.get(id).filter(|_| kind == SUBGRAPH_KIND) {
            Some(edit) => with_release_version(config, &edit.text),
            None => config,
        }
    }

    fn prompted_config(&self, id: &NodeId, kind: &str) -> serde_json::Value {
        let mut config = self.configs.get(id).cloned().unwrap_or(serde_json::Value::Null);
        let (Some(field), Some(edit)) = (editable_field(kind), self.prompts.get(id)) else { return config };
        let present = config.get(field).is_some();
        if edit.text.trim().is_empty() {
            let cleared_list = listed_field(kind) && (present || self.listed_default(kind, field) != json!([]));
            if cleared_list {
                if !config.is_object() {
                    config = json!({});
                }
                if let Some(map) = config.as_object_mut() {
                    map.insert(field.to_owned(), json!([]));
                }
            } else if let Some(map) = config.as_object_mut().filter(|_| present) {
                map.remove(field);
            }
            return config;
        }
        if listed_field(kind) {
            let names = listed_names(&edit.text);
            if !present && json!(names) == self.listed_default(kind, field) {
                return config;
            }
            if !config.is_object() {
                config = json!({});
            }
            if let Some(map) = config.as_object_mut() {
                map.insert(field.to_owned(), json!(names));
            }
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

    fn card_notes(&self) -> BTreeMap<NodeId, String> {
        self.canvas
            .nodes
            .iter()
            .filter_map(|card| {
                let spec = self.specs.iter().find(|spec| spec.kind == card.kind);
                card_note(&card.kind, &self.node_config(&card.id, &card.kind), spec, &self.spell.variables).map(|note| (card.id.clone(), note))
            })
            .collect()
    }

    fn sync_listed_outputs(&mut self) {
        let listed: Vec<(NodeId, Vec<String>, SocketType, Vec<String>)> = self
            .canvas
            .nodes
            .iter()
            .filter(|card| listed_field(&card.kind))
            .filter_map(|card| {
                let spec = self.specs.iter().find(|spec| spec.kind == card.kind)?;
                let open = spec.outputs.iter().find(|socket| socket.name == magicklala_graph::DYNAMIC_SOCKET)?;
                let field = editable_field(&card.kind)?;
                let configured = self.prompted_config(&card.id, &card.kind);
                let listed = configured.get(field).cloned().unwrap_or_else(|| self.listed_default(&card.kind, field));
                let names = listed.as_array().map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                Some((card.id.clone(), names, open.ty, fixed_outputs(spec)))
            })
            .collect();
        for (id, names, ty, fixed) in listed {
            self.canvas.list_outputs(&id, &names, ty, &fixed);
        }
    }

    fn carry_rename(&mut self, old: &str, new: &str) {
        let renamed = format!("{{{{{new}}}}}");
        let fed: std::collections::HashSet<NodeId> =
            self.canvas.wires.iter().filter(|wire| wire.to_socket == old && !wire.ty.is_flow()).map(|wire| wire.to_node.clone()).collect();
        for (_, config) in self.configs.iter_mut().filter(|(id, _)| !fed.contains(*id)) {
            for value in config.as_object_mut().into_iter().flat_map(|settings| settings.values_mut()) {
                if magicklala_graph::bound_variable(value) == Some(old) {
                    *value = serde_json::Value::String(renamed.clone());
                } else if let Some(text) = value.as_str().filter(|text| template_roots(text).iter().any(|root| root == old)) {
                    *value = serde_json::Value::String(magicklala_nodes::rename_template_root(text, old, new));
                }
            }
        }
        let kinds: BTreeMap<NodeId, String> = self.canvas.nodes.iter().map(|card| (card.id.clone(), card.kind.clone())).collect();
        for (id, edit) in self.prompts.iter_mut().filter(|(id, _)| !fed.contains(*id)) {
            let names_variable = kinds.get(id).is_some_and(|kind| kind.starts_with("var/")) && edit.text.trim() == old;
            if names_variable {
                *edit = TextEdit::new(new);
            } else if template_roots(&edit.text).iter().any(|root| root == old) {
                *edit = TextEdit::new(magicklala_nodes::rename_template_root(&edit.text, old, new));
            }
        }
    }

    fn where_they_stand(&self, mut nodes: Vec<NodeCardModel>) -> Vec<NodeCardModel> {
        for card in &mut nodes {
            if let Some(now) = self.canvas.card(&card.id) {
                card.position = now.position;
            }
        }
        nodes
    }

    fn edit_snapshot(&self, typing: Option<String>) -> EditSnapshot {
        EditSnapshot {
            nodes: self.canvas.nodes.clone(),
            wires: self.canvas.wires.clone(),
            shelved: self.canvas.shelved.clone(),
            configs: self.configs.clone(),
            prompts: self.prompts.clone(),
            versions: self.versions.clone(),
            subgraphs: self.spell.subgraphs.clone(),
            variables: self.spell.variables.clone(),
            var_name_edits: self.var_name_edits.clone(),
            var_default_edits: self.var_default_edits.clone(),
            aside_wires: self.aside_wires.clone(),
            aside_bridges: self.aside_bridges.clone(),
            aside_refused: self.aside_refused.clone(),
            wire_homes: self.wire_homes.clone(),
            wire_ledger: self.wire_ledger.clone(),
            typing,
        }
    }

    fn typed_node(&self, nodes: &[NodeInstance], wires: &[Wire], subgraphs: &[SubgraphRef], shelf: &[magicklala_graph::ShelfEntry]) -> Option<NodeId> {
        if wires != self.spell.wires.as_slice()
            || subgraphs != self.spell.subgraphs.as_slice()
            || shelf != self.spell.shelf.as_slice()
            || nodes.len() != self.spell.nodes.len()
        {
            return None;
        }
        let mut changed = nodes.iter().zip(&self.spell.nodes).filter(|(now, before)| now != before);
        let (now, before) = changed.next()?;
        (changed.next().is_none() && now.id == before.id && now.kind == before.kind && now.position == before.position).then(|| now.id.clone())
    }

    pub fn sync_spell(&mut self) {
        self.sync_edits();
    }

    fn wire_refusal(error: &magicklala_canvas::ConnectError) -> String {
        use magicklala_canvas::ConnectError;
        match error {
            ConnectError::FlowMismatch { .. } => "Wire refused: a flow socket and a value socket cannot meet".to_owned(),
            ConnectError::TypeMismatch { from, to } => format!("Wire refused: {from:?} does not fit {to:?}"),
            ConnectError::UnknownSocket { socket, .. } => format!("Wire refused: no socket {socket}"),
            ConnectError::SameNode => "Wire refused: a step cannot wire into itself".to_owned(),
            ConnectError::Duplicate => "Wire refused: that wire already exists".to_owned(),
        }
    }

    pub fn sync_edits(&mut self) -> bool {
        if self.is_running() {
            return false;
        }
        self.sync_listed_outputs();
        self.order_wires();
        self.canvas.notes = self.card_notes();
        let refusal = self.canvas.last_connect_error.take().map(|error| Self::wire_refusal(&error));
        if refusal.is_some() {
            self.wire_error = refusal.clone();
        }
        if self.aside_wires.keys().chain(self.aside_bridges.keys()).chain(self.aside_refused.keys()).any(|id| !self.canvas.is_shelved(id)) {
            self.rebuild_bridges();
        }
        let pulled: Vec<(NodeId, (SocketRef, SocketRef))> = self
            .aside_bridges
            .iter()
            .flat_map(|(id, bridges)| bridges.iter().map(move |bridge| (id.clone(), bridge.clone())))
            .filter(|(_, (from, to))| {
                !self
                    .canvas
                    .wires
                    .iter()
                    .any(|wire| wire.from_node == from.node && wire.from_socket == from.socket && wire.to_node == to.node && wire.to_socket == to.socket)
            })
            .collect();
        for (id, bridge) in pulled {
            if let Some(bridges) = self.aside_bridges.get_mut(&id) {
                bridges.retain(|kept| kept != &bridge);
            }
            self.aside_refused.entry(id).or_default().push(bridge);
        }
        self.aside_bridges.retain(|_, bridges| !bridges.is_empty());
        let mut vars_changed = std::mem::take(&mut self.variables_touched);
        let discrete = std::mem::take(&mut self.discrete_edit);
        let variables_before = self.spell.variables.clone();
        let mut renames: Vec<(String, String)> = Vec::new();
        let mut taken: Vec<String> = self.spell.variables.iter().map(|variable| variable.name.clone()).collect();
        taken.extend(self.canvas.wires.iter().filter(|wire| !wire.ty.is_flow()).map(|wire| wire.to_socket.clone()));
        for (idx, var) in self.spell.variables.iter_mut().enumerate() {
            if let Some(edit) = self.var_name_edits.get(idx) {
                let next_name = edit.text.trim();
                let free = magicklala_nodes::is_template_name(next_name) && !taken.iter().any(|name| name == next_name);
                if free && next_name != var.name {
                    renames.push((var.name.clone(), next_name.to_owned()));
                    var.name = next_name.to_owned();
                    vars_changed = true;
                }
            }
            if let Some(edit) = self.var_default_edits.get(idx)
                && edit.text.trim().is_empty()
                && !matches!(var.ty, SocketType::String | SocketType::Any)
                && var.default.is_some()
            {
                var.default = None;
                vars_changed = true;
            }
            if let Some(edit) = self.var_default_edits.get(idx)
                && edit.text != Self::format_value(var.default.as_ref())
                && let Some(parsed) = Self::try_parse_value(&edit.text, var.ty)
                && var.default.as_ref() != Some(&parsed)
            {
                var.default = Some(parsed);
                vars_changed = true;
            }
        }
        for (old, new) in renames {
            self.carry_rename(&old, &new);
        }
        let edited_variable = (self.spell.variables.len() == variables_before.len())
            .then(|| {
                let mut changed =
                    self.spell.variables.iter().zip(&variables_before).enumerate().filter(|(_, (now, before))| now != before).map(|(index, _)| index);
                let first = changed.next()?;
                changed.next().is_none().then(|| format!("variable:{first}"))
            })
            .flatten();
        let problems = self.variable_errors();
        let problems_changed = problems != self.var_problems;
        self.var_problems = problems;
        let (nodes, wires, subgraphs) = self.rebuilt_parts();
        let shelf = self.shelf_entries();
        if refusal.is_some() && !problems_changed && !vars_changed && wires == self.spell.wires && shelf == self.spell.shelf {
            return true;
        }
        if !problems_changed
            && !vars_changed
            && nodes == self.spell.nodes
            && wires == self.spell.wires
            && subgraphs == self.spell.subgraphs
            && shelf == self.spell.shelf
        {
            self.undo_seen = self.undo_stack.len();
            if self.last_synced.is_none() {
                self.last_synced = Some(Box::new(self.edit_snapshot(None)));
            }
            return false;
        }
        let shelf_changed = shelf != self.spell.shelf;
        let unplaced = |list: &[NodeInstance]| list.iter().map(|node| (node.id.clone(), node.kind.clone(), node.config.clone())).collect::<Vec<_>>();
        let recipe_changed = vars_changed
            || problems_changed
            || wires != self.spell.wires
            || subgraphs != self.spell.subgraphs
            || shelf_changed
            || unplaced(&nodes) != unplaced(&self.spell.nodes);
        if recipe_changed && !self.restoring {
            self.redo_stack.clear();
        }
        if recipe_changed && !self.restoring && self.undo_stack.len() == self.undo_seen {
            let typing = if discrete {
                None
            } else {
                edited_variable.clone().or_else(|| self.typed_node(&nodes, &wires, &subgraphs, &shelf).map(|id| format!("node:{}", id.0)))
            };
            let same_typing = typing.is_some() && matches!(self.undo_stack.last(), Some(UndoEntry::Edit(kept)) if kept.typing == typing);
            match self.last_synced.take() {
                Some(_) if same_typing => {}
                Some(mut before) => {
                    before.typing = typing;
                    self.undo_stack.push(UndoEntry::Edit(before));
                }
                None => self.undo_stack.clear(),
            }
            self.undo_banner = None;
        }
        self.undo_seen = self.undo_stack.len();
        self.last_synced = Some(Box::new(self.edit_snapshot(None)));
        self.spell.shelf = shelf;
        if recipe_changed {
            self.canvas.clear_run_marks();
            self.canvas.last_brew = None;
            self.canvas.brew_total_ms = None;
            self.validation = ValidationState::Unchecked;
            self.canvas.invalid_brew = false;
            self.canvas.validated = false;
            self.hex_findings.clear();
            self.trace.retain(|line| line.kind != "RunRejected");
            if !self.trace.is_empty() {
                self.trace_stale = true;
            }
        }
        self.spell.nodes = nodes;
        self.spell.wires = wires;
        self.spell.subgraphs = subgraphs;
        self.draft_status = None;
        self.publish_status = None;
        self.store_error = None;
        self.wire_error = refusal;
        self.active_release_version = if self.var_problems.is_empty() { self.live_release_matching_recipe() } else { None };
        self.canvas.release_badge = self.active_release_version.map(|version| format!("v{version}"));
        self.dirty = true;
        self.canvas.spell_name = self.spell.name.clone();
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
        StudioSubgraphResolver::load(self.store.as_ref(), &self.brew_graph())
    }

    fn release_version_errors(&self) -> Vec<String> {
        self.brew_graph()
            .nodes
            .iter()
            .filter(|node| node.kind == SUBGRAPH_KIND)
            .filter_map(|node| configured_release_version(node).err().map(|error| format!("node {:?}: {error}", node.id.0)))
            .collect()
    }

    pub fn brew_graph(&self) -> SpellGraph {
        recipe_from(&self.spell, &self.canvas, &self.specs)
    }

    fn wire_home(&self, pair: &(SocketRef, SocketRef)) -> Option<(WireId, usize)> {
        self.wire_homes.iter().find(|(known, _)| known == pair).map(|(_, home)| home.clone())
    }

    fn homed_wires(&self, pairs: &[(SocketRef, SocketRef)]) -> Vec<Wire> {
        let mut wires = stored_wires("parked", pairs);
        for (wire, pair) in wires.iter_mut().zip(pairs) {
            if let Some((id, _)) = self.wire_home(pair) {
                wire.id = id;
            }
        }
        wires
    }

    fn rehome_wire(&mut self, pair: &(SocketRef, SocketRef)) {
        let Some((id, _)) = self.wire_home(pair) else { return };
        self.wire_homes.retain(|(known, _)| known != pair);
        let (from, to) = pair;
        if let Some(wire) = self
            .canvas
            .wires
            .iter_mut()
            .find(|wire| wire.from_node == from.node && wire.from_socket == from.socket && wire.to_node == to.node && wire.to_socket == to.socket)
        {
            wire.id = id;
        }
        self.order_wires();
    }

    fn seed_wire_ledger(&mut self) {
        let mut ledger: Vec<WireId> = self.canvas.wires.iter().map(|wire| wire.id.clone()).collect();
        let mut homes: Vec<(WireId, usize)> = self.wire_homes.iter().map(|(_, home)| home.clone()).collect();
        homes.sort_by_key(|(_, at)| *at);
        for (id, at) in homes {
            if !ledger.contains(&id) {
                let at = at.min(ledger.len());
                ledger.insert(at, id);
            }
        }
        self.wire_ledger = ledger;
    }

    fn order_wires(&mut self) {
        for wire in &self.canvas.wires {
            if !self.wire_ledger.contains(&wire.id) {
                self.wire_ledger.push(wire.id.clone());
            }
        }
        let ledger = &self.wire_ledger;
        self.canvas.wires.sort_by_key(|wire| ledger.iter().position(|known| known == &wire.id));
    }

    fn shelf_entries(&self) -> Vec<magicklala_graph::ShelfEntry> {
        self.canvas
            .nodes
            .iter()
            .filter(|card| self.canvas.is_shelved(&card.id))
            .map(|card| magicklala_graph::ShelfEntry {
                node: card.id.clone(),
                parked: self.aside_wires.get(&card.id).map(|wires| self.homed_wires(wires)).unwrap_or_default(),
                parked_at: self
                    .aside_wires
                    .get(&card.id)
                    .map(|wires| wires.iter().map(|pair| self.wire_home(pair).map_or(u32::MAX, |(_, at)| at as u32)).collect())
                    .unwrap_or_default(),
                bridges: self.aside_bridges.get(&card.id).map(|wires| stored_wires("bridge", wires)).unwrap_or_default(),
                refused: self.aside_refused.get(&card.id).map(|wires| stored_wires("refused", wires)).unwrap_or_default(),
            })
            .collect()
    }

    fn bridge_around(&mut self, id: &NodeId) -> Vec<(SocketRef, SocketRef)> {
        let sources: Vec<SocketRef> = self
            .canvas
            .wires
            .iter()
            .filter(|wire| &wire.to_node == id && wire.ty.is_flow())
            .map(|wire| SocketRef::new(wire.from_node.clone(), wire.from_socket.clone()))
            .collect();
        let targets: Vec<SocketRef> = self
            .canvas
            .wires
            .iter()
            .filter(|wire| &wire.from_node == id && wire.ty.is_flow())
            .map(|wire| SocketRef::new(wire.to_node.clone(), wire.to_socket.clone()))
            .collect();
        let value_sources: Vec<(SocketRef, SocketType)> = self
            .canvas
            .wires
            .iter()
            .filter(|wire| &wire.to_node == id && !wire.ty.is_flow())
            .map(|wire| (SocketRef::new(wire.from_node.clone(), wire.from_socket.clone()), wire.ty))
            .collect();
        let value_targets: Vec<(SocketRef, SocketType)> = self
            .canvas
            .wires
            .iter()
            .filter(|wire| &wire.from_node == id && !wire.ty.is_flow())
            .map(|wire| {
                let want = self
                    .canvas
                    .card(&wire.to_node)
                    .and_then(|card| card.inputs.iter().find(|socket| socket.name == wire.to_socket))
                    .map_or(wire.ty, |socket| socket.ty);
                (SocketRef::new(wire.to_node.clone(), wire.to_socket.clone()), want)
            })
            .collect();
        self.canvas.set_aside(id);
        let mut bridges = Vec::new();
        let straight = sources.len() == 1 && targets.len() == 1;
        for target in targets.iter().filter(|_| straight) {
            if self.is_fed(&target.node, &target.socket) {
                continue;
            }
            for source in &sources {
                if self.canvas.connect_sockets(source, target).is_ok() {
                    bridges.push((source.clone(), target.clone()));
                }
            }
        }
        for (target, want) in &value_targets {
            if self.is_fed(&target.node, &target.socket) {
                continue;
            }
            let mut ranked: Vec<&(SocketRef, SocketType)> = value_sources.iter().filter(|(_, ty)| want.accepts(*ty)).collect();
            ranked.sort_by_key(|(_, ty)| ty != want);
            for (source, _) in ranked {
                if self.canvas.connect_sockets(source, target).is_ok() {
                    bridges.push((source.clone(), target.clone()));
                    break;
                }
            }
        }
        bridges
    }

    fn rebuild_bridges(&mut self) {
        let stale: Vec<(SocketRef, SocketRef)> = self.aside_bridges.drain().flat_map(|(_, wires)| wires).collect();
        self.drop_bridges(&stale);
        let canvas = &self.canvas;
        self.aside_wires.retain(|id, _| canvas.is_shelved(id));
        self.aside_refused.retain(|id, _| canvas.is_shelved(id));
        let mut known: Vec<(SocketRef, SocketRef)> = self
            .canvas
            .wires
            .iter()
            .map(|wire| (SocketRef::new(wire.from_node.clone(), wire.from_socket.clone()), SocketRef::new(wire.to_node.clone(), wire.to_socket.clone())))
            .collect();
        for pair in self.aside_wires.values().flatten() {
            if !known.contains(pair) {
                known.push(pair.clone());
            }
        }
        let shelved: Vec<NodeId> = self.canvas.nodes.iter().filter(|card| self.canvas.is_shelved(&card.id)).map(|card| card.id.clone()).collect();
        for id in shelved {
            let bridges = self.bypass_for(&id, &known);
            if !bridges.is_empty() {
                self.aside_bridges.insert(id, bridges);
            }
        }
    }

    fn output_type(&self, socket: &SocketRef) -> Option<SocketType> {
        self.canvas.card(&socket.node)?.outputs.iter().find(|output| output.name == socket.socket).map(|output| output.ty)
    }

    fn input_type(&self, socket: &SocketRef) -> Option<SocketType> {
        self.canvas.card(&socket.node)?.inputs.iter().find(|input| input.name == socket.socket).map(|input| input.ty)
    }

    fn reach_past_shelf(&self, id: &NodeId, known: &[(SocketRef, SocketRef)], flow: bool, upstream: bool) -> Vec<SocketRef> {
        let mut found = Vec::new();
        let mut seen = vec![id.clone()];
        let mut queue = VecDeque::from([id.clone()]);
        while let Some(node) = queue.pop_front() {
            for (from, to) in known {
                let (near, far) = if upstream { (to, from) } else { (from, to) };
                if near.node != node || self.output_type(from).is_some_and(|ty| ty.is_flow()) != flow {
                    continue;
                }
                if node != *id && !flow {
                    continue;
                }
                if self.canvas.is_shelved(&far.node) {
                    if !seen.contains(&far.node) {
                        seen.push(far.node.clone());
                        queue.push_back(far.node.clone());
                    }
                } else if !found.contains(far) {
                    found.push(far.clone());
                }
            }
        }
        found
    }

    fn bypass_for(&mut self, id: &NodeId, known: &[(SocketRef, SocketRef)]) -> Vec<(SocketRef, SocketRef)> {
        let mut bridges = Vec::new();
        let refused: Vec<(SocketRef, SocketRef)> = self.aside_refused.values().flatten().cloned().collect();
        let welcome = |source: &SocketRef, target: &SocketRef| !refused.iter().any(|(from, to)| from == source && to == target);
        let sources = self.reach_past_shelf(id, known, true, true);
        let targets = self.reach_past_shelf(id, known, true, false);
        if let ([source], [target]) = (sources.as_slice(), targets.as_slice())
            && !self.is_fed(&target.node, &target.socket)
            && welcome(source, target)
            && self.canvas.connect_sockets(source, target).is_ok()
        {
            bridges.push((source.clone(), target.clone()));
        }
        let value_sources: Vec<(SocketRef, SocketType)> =
            self.reach_past_shelf(id, known, false, true).into_iter().filter_map(|source| self.output_type(&source).map(|ty| (source, ty))).collect();
        for target in self.reach_past_shelf(id, known, false, false) {
            if self.is_fed(&target.node, &target.socket) {
                continue;
            }
            let Some(want) = self.input_type(&target) else { continue };
            let mut ranked: Vec<&(SocketRef, SocketType)> = value_sources.iter().filter(|(_, ty)| want.accepts(*ty)).collect();
            ranked.sort_by_key(|(_, ty)| *ty != want);
            for (source, _) in ranked.into_iter().filter(|(source, _)| welcome(source, &target)) {
                if self.canvas.connect_sockets(source, &target).is_ok() {
                    bridges.push((source.clone(), target.clone()));
                    break;
                }
            }
        }
        bridges
    }

    fn drop_bridges(&mut self, bridges: &[(SocketRef, SocketRef)]) {
        self.canvas.wires.retain(|wire| {
            !bridges
                .iter()
                .any(|(from, to)| wire.from_node == from.node && wire.from_socket == from.socket && wire.to_node == to.node && wire.to_socket == to.socket)
        });
    }

    fn hex_findings_for_spell(&self) -> Vec<HexFinding> {
        self.hex_findings_for_root(self.root_subgraph())
    }

    fn hex_findings_for_root(&self, root: SubgraphKey) -> Vec<HexFinding> {
        let variable_findings: Vec<HexFinding> = self
            .variable_errors()
            .into_iter()
            .map(|message| HexFinding { node: None, socket: None, readable: message.clone(), message, mistyped: false })
            .collect();
        let version_findings: Vec<HexFinding> = self
            .brew_graph()
            .nodes
            .iter()
            .filter(|node| node.kind == SUBGRAPH_KIND)
            .filter_map(|node| {
                configured_release_version(node).err().map(|error| {
                    let message = format!("node {:?}: {error}", node.id.0);
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: message.clone(), message, mistyped: false }
                })
            })
            .collect();
        let mut findings: Vec<HexFinding> = variable_findings
            .into_iter()
            .chain(version_findings)
            .chain(self.blank_field_findings())
            .chain(self.stale_list_findings())
            .chain(self.undeclared_variable_findings())
            .chain(self.bound_setting_findings())
            .chain(self.unfilled_word_findings())
            .collect();
        let validator = SpellValidator::new(self.specs.clone());
        if let Err(errors) = validator.validate(&self.brew_graph()) {
            let title_of = |id: &NodeId| self.canvas.display_title(id);
            findings.extend(errors.iter().map(|error| HexFinding::from_validation(error, &self.spell, &title_of)));
        }
        if !findings.is_empty() {
            return findings;
        }
        if self.spell.subgraphs.is_empty() {
            return Vec::new();
        }
        let resolver = match self.subgraph_resolver() {
            Ok(resolver) => resolver,
            Err(error) => return vec![HexFinding::loose(error.to_string())],
        };
        match resolve_subgraphs_from(&self.brew_graph(), root, &resolver, &validator) {
            Ok(_) => Vec::new(),
            Err(error) => vec![HexFinding::loose(error.to_string())],
        }
    }

    fn unfilled_word_findings(&self) -> Vec<HexFinding> {
        let graph = self.brew_graph();
        let fed = |node: &NodeId, socket: &str| graph.wires.iter().any(|wire| &wire.to_node == node && wire.to_socket == socket);
        graph
            .nodes
            .iter()
            .flat_map(|node| {
                let words = |field: &str| node.config.get(field).and_then(serde_json::Value::as_str).map(template_roots).unwrap_or_default();
                let (missing, why): (Vec<String>, &str) = match editable_field(&node.kind) {
                    Some("template") => (
                        words("template")
                            .into_iter()
                            .filter(|root| !fed(&node.id, root) && !graph.variables.iter().any(|variable| &variable.name == root))
                            .collect(),
                        "no wire or variable has that name",
                    ),
                    _ => (Vec::new(), ""),
                };
                missing.into_iter().map(move |root| {
                    let readable = format!("{}: nothing fills {{{{{root}}}}}, {why}", self.canvas.display_title(&node.id));
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: readable.clone(), message: readable, mistyped: false }
                })
            })
            .collect()
    }

    fn undeclared_variable_findings(&self) -> Vec<HexFinding> {
        let graph = self.brew_graph();
        let bound = graph.nodes.iter().flat_map(|node| {
            node.config
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| !magicklala_graph::UNBOUND_SETTINGS.contains(&key.as_str()))
                .filter_map(|(_, value)| magicklala_graph::bound_variable(value))
                .filter(|name| !self.spell.variables.iter().any(|variable| variable.name == *name))
                .map(move |name| {
                    let readable = format!("{}: no variable is named {name}, add it on the Variables tab", self.canvas.display_title(&node.id));
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: readable.clone(), message: readable, mistyped: false }
                })
                .collect::<Vec<_>>()
        });
        let bound: Vec<HexFinding> = bound.collect();
        bound.into_iter().chain(self.undeclared_variable_steps()).collect()
    }

    fn bound_setting_findings(&self) -> Vec<HexFinding> {
        use magicklala_graph::FieldKind;
        let graph = self.brew_graph();
        let mut findings = Vec::new();
        for node in &graph.nodes {
            let Some(spec) = self.specs.iter().find(|spec| spec.kind == node.kind) else { continue };
            for (key, setting) in node.config.as_object().into_iter().flatten().filter(|(key, _)| !magicklala_graph::UNBOUND_SETTINGS.contains(&key.as_str())) {
                let Some(name) = magicklala_graph::bound_variable(setting) else { continue };
                let Some(field) = spec.config.iter().find(|field| &field.key == key) else { continue };
                let Some(variable) = self.spell.variables.iter().find(|variable| variable.name == name) else { continue };
                let value = variable.default.as_ref().map(|value| value.to_json()).filter(|value| !value.is_null());
                let whole = matches!(field.kind, FieldKind::Number { min, .. } if min == Some(1.0))
                    || field.default.as_ref().is_some_and(|default| default.is_i64() || default.is_u64());
                let need = match &field.kind {
                    FieldKind::Number { min, max } => {
                        let noun = if whole { "a whole number" } else { "a number" };
                        Some(match (min, max) {
                            (Some(min), Some(max)) => format!("{noun} from {min} to {max}"),
                            (Some(min), None) => format!("{noun} of at least {min}"),
                            (None, Some(max)) => format!("{noun} of at most {max}"),
                            (None, None) => noun.to_owned(),
                        })
                    }
                    FieldKind::Boolean => Some("true or false".to_owned()),
                    FieldKind::Select { options } => Some(format!("one of {}", options.join(", "))),
                    FieldKind::Json => None,
                    FieldKind::Text | FieldKind::TextArea | FieldKind::Code { .. } | FieldKind::Credential { .. } | FieldKind::Model { .. } => {
                        Some("text".to_owned())
                    }
                };
                let Some(need) = need else { continue };
                let type_fits = match &field.kind {
                    FieldKind::Number { .. } => matches!(variable.ty, SocketType::Int | SocketType::Float | SocketType::Any),
                    FieldKind::Boolean => matches!(variable.ty, SocketType::Bool | SocketType::Any),
                    _ => matches!(variable.ty, SocketType::String | SocketType::Any),
                };
                let value_fits = |value: &serde_json::Value| match &field.kind {
                    FieldKind::Number { min, max } => value
                        .as_f64()
                        .is_some_and(|number| min.is_none_or(|min| number >= min) && max.is_none_or(|max| number <= max) && (!whole || number.fract() == 0.0)),
                    FieldKind::Boolean => value.is_boolean(),
                    FieldKind::Select { options } => value.as_str().is_some_and(|text| options.iter().any(|option| option == text)),
                    _ => value.is_string(),
                };
                let problem = match &value {
                    _ if !type_fits => Some(format!("holds {}", type_words(variable.ty))),
                    None if matches!(field.kind, FieldKind::Number { .. }) && !set_earlier_on_route(&graph, &node.id, name) => Some("has no value".to_owned()),
                    None => None,
                    Some(value) if !value_fits(value) => Some(match value.as_f64().filter(|number| value.is_f64() && number.fract() == 0.0) {
                        Some(whole) => format!("is {whole}"),
                        None => format!("is {value}"),
                    }),
                    Some(_) => None,
                };
                if let Some(problem) = problem {
                    let readable = format!("{}: {} needs {need}, but the variable {name} {problem}", self.canvas.display_title(&node.id), field.label);
                    findings.push(HexFinding { node: Some(node.id.clone()), socket: None, readable: readable.clone(), message: readable, mistyped: false });
                }
            }
        }
        findings
    }
    fn undeclared_variable_steps(&self) -> Vec<HexFinding> {
        self.brew_graph()
            .nodes
            .iter()
            .filter(|node| matches!(node.kind.as_str(), "var/get" | "var/set" | "var/reset"))
            .filter_map(|node| {
                let name = node.config.get("name")?.as_str()?.trim();
                (!name.is_empty() && !self.spell.variables.iter().any(|variable| variable.name == name)).then(|| {
                    let readable = format!("{}: no variable is named {name}, add it on the Variables tab", self.canvas.display_title(&node.id));
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: readable.clone(), message: readable, mistyped: false }
                })
            })
            .collect()
    }

    fn listed_default(&self, kind: &str, field: &str) -> serde_json::Value {
        self.specs
            .iter()
            .find(|spec| spec.kind == kind)
            .and_then(|spec| spec.config.iter().find(|entry| entry.key == field))
            .and_then(|entry| entry.default.clone())
            .unwrap_or_else(|| json!([]))
    }

    fn stale_list_findings(&self) -> Vec<HexFinding> {
        let graph = self.brew_graph();
        let clashes = graph.nodes.iter().filter(|node| listed_field(&node.kind)).flat_map(|node| {
            let fixed = self.specs.iter().find(|spec| spec.kind == node.kind).map(fixed_outputs).unwrap_or_default();
            editable_field(&node.kind)
                .and_then(|field| node.config.get(field))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .filter(|name| fixed.iter().any(|kept| kept == name))
                .map(|name| {
                    let readable = format!("{}: {name} is already an output, pick another name", self.canvas.display_title(&node.id));
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: readable.clone(), message: readable, mistyped: false }
                })
                .collect::<Vec<_>>()
        });
        let clashes: Vec<HexFinding> = clashes.collect();
        clashes
            .into_iter()
            .chain(graph.wires.iter().filter_map(|wire| {
                let node = graph.nodes.iter().find(|node| node.id == wire.from_node && listed_field(&node.kind))?;
                let names = node.config.get(editable_field(&node.kind)?)?.as_array()?;
                let spec = self.specs.iter().find(|spec| spec.kind == node.kind)?;
                let listed = names.iter().any(|name| name.as_str() == Some(wire.from_socket.as_str()));
                (!listed && !fixed_outputs(spec).contains(&wire.from_socket)).then(|| {
                    let readable = format!("{}: {} is no longer listed but still has a wire", self.canvas.display_title(&node.id), wire.from_socket);
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: readable.clone(), message: readable, mistyped: false }
                })
            }))
            .collect()
    }

    fn blank_field_findings(&self) -> Vec<HexFinding> {
        self.brew_graph()
            .nodes
            .iter()
            .filter(|node| node.kind != SUBGRAPH_KIND)
            .filter_map(|node| {
                let key = editable_field(&node.kind)?;
                let field = self.specs.iter().find(|spec| spec.kind == node.kind)?.config.iter().find(|field| field.key == key && field.required)?;
                let blank = node.config.get(key).or(field.default.as_ref()).is_none_or(|value| {
                    value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()) || value.as_array().is_some_and(Vec::is_empty)
                });
                blank.then(|| {
                    let label = field.label.to_lowercase();
                    let readable = if label.ends_with('s') {
                        format!("{}: needs {label}", self.canvas.display_title(&node.id))
                    } else {
                        format!("{}: needs a {label}", self.canvas.display_title(&node.id))
                    };
                    HexFinding { node: Some(node.id.clone()), socket: None, readable: readable.clone(), message: readable, mistyped: false }
                })
            })
            .collect()
    }

    pub fn validate(&mut self) -> &ValidationState {
        if self.is_running() {
            return &self.validation;
        }
        self.sync_spell();
        self.wire_error = None;
        let findings = self.hex_findings_for_spell();
        self.show_findings(findings)
    }

    fn show_findings(&mut self, findings: Vec<HexFinding>) -> &ValidationState {
        let old_run = self.canvas.last_brew.is_some() || self.trace.iter().any(|line| line.kind != "RunRejected");
        if old_run && findings.is_empty() && self.canvas.last_brew.is_some() {
            self.hex_findings.clear();
            self.trace.retain(|line| line.kind != "RunRejected");
            self.validation = ValidationState::Valid;
            self.canvas.validated = true;
            self.canvas.invalid_brew = false;
            return &self.validation;
        }
        if old_run {
            self.canvas.clear_run_marks();
            self.canvas.last_brew = None;
            self.canvas.brew_total_ms = None;
            self.canvas.brew_summary = None;
            self.trace_stale = true;
        }
        for card in &mut self.canvas.nodes {
            if card.status == ExecutionStatus::Failed {
                card.status = ExecutionStatus::Idle;
            }
            for socket in &mut card.inputs {
                socket.flagged = false;
            }
        }
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
                            socket.mistyped = finding.mistyped;
                        }
                    }
                    rejected.push(TraceLine {
                        at_ms: 0,
                        kind: "RunRejected",
                        detail: finding.readable.clone(),
                        status: ExecutionStatus::Failed,
                        node: finding.node.clone(),
                        path: Vec::new(),
                    });
                }
                self.canvas.brew_summary = Some(hex_plaque(&findings, &self.canvas));
                let errors: Vec<String> = findings.into_iter().map(|finding| finding.message).collect();
                if old_run || self.trace.is_empty() || self.trace.iter().all(|line| line.kind == "RunRejected") {
                    self.trace = rejected;
                    self.trace_stale = false;
                }
                ValidationState::Invalid(errors)
            }
        };
        self.canvas.validated = matches!(self.validation, ValidationState::Valid);
        self.canvas.invalid_brew = matches!(self.validation, ValidationState::Invalid(_));
        &self.validation
    }

    fn live_release_matching_recipe(&self) -> Option<u32> {
        let release = self.last_published.as_ref()?;
        same_recipe(&release.graph, &self.brew_graph()).then_some(release.version)
    }

    pub fn is_unchanged_from_live_release(&self) -> bool {
        self.active_release_version.is_some() && self.live_release_matching_recipe().is_some() && calls_only_pinned_spells(&self.brew_graph())
    }

    fn variable_errors(&self) -> Vec<String> {
        let typed: Vec<String> = self.var_name_edits.iter().map(|edit| edit.text.trim().to_owned()).collect();
        let naming = typed.iter().enumerate().filter_map(|(index, name)| {
            if name.is_empty() {
                Some(format!("Variable {} needs a name", index + 1))
            } else if typed[..index].contains(name) {
                Some(format!("{name}: two variables share this name"))
            } else if self.spell.variables.get(index).is_some_and(|variable| &variable.name != name)
                && self.canvas.wires.iter().any(|wire| !wire.ty.is_flow() && &wire.to_socket == name)
            {
                Some(format!("{name}: a step input already uses this name"))
            } else if !magicklala_nodes::is_template_name(name) {
                Some(format!("{name}: a variable name uses only letters, digits, _ and -"))
            } else {
                None
            }
        });
        let defaults = (0..self.spell.variables.len())
            .filter_map(|index| self.var_default_error(index).map(|problem| format!("{}: {problem}", self.spell.variables[index].name)));
        naming.chain(defaults).collect()
    }

    pub fn chat_door(&self) -> Option<&'static str> {
        let graph = self.brew_graph();
        [CHAT_TRIGGER_KIND, "core/on_message"].into_iter().find(|kind| graph.nodes.iter().any(|node| node.kind == *kind))
    }

    pub fn has_recipe_trigger(&self) -> bool {
        self.brew_graph().nodes.iter().any(|node| self.specs.iter().any(|spec| spec.kind == node.kind && spec.is_trigger))
    }

    pub fn run_spell(&mut self) -> bool {
        if !self.has_recipe_trigger() || matches!(self.validation, ValidationState::Invalid(_)) {
            return false;
        }
        let payload = self.brew_payload();
        self.start_run(None, payload, false)
    }

    fn brew_payload(&self) -> serde_json::Value {
        let chat_shaped = self.run_payload.get("channel").is_some() && self.run_payload.get("content").is_some();
        let door = self.trigger(None, &self.run_payload).and_then(|trigger| self.spell.node(&trigger.node).map(|node| node.kind.clone())).unwrap_or_default();
        if !chat_shaped {
            return self.run_payload.clone();
        }
        let content = self.run_payload.get("content").cloned().unwrap_or_default();
        match door.as_str() {
            "event/on_webhook" => serde_json::json!({ "method": "POST", "path": "/hooks/studio", "headers": {}, "query": {}, "body": { "content": content } }),
            "event/on_tick" => {
                let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or_default();
                serde_json::json!({ "tick": self.runs.max(1), "at": format!("unix {at}") })
            }
            "event/on_variable_changed" => {
                let name = self.spell.variables.first().map_or_else(|| "variable".to_owned(), |variable| variable.name.clone());
                serde_json::json!({ "name": name, "value": content })
            }
            "event/on_memory" => serde_json::json!({ "content": content, "payload": {} }),
            _ => self.run_payload.clone(),
        }
    }

    fn trigger(&self, prefer: Option<&str>, payload: &serde_json::Value) -> Option<RunTrigger> {
        let spec_of = |node: &NodeInstance| self.specs.iter().find(|spec| spec.kind == node.kind && spec.is_trigger);
        let graph = self.brew_graph();
        let mut triggers: Vec<(&NodeInstance, &NodeSpec)> = graph.nodes.iter().filter_map(|node| spec_of(node).map(|spec| (node, spec))).collect();
        triggers.sort_by_key(|(node, _)| !graph.wires.iter().any(|wire| wire.from_node == node.id));
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
        self.canvas.brew_summary = None;
        self.trace.clear();
        self.trace_stale = false;
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
                self.trace_stale = false;
                self.active = Some(active);
                self.canvas.running_brew = true;
                self.canvas.last_brew = None;
                self.canvas.brew_total_ms = None;
                true
            }
            Err(error) => {
                if self.trace.is_empty() {
                    self.trace.push(TraceLine {
                        at_ms: 0,
                        kind: "RunRejected",
                        detail: error.clone(),
                        status: ExecutionStatus::Failed,
                        node: None,
                        path: Vec::new(),
                    });
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
        let graph = self.brew_graph();
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
        self.trace.push(TraceLine { at_ms, kind, detail, status: ExecutionStatus::Failed, node: None, path: Vec::new() });
        let left = if kind == "RunCancelled" { ExecutionStatus::Idle } else { ExecutionStatus::Failed };
        self.canvas.nodes.iter_mut().filter(|card| card.status == ExecutionStatus::Running).for_each(|card| card.status = left);
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
        self.canvas.running_brew = self.is_running();
        self.canvas.last_brew = match report.outcome {
            RunOutcome::Finished(status) => Some(status),
            RunOutcome::Invalid => None,
        };
        self.canvas.brew_total_ms = self.trace.last().map(|line| line.at_ms);
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

    pub fn has_open_popover(&self) -> bool {
        self.pantry_drag.is_some()
            || self.canvas.context_menu.is_some()
            || self.model_menu_open
            || self.open_var_type.is_some()
            || self.pending_type_reset.is_some()
            || self.pending_restore_version.is_some()
            || self.pending_switch.is_some()
    }

    pub fn dismiss_topmost(&mut self) -> bool {
        if self.pantry_drag.take().is_some() {
            return true;
        }
        if self.canvas.context_menu.is_some() {
            self.close_context_menu();
            return true;
        }
        if std::mem::take(&mut self.model_menu_open) {
            return true;
        }
        if self.open_var_type.take().is_some() || self.pending_type_reset.take().is_some() {
            return true;
        }
        if self.pending_restore_version.take().is_some() || self.pending_switch.take().is_some() {
            return true;
        }
        if self.canvas.selection.is_some() {
            self.canvas.select(None);
            return true;
        }
        false
    }

    pub fn close_context_menu(&mut self) {
        self.canvas.context_menu = None;
    }

    pub fn open_cauldron(&mut self) {
        self.canvas.mode = WorkbenchMode::Cauldron;
        self.close_context_menu();
    }

    pub fn open_grimoire(&mut self) {
        self.canvas.mode = WorkbenchMode::Grimoire;
        self.close_context_menu();
        let viewport = self.canvas_size();
        self.canvas.fit(viewport);
    }

    pub fn new_empty_spell(&mut self) {
        self.detach_spell_history();
        let taken = |name: &str| self.saved_potions.iter().any(|(saved, _)| saved == name);
        let name = std::iter::once("New Potion".to_owned()).chain((2..).map(|n| format!("New Potion {n}"))).find(|name| !taken(name)).unwrap_or_default();
        self.spell = SpellGraph::new(&name);
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
        self.redo_stack.clear();
        self.undo_banner = None;
        self.validation = ValidationState::Unchecked;
        self.sync_var_edits_from_spell();
        self.last_synced = Some(Box::new(self.edit_snapshot(None)));
        self.seed_wire_ledger();
        self.canvas.notes = self.card_notes();
    }

    pub fn quick_add_from_menu(&mut self) {
        let anchor = self.canvas.context_menu.unwrap_or([self.canvas_size().0 * 0.5, self.canvas_size().1 * 0.5]);
        self.close_context_menu();
        self.spawn_at(QUICK_ADD_KIND, anchor);
    }

    pub fn duplicate_selected_node(&mut self) {
        if self.is_running() {
            return;
        }
        let anchor = self.canvas.context_menu.unwrap_or([self.canvas_size().0 * 0.5, self.canvas_size().1 * 0.5]);
        let Some(card) = self.canvas.selected_card().cloned() else { return self.close_context_menu() };
        let mark = self.undo_stack.len();
        let selected_clone = Some((
            self.configs.get(&card.id).cloned(),
            self.prompts.get(&card.id).map(|e| e.text.clone()),
            self.versions.get(&card.id).map(|e| e.text.clone()),
        ));
        self.close_context_menu();
        let at = [anchor[0] + 24.0, anchor[1] + 24.0];
        let spawned = if self.canvas.is_shelved(&card.id) { self.place_ingredient(&card.kind, at, false) } else { self.spawn_at(&card.kind, at) };
        if let Some(new_id) = &spawned
            && let Some(copy) = self.canvas.nodes.iter_mut().find(|other| &other.id == new_id)
        {
            for socket in &card.inputs {
                copy.ensure_socket(&socket.name, socket.ty, false);
            }
            for socket in &card.outputs {
                copy.ensure_socket(&socket.name, socket.ty, true);
            }
        }
        if let Some(new_id) = spawned
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
            self.fold_undo_since(mark);
        }
    }

    pub fn set_aside_selected(&mut self) {
        self.close_context_menu();
        if let Some(id) = self.canvas.selection.clone() {
            self.toss(magicklala_canvas::Toss::Aside(id));
        }
    }

    pub fn pour_selected_in(&mut self) {
        self.close_context_menu();
        if let Some(id) = self.canvas.selection.clone() {
            self.toss(magicklala_canvas::Toss::IntoPot(id));
        }
    }

    pub fn delete_selected_node(&mut self) {
        if self.is_running() {
            return;
        }
        self.close_context_menu();
        let Some(selected_id) = self.canvas.selection.clone() else { return };
        let attached_wires: Vec<WireModel> = self.canvas.wires.iter().filter(|w| w.from_node == selected_id || w.to_node == selected_id).cloned().collect();
        let shelf = self.canvas.is_shelved(&selected_id).then(|| ParkedShelf {
            parked: self.aside_wires.get(&selected_id).cloned().unwrap_or_default(),
            bridges: self.aside_bridges.get(&selected_id).cloned().unwrap_or_default(),
            refused: self.aside_refused.get(&selected_id).cloned().unwrap_or_default(),
        });
        let at = self.canvas.nodes.iter().position(|card| card.id == selected_id).unwrap_or(self.canvas.nodes.len());
        let call = self.spell.subgraphs.iter().find(|reference| reference.node_id == selected_id).cloned();
        let title = self.canvas.display_title(&selected_id);
        let bridges = self.bridge_around(&selected_id);
        self.canvas.select(Some(selected_id.clone()));
        if let Some(removed) = self.canvas.remove_selected_node() {
            self.aside_wires.remove(&removed.id);
            self.aside_bridges.remove(&removed.id);
            self.aside_refused.remove(&removed.id);
            let prompt = self.prompts.remove(&removed.id);
            let version = self.versions.remove(&removed.id);
            let config = self.configs.remove(&removed.id);
            self.undo_stack.push(UndoEntry::Node { at, card: Box::new(removed), prompt, version, config, wires: attached_wires, bridges, shelf, call });
            self.undo_banner = Some(format!("Removed {title}"));
            self.validation = ValidationState::Unchecked;
            self.dirty = true;
            self.sync_spell();
        }
    }

    fn restore_edit(&mut self, before: EditSnapshot) {
        let EditSnapshot {
            nodes,
            wires,
            shelved,
            configs,
            prompts,
            versions,
            subgraphs,
            variables,
            var_name_edits,
            var_default_edits,
            aside_wires,
            aside_bridges,
            aside_refused,
            wire_homes,
            wire_ledger,
            ..
        } = before;
        self.aside_wires = aside_wires;
        self.aside_bridges = aside_bridges;
        self.aside_refused = aside_refused;
        self.wire_homes = wire_homes;
        self.wire_ledger = wire_ledger;
        self.spell.variables = variables;
        self.var_name_edits = var_name_edits;
        self.var_default_edits = var_default_edits;
        self.variables_touched = true;
        self.canvas.nodes = self.where_they_stand(nodes);
        self.canvas.wires = wires;
        let spots: Vec<(NodeId, [f32; 2])> = self.canvas.nodes.iter().map(|card| (card.id.clone(), card.position)).collect();
        for (id, spot) in spots {
            self.canvas.move_node(&id, spot);
        }
        self.canvas.shelved = shelved;
        self.configs = configs;
        self.prompts = prompts;
        self.versions = versions;
        self.spell.subgraphs = subgraphs;
        let kept = self.canvas.selection.clone().filter(|id| self.canvas.card(id).is_some());
        self.canvas.select(kept);
        self.validation = ValidationState::Unchecked;
        self.canvas.validated = false;
        self.dirty = true;
        self.sync_spell();
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn redo(&mut self) {
        if self.is_running() {
            return;
        }
        let Some(after) = self.redo_stack.pop() else { return };
        let now = self.edit_snapshot(None);
        self.restoring = true;
        self.restore_edit(after);
        self.restoring = false;
        self.undo_stack.push(UndoEntry::Edit(Box::new(now)));
        self.undo_seen = self.undo_stack.len();
        self.undo_banner = None;
    }

    pub fn undo(&mut self) {
        if self.is_running() {
            return;
        }
        let Some(entry) = self.undo_stack.pop() else { return };
        let now = self.edit_snapshot(None);
        self.undo_banner = None;
        self.restoring = true;
        self.apply_undo(entry);
        self.restoring = false;
        self.redo_stack.push(now);
    }

    fn apply_undo(&mut self, entry: UndoEntry) {
        match entry {
            UndoEntry::Aside { id, wires, bridges } => {
                self.drop_bridges(&bridges);
                let stale: Vec<(SocketRef, SocketRef)> = self.aside_bridges.drain().flat_map(|(_, wires)| wires).collect();
                self.drop_bridges(&stale);
                self.canvas.unshelve(&id);
                for (from, to) in &wires {
                    let _ = self.canvas.connect_sockets(from, to);
                }
                for pair in &wires {
                    self.rehome_wire(pair);
                }
                self.aside_wires.remove(&id);
                self.rebuild_bridges();
                self.validation = ValidationState::Unchecked;
                self.sync_spell();
            }
            UndoEntry::Node { at, card, prompt, version, config, wires, bridges, shelf, call } => {
                self.drop_bridges(&bridges);
                let id = card.id.clone();
                if let Some(call) = call.filter(|call| !self.spell.subgraphs.iter().any(|kept| kept.node_id == call.node_id)) {
                    self.spell.subgraphs.push(call);
                }
                if let Some(prompt) = prompt {
                    self.prompts.insert(id.clone(), prompt);
                }
                if let Some(version) = version {
                    self.versions.insert(id.clone(), version);
                }
                if let Some(config) = config {
                    self.configs.insert(id.clone(), config);
                }
                self.canvas.restore_node(*card, wires, at);
                if let Some(ParkedShelf { parked, bridges, refused }) = shelf {
                    self.canvas.shelve(&id);
                    if !parked.is_empty() {
                        self.aside_wires.insert(id.clone(), parked);
                    }
                    if !refused.is_empty() {
                        self.aside_refused.insert(id.clone(), refused);
                    }
                    if !bridges.is_empty() {
                        self.aside_bridges.insert(id.clone(), bridges);
                    }
                }
                self.validation = ValidationState::Unchecked;
                self.dirty = true;
                self.sync_spell();
            }
            UndoEntry::VariableAdded { index } => {
                if index < self.spell.variables.len() {
                    self.spell.variables.remove(index);
                    if index < self.var_name_edits.len() {
                        self.var_name_edits.remove(index);
                    }
                    if index < self.var_default_edits.len() {
                        self.var_default_edits.remove(index);
                    }
                    self.open_var_type = None;
                    self.pending_type_reset = None;
                    self.variables_touched = true;
                    self.validation = ValidationState::Unchecked;
                    self.dirty = true;
                    self.sync_spell();
                }
            }
            UndoEntry::Edit(before) => self.restore_edit(*before),
            UndoEntry::Recipe { nodes, wires, shelved } => {
                self.canvas.nodes = self.where_they_stand(nodes);
                self.canvas.wires = wires;
                let spots: Vec<(NodeId, [f32; 2])> = self.canvas.nodes.iter().map(|card| (card.id.clone(), card.position)).collect();
                for (id, spot) in spots {
                    self.canvas.move_node(&id, spot);
                }
                self.canvas.shelved = shelved;
                self.canvas.select(None);
                self.validation = ValidationState::Unchecked;
                self.canvas.validated = false;
                self.dirty = true;
                self.sync_spell();
            }
            UndoEntry::VariableType { index, variable, default_edit } => {
                if index < self.spell.variables.len() {
                    self.spell.variables[index].ty = variable.ty;
                    self.spell.variables[index].default = variable.default;
                    if let Some(edit) = self.var_default_edits.get_mut(index) {
                        *edit = default_edit;
                    }
                    self.open_var_type = None;
                    self.pending_type_reset = None;
                    self.variables_touched = true;
                    self.validation = ValidationState::Unchecked;
                    self.canvas.validated = false;
                    self.dirty = true;
                    self.sync_spell();
                }
            }
            UndoEntry::Variable { index, variable, name_edit, default_edit } => {
                let at = index.min(self.spell.variables.len());
                self.spell.variables.insert(at, variable);
                let name_at = at.min(self.var_name_edits.len());
                self.var_name_edits.insert(name_at, name_edit);
                let def_at = at.min(self.var_default_edits.len());
                self.var_default_edits.insert(def_at, default_edit);
                self.variables_touched = true;
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
            let message = format!("The database passed with --db could not be opened: {error}");
            return Err(self.refuse(message));
        }
        if self.store.as_ref().is_some_and(StudioStore::is_persistent) {
            return Ok(());
        }
        Err(self.refuse(format!("Pass --db <file.sqlite> to {action} on disk")))
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
            let message = format!("Cannot publish an invalid potion: {}", errors.join("; "));
            self.store_error = Some(message);
        }
    }

    pub fn publish(&mut self) -> Result<u32, StudioError> {
        if self.is_running() {
            return Err(self.refuse("Publish waits until the brew ends".to_owned()));
        }
        self.sync_spell();
        if self.canvas.nodes.is_empty() {
            return Err(self.refuse("Cannot publish an empty potion".to_owned()));
        }
        if self.is_unchanged_from_live_release() {
            return Err(self.refuse("No changes since the live release".to_owned()));
        }
        self.publish_status = None;
        self.require_disk("publish releases")?;
        let next = match self.next_release_version() {
            Ok(next) => next,
            Err(error) => return Err(self.refuse(error.to_string())),
        };
        let target = (self.root_spell(), Some(next));
        let findings = self.hex_findings_for_root(target);
        if !findings.is_empty() {
            let readable: Vec<String> = findings.iter().map(|finding| finding.readable.clone()).collect();
            let message = format!("Cannot publish an invalid potion: {}", readable.join("; "));
            self.show_findings(findings);
            return Err(self.refuse(message));
        }
        self.validation = ValidationState::Valid;
        let recipe = self.brew_graph();
        let draft = self.spell.clone();
        let store = self.store.as_mut().ok_or_else(|| StudioError::Store("no studio store".to_owned()))?;
        if let Err(error) = store.save_draft(&draft) {
            self.store_error = Some(error.to_string());
            return Err(error);
        }
        let published = store.publish(&recipe);
        let release = published.inspect_err(|error| self.store_error = Some(error.to_string()))?;
        self.refresh_releases(&release);
        self.store_error = None;
        self.draft_status = None;
        self.dirty = false;
        self.active_release_version = Some(release.version);
        let version = release.version;
        self.canvas.release_badge = Some(format!("v{version}"));
        self.publish_status = Some(format!("Published v{version}"));
        self.last_published = Some(release);
        self.refresh_saved_potions();
        Ok(version)
    }

    pub fn save_draft(&mut self) -> Result<SpellRecordId, StudioError> {
        if self.is_running() {
            return Err(self.refuse("Save waits until the brew ends".to_owned()));
        }
        self.sync_spell();
        self.draft_status = None;
        self.publish_status = None;
        self.require_disk("save drafts")?;
        let version_errors = [self.variable_errors(), self.release_version_errors()].concat();
        if !version_errors.is_empty() {
            return Err(self.refuse(format!("Cannot save the draft: {}", version_errors.join("; "))));
        }
        let store = self.store.as_mut().ok_or_else(|| StudioError::Store("no studio store".to_owned()))?;
        let saved = store.save_draft(&self.spell);
        let record = saved.inspect_err(|error| self.store_error = Some(error.to_string()))?;
        self.store_error = None;
        self.dirty = false;
        self.active_release_version = self.live_release_matching_recipe();
        self.canvas.release_badge = self.active_release_version.map(|version| format!("v{version}"));
        self.draft_status = Some(DRAFT_SAVED.to_owned());
        self.refresh_saved_potions();
        Ok(record.id)
    }

    pub fn spell_record_id(&self) -> Option<SpellRecordId> {
        self.store.as_ref().and_then(StudioStore::spell_record_id)
    }

    pub fn send_chat(&mut self) {
        let message = self.chat_input.text.trim().to_owned();
        let Some(door) = self.chat_door() else { return };
        if message.is_empty() || self.is_running() {
            return;
        }
        self.chat.push(ChatMessage { from_user: true, text: message.clone() });
        self.chat_input = TextEdit::new("");
        self.start_run(Some(door), json!({ "channel": STUDIO_CHANNEL, "sender": STUDIO_SENDER, "content": message }), true);
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

pub fn studio_key(app: &mut StudioApp, key: &rune::KeyEvent, _: &mut rune::Tx) -> bool {
    if key.key == rune::Key::Escape {
        return app.dismiss_topmost();
    }
    let on_workbench = matches!(app.right_tab, RightTab::Inspector | RightTab::Trace) && !app.has_open_popover();
    match &key.key {
        rune::Key::Delete | rune::Key::Backspace if on_workbench && app.canvas.selection.is_some() && !app.is_running() => {
            app.delete_selected_node();
            true
        }
        _ if !app.has_open_popover() && key.is_command('z') && key.mods.shift && !app.redo_stack.is_empty() && !app.is_running() => {
            app.redo();
            true
        }
        _ if !app.has_open_popover() && key.is_command('z') && !key.mods.shift && !app.undo_stack.is_empty() && !app.is_running() => {
            app.undo();
            true
        }
        _ => false,
    }
}

fn spec(config: StudioConfig) -> AppSpec<StudioApp> {
    remember_run_timeout(config.run_timeout);
    AppSpec {
        fonts: STUDIO_FONTS,
        families: STUDIO_FAMILIES,
        setup: Some(StudioApp::set_waker),
        poll: Some(|app, _, _| app.poll_background() | app.sync_edits()),
        on_key: Some(studio_key),
        ..AppSpec::new(TITLE, config.size, StudioApp::demo, studio_view)
    }
}

fn remember_run_timeout(timeout: Duration) {
    if let Ok(mut slot) = LAUNCH_RUN_TIMEOUT.lock() {
        *slot = Some(timeout);
    }
}

mod view;
pub use view::{potion_mood, prompt_editor_height, prompt_font, studio_status, studio_view};

fn trace_line(event: &TraceEvent, at_ms: u64, canvas: &CanvasState) -> TraceLine {
    let path = event.subgraph_path();
    let along_path = |leaf: &str| path.iter().map(|step| step.0.as_str()).chain(std::iter::once(leaf)).collect::<Vec<_>>().join(" › ");
    let title = |id: &NodeId| {
        if !path.is_empty() {
            return along_path(&id.0);
        }
        canvas.display_title(id)
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
    TraceLine { at_ms, kind, detail, status, node, path: path.to_vec() }
}

pub fn numbered_steps(trace: &[TraceLine]) -> Vec<(String, &TraceLine)> {
    let steps: Vec<&TraceLine> = trace.iter().filter(|line| line.kind == "NodeFinished" || line.kind == "NodeFailed").collect();
    let mut tops: Vec<&NodeId> = Vec::new();
    for line in &steps {
        let Some(node) = line.node.as_ref() else { continue };
        let top = line.path.first().filter(|_| !line.path.is_empty());
        if top.is_none() && !tops.contains(&node) {
            tops.push(node);
        }
    }
    for line in &steps {
        if let Some(parent) = line.path.first()
            && !tops.contains(&parent)
        {
            tops.push(parent);
        }
    }
    let mut numbered = Vec::new();
    for (index, top) in tops.iter().enumerate() {
        let number = index + 1;
        numbered.extend(steps.iter().filter(|line| line.path.is_empty() && line.node.as_ref() == Some(top)).map(|line| (number.to_string(), *line)));
        let mut children: Vec<(&[NodeId], &NodeId)> = Vec::new();
        for line in steps.iter().filter(|line| line.path.first() == Some(top)) {
            let Some(node) = line.node.as_ref() else { continue };
            let key = (line.path.as_slice(), node);
            if !children.contains(&key) {
                children.push(key);
            }
            let child = children.iter().position(|known| known == &key).unwrap_or(0) + 1;
            numbered.push((format!("{number}.{child}"), *line));
        }
    }
    numbered
}

pub fn top_step_count(trace: &[TraceLine]) -> usize {
    numbered_steps(trace).iter().filter(|(number, _)| !number.contains('.')).map(|(number, _)| number.clone()).collect::<std::collections::BTreeSet<_>>().len()
}

fn type_words(ty: SocketType) -> &'static str {
    match ty {
        SocketType::Bool => "true or false",
        SocketType::Int => "a whole number",
        SocketType::Float => "a number",
        SocketType::String => "text",
        _ => "structured data",
    }
}

pub fn template_roots(text: &str) -> Vec<String> {
    magicklala_nodes::template_roots(text)
}

fn default_prompt(kind: &str) -> String {
    match kind {
        "ai/generate_text" => "You are a helpful agent. Answer the user clearly and kindly.".to_owned(),
        "code/rhai_script" => "content.to_upper()".to_owned(),
        "text/template" => "{{content}}".to_owned(),
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

fn hex_plaque(findings: &[HexFinding], canvas: &CanvasState) -> String {
    let title_of = |id: &NodeId| canvas.card(id).map(|_| canvas.display_title(id));
    match findings.first() {
        Some(HexFinding { socket: Some(socket), mistyped: true, .. }) => format!("Hexed · wrong type \"{socket}\""),
        Some(HexFinding { socket: Some(socket), .. }) => format!("Hexed · missing \"{socket}\""),
        Some(HexFinding { node: Some(node), .. }) => format!("Hexed · {}", title_of(node).unwrap_or_else(|| node.0.clone())),
        Some(HexFinding { node: None, readable, .. }) => format!("Hexed · {readable}"),
        None => "Hexed".to_owned(),
    }
}

fn demo_spell() -> SpellGraph {
    let mut spell = SpellGraph::new("Support Agent");
    let place =
        |id: &str, kind: &str, position: [f32; 2], config: serde_json::Value| NodeInstance { id: NodeId::from(id), kind: kind.to_owned(), position, config };
    spell.nodes = vec![
        place("trigger", "event/on_message", [0.0, 108.0], serde_json::Value::Null),
        place("recall", "memory/search", [250.0, 12.0], json!({ "limit": "{{max_memories}}" })),
        place("search", "knowledge/search", [500.0, 208.0], serde_json::Value::Null),
        place(
            "llm",
            "ai/generate_text",
            [750.0, 86.0],
            json!({ "provider": "openai", "model": "gpt-4o-mini", "temperature": "{{temperature}}", "prompt": default_prompt("ai/generate_text") }),
        ),
        place("reply", "io/send_message", [1000.0, 128.0], serde_json::Value::Null),
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
        VariableDef { name: "max_memories".to_owned(), ty: SocketType::Int, default: Some(Value::Int(5)), exposed: true },
        VariableDef { name: "temperature".to_owned(), ty: SocketType::Float, default: Some(Value::Float(0.4)), exposed: true },
    ]
}
