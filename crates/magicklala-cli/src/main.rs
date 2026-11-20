use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use magicklala_graph::{EngineServices, NodeCatalog, NodeId, RunTrigger, SpellEngine, SpellGraph, SpellId, SpellValidator, SubgraphKey, TraceEvent, Value};
use magicklala_nodes::{HttpLlmGateway, InMemoryKnowledge, InMemoryMemory, LlmGatewayConfig, NodeRegistry, default_registry};
use magicklala_server::{ALLOWED_HOSTS_ENV, API_TOKEN_ENV, AppState, ProjectSubgraphResolver, ServerConfig, Wiring};
use magicklala_store::{AesGcmVault, CryptoVault, MASTER_KEY_ENV, ProjectId, Repository, SpellRecord, SqliteStore, is_in_memory_database_url};

const VAULT_KEY_ENV: &str = "MAGICKLALA_VAULT_KEY";
const EMBEDDER_URL_ENV: &str = "MAGICKLALA_EMBEDDER_URL";
const EMBEDDER_MODEL_ENV: &str = "MAGICKLALA_EMBEDDER_MODEL";
const DATABASE_ALIAS_ENV: &str = "MAGICKLALA_DB";
const DEFAULT_DATABASE_URL: &str = "sqlite://magicklala.db";

#[derive(Parser)]
#[command(name = "magicklala", version, about = "MagickLaLa visual AI agent studio and runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, env = "MAGICKLALA_BIND", default_value = "127.0.0.1:7777")]
        bind: std::net::SocketAddr,
        #[arg(long, visible_alias = "db", env = "MAGICKLALA_DATABASE_URL")]
        database_url: Option<String>,
        #[arg(long)]
        insecure_no_auth: bool,
    },
    Studio {
        #[arg(long, value_name = "PATH")]
        snap: Option<PathBuf>,
        #[arg(long, value_name = "PATH", env = "MAGICKLALA_DB")]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = 1280.0)]
        width: f32,
        #[arg(long, default_value_t = 800.0)]
        height: f32,
    },
    Spell {
        #[command(subcommand)]
        command: SpellCommand,
    },
    Agent {
        #[arg(long, global = true, env = "MAGICKLALA_SERVER", default_value = "http://127.0.0.1:7777")]
        server: String,
        #[arg(long, global = true, env = "MAGICKLALA_API_TOKEN", hide_env_values = true)]
        token: Option<String>,
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Subcommand)]
enum SpellCommand {
    Run {
        file: PathBuf,
        #[arg(long)]
        stream: bool,
        #[arg(long)]
        trace: bool,
        #[arg(long, default_value = "")]
        input: String,
        #[arg(long, env = "MAGICKLALA_LLM_BASE_URL")]
        llm_base_url: Option<String>,
        #[arg(long, env = "MAGICKLALA_MODEL")]
        model: Option<String>,
        #[arg(long, value_name = "PATH_OR_URL", env = "MAGICKLALA_DB")]
        db: Option<String>,
        #[arg(long, value_name = "RECORD_ID")]
        spell_id: Option<uuid::Uuid>,
    },
    Validate {
        file: PathBuf,
    },
    Specs,
}

#[derive(Subcommand)]
enum AgentCommand {
    List,
    Start { id: uuid::Uuid },
    Stop { id: uuid::Uuid },
}

struct TerminalTrace {
    stream: bool,
    trace: bool,
}

impl TerminalTrace {
    fn write_trace_line(&self, event: &TraceEvent) {
        let Ok(line) = serde_json::to_string(event) else { return };
        if self.stream {
            let mut stderr = std::io::stderr().lock();
            writeln!(stderr, "{line}").ok();
        } else {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{line}").ok();
            stdout.flush().ok();
        }
    }
}

impl magicklala_graph::EventEmitter for TerminalTrace {
    fn emit(&self, event: TraceEvent) {
        if self.stream
            && let TraceEvent::StreamChunk { delta, .. } = &event
        {
            let mut stdout = std::io::stdout().lock();
            write!(stdout, "{delta}").ok();
            stdout.flush().ok();
        }
        if self.trace {
            self.write_trace_line(&event);
        }
    }
}

type CliResult = Result<(), Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).init();
    match dispatch(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: Cli) -> CliResult {
    match cli.command {
        Command::Studio { snap, width, height, db } => {
            let config = magicklala_studio::StudioConfig { size: (width, height), db_path: db, ..Default::default() };
            match snap {
                Some(path) => Ok(magicklala_studio::render_headless_snapshot_with_config(&path, config, magicklala_studio::Theme::Dark)?),
                None => Ok(magicklala_studio::run_studio(config)?),
            }
        }
        Command::Serve { bind, database_url, insecure_no_auth } => {
            let config = ServerConfig::from_env(bind, database_url_or_default(database_url));
            refuse_open_bind(&config, insecure_no_auth)?;
            block_on(serve(config))
        }
        Command::Spell { command } => spell(command),
        Command::Agent { server, token, command } => block_on(agent(server, token, command)),
    }
}

fn block_on(future: impl Future<Output = CliResult>) -> CliResult {
    tokio::runtime::Runtime::new()?.block_on(future)
}

fn database_url_or_default(argument: Option<String>) -> String {
    let raw = argument.or_else(|| std::env::var(DATABASE_ALIAS_ENV).ok()).map(|url| url.trim().to_owned()).filter(|url| !url.is_empty());
    match raw {
        None => DEFAULT_DATABASE_URL.to_owned(),
        Some(url) if url.starts_with("sqlite:") => url,
        Some(path) => format!("sqlite://{path}"),
    }
}

fn spell_db_file(raw: &Path) -> PathBuf {
    let text = raw.to_string_lossy();
    let stripped = text.strip_prefix("sqlite://").or_else(|| text.strip_prefix("sqlite:")).unwrap_or(&text);
    PathBuf::from(stripped)
}

fn refuse_open_bind(config: &ServerConfig, insecure_no_auth: bool) -> CliResult {
    if config.bind.ip().is_loopback() || config.access.requires_token() || insecure_no_auth {
        return Ok(());
    }
    Err(format!(
        "refusing to serve on non-loopback address {} without {API_TOKEN_ENV}: anyone who can reach it would control every agent; set {API_TOKEN_ENV}, bind to 127.0.0.1, or pass --insecure-no-auth to accept the risk",
        config.bind
    )
    .into())
}

fn vault(database_url: &str) -> Result<Arc<dyn CryptoVault>, Box<dyn std::error::Error>> {
    let fallback = std::env::var(VAULT_KEY_ENV).ok().filter(|key| !key.trim().is_empty());
    match AesGcmVault::from_env_or_key(fallback.as_deref()) {
        Ok(vault) => Ok(Arc::new(vault)),
        Err(_) if is_in_memory_database_url(database_url) => {
            eprintln!("warning: neither {MASTER_KEY_ENV} nor {VAULT_KEY_ENV} is set; the in-memory store seals credentials with an ephemeral key");
            Ok(Arc::new(AesGcmVault::new(&AppState::ephemeral_vault_key())))
        }
        Err(_) => Err(format!(
            "refusing to start: {database_url} is a file-backed database, so set {MASTER_KEY_ENV} or {VAULT_KEY_ENV} to a stable 32-byte key (64 hex characters); an ephemeral key would make every sealed credential unreadable after a restart"
        )
        .into()),
    }
}

fn embedder() -> Arc<dyn magicklala_embedder::Embedder> {
    match std::env::var(EMBEDDER_URL_ENV).ok().filter(|url| !url.is_empty()) {
        Some(url) => {
            let model = std::env::var(EMBEDDER_MODEL_ENV).unwrap_or_else(|_| "text-embedding-3-small".to_owned());
            let dimensions = std::env::var("MAGICKLALA_EMBEDDER_DIMENSIONS").ok().and_then(|value| value.parse().ok()).unwrap_or(1_536);
            let embedder = magicklala_embedder::HttpEmbedder::new(&url, model, dimensions);
            match std::env::var("MAGICKLALA_EMBEDDER_API_KEY").ok().filter(|key| !key.is_empty()) {
                Some(key) => Arc::new(embedder.with_api_key(key)),
                None => Arc::new(embedder),
            }
        }
        None => Arc::new(magicklala_embedder::LocalHashEmbedder::default()),
    }
}

async fn serve(config: ServerConfig) -> CliResult {
    let vault = vault(&config.database_url)?;
    let store = SqliteStore::connect(&config.database_url).await?;
    store.migrate().await?;
    let store: Arc<dyn Repository> = Arc::new(store);
    let wiring =
        Wiring::new(Arc::new(default_registry()), store, vault).with_embedder(embedder()).with_access(config.access.clone()).with_spell_runs(config.runs);
    let state = AppState::wire(wiring);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let address = listener.local_addr()?;
    if !address.ip().is_loopback() && !config.access.requires_token() {
        eprintln!(
            "warning: serving on non-loopback address {address} without {API_TOKEN_ENV} because --insecure-no-auth was passed; anyone who can reach it controls every agent"
        );
    }
    if address.ip().is_unspecified() && config.access.allowed_hosts.is_empty() {
        eprintln!("warning: bound to {address}; only loopback Host headers are accepted until {ALLOWED_HOSTS_ENV} lists the names clients use");
    }
    let booted = state.boot_enabled_agents().await;
    for (agent, reason) in &booted.failed {
        eprintln!("warning: enabled agent {agent} did not boot: {reason}");
    }
    for (webhook, path) in &booted.refused_unsigned_webhooks {
        eprintln!(
            "warning: unsigned webhook {webhook} ({path}) is refused while {API_TOKEN_ENV} is set; delete it and create a signed webhook with secret_credential_id"
        );
    }
    eprintln!("magicklala server listening on http://{address} (store: sqlite {}, booted {} agent(s))", config.database_url, booted.started.len());
    Ok(magicklala_server::serve_on(listener, state).await?)
}

fn load(file: &Path) -> Result<SpellGraph, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(file).map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    Ok(serde_json::from_str(&text).map_err(|error| format!("{} is not a spell: {error}", file.display()))?)
}

fn draft_root((project, record): &(ProjectId, SpellRecord)) -> (ProjectId, SubgraphKey) {
    (*project, (SpellId(record.id.0), None))
}

async fn stored_spell(
    store: &SqliteStore,
    db: &str,
    graph: &SpellGraph,
    spell_id: Option<uuid::Uuid>,
) -> Result<(ProjectId, SubgraphKey), Box<dyn std::error::Error>> {
    let mut saved = Vec::new();
    for project in store.list_projects().await? {
        saved.extend(store.list_spells(project.id).await?.into_iter().map(|record| (project.id, record)));
    }
    if let Some(id) = spell_id {
        let chosen = saved.iter().find(|(_, record)| record.id.0 == id).ok_or_else(|| format!("spell record {id} was not found in {db}"))?;
        let record = &chosen.1;
        if record.graph.id != graph.id {
            return Err(format!("spell record {} has graph id {}, which does not match file graph id {}", record.id.0, record.graph.id.0, graph.id.0).into());
        }
        return Ok(draft_root(chosen));
    }
    let matching: Vec<&(ProjectId, SpellRecord)> = saved.iter().filter(|(_, record)| record.graph.id == graph.id).collect();
    match matching.as_slice() {
        [] => Err(format!("spell {} is not stored in {db}; save it there through the server or Studio, or run without --db", graph.id.0).into()),
        [only] => Ok(draft_root(only)),
        many => {
            let records: Vec<String> = many.iter().map(|(_, record)| format!("{} ({})", record.id.0, record.name)).collect();
            Err(format!("multiple saved spells share graph id {} in {db}: {}; pass --spell-id <record-id> to pick one", graph.id.0, records.join(", ")).into())
        }
    }
}

async fn stored_subgraphs(
    db: &str,
    graph: &SpellGraph,
    spell_id: Option<uuid::Uuid>,
    registry: &NodeRegistry,
) -> Result<(ProjectSubgraphResolver, SubgraphKey), Box<dyn std::error::Error>> {
    let url = database_url_or_default(Some(db.to_owned()));
    if !is_in_memory_database_url(&url) && !spell_db_file(Path::new(db)).exists() {
        return Err(format!("database {db} does not exist; pass the SQLite file magicklala serve uses").into());
    }
    let store = SqliteStore::connect(&url).await?;
    store.migrate().await?;
    let (project, root_key) = stored_spell(&store, db, graph, spell_id).await?;
    let resolver = ProjectSubgraphResolver::load(&store, project, graph).await?;
    resolver.verify_from(graph, root_key, &SpellValidator::new(registry.specs()))?;
    Ok((resolver, root_key))
}

fn override_model(graph: &mut SpellGraph, model: &str) {
    for node in graph.nodes.iter_mut().filter(|node| node.kind.starts_with("ai/")) {
        if let Some(config) = node.config.as_object_mut() {
            config.insert("model".to_owned(), serde_json::Value::String(model.to_owned()));
        }
    }
}

fn cli_payload(input: &str) -> Value {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(json @ serde_json::Value::Object(_)) => Value::from(json),
        _ => Value::Event(std::collections::BTreeMap::from([
            ("channel".to_owned(), Value::String("cli".to_owned())),
            ("sender".to_owned(), Value::String("cli".to_owned())),
            ("content".to_owned(), Value::String(input.to_owned())),
        ])),
    }
}

fn spell(command: SpellCommand) -> CliResult {
    let registry = Arc::new(default_registry());
    match command {
        SpellCommand::Specs => {
            println!("{}", serde_json::to_string_pretty(&registry.specs())?);
            Ok(())
        }
        SpellCommand::Validate { file } => {
            let graph = load(&file)?;
            match SpellValidator::new(registry.specs()).validate(&graph) {
                Ok(()) => {
                    println!("valid: {}", graph.name);
                    Ok(())
                }
                Err(errors) => Err(errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n").into()),
            }
        }
        SpellCommand::Run { file, stream, trace, input, llm_base_url, model, db, spell_id } => {
            let db = db.as_deref().map(str::trim).filter(|db| !db.is_empty());
            if spell_id.is_some() && db.is_none() {
                return Err(format!("--spell-id requires --db <file.sqlite> (or {DATABASE_ALIAS_ENV})").into());
            }
            let mut graph = load(&file)?;
            let runtime = tokio::runtime::Runtime::new()?;
            let resolver = match db {
                Some(db) => Some(runtime.block_on(stored_subgraphs(db, &graph, spell_id, &registry))?),
                None => None,
            };
            if let Some(model) = model.as_deref().filter(|model| !model.is_empty()) {
                override_model(&mut graph, model);
            }
            let trigger_node = graph
                .nodes
                .iter()
                .find(|node| registry.get(&node.kind).is_some_and(|executor| executor.spec().is_trigger))
                .or_else(|| graph.nodes.first())
                .map(|node| node.id.clone())
                .unwrap_or_else(|| NodeId::from("trigger"));
            let gateway_config = LlmGatewayConfig { base_url: llm_base_url.filter(|url| !url.is_empty()), ..LlmGatewayConfig::from_env() };
            let services = EngineServices {
                llm: Some(Arc::new(HttpLlmGateway::new(gateway_config))),
                memory: Some(Arc::new(InMemoryMemory::new())),
                knowledge: Some(Arc::new(InMemoryKnowledge::new())),
            };
            let engine = match resolver {
                Some((resolver, root_key)) => SpellEngine::new(registry, services).with_subgraph_resolver(Arc::new(resolver)).with_root_subgraph(root_key),
                None => SpellEngine::new(registry, services),
            };
            let trigger = RunTrigger { node: trigger_node, socket: "out".to_owned(), payload: cli_payload(&input) };
            let emitter = Arc::new(TerminalTrace { stream, trace });
            let summary = runtime.block_on(engine.run(&graph, trigger, emitter))?;
            let rendered = serde_json::to_string_pretty(&summary)?;
            if stream {
                println!();
                eprintln!("{rendered}");
            } else {
                println!("{rendered}");
            }
            Ok(())
        }
    }
}

async fn api_call(request: reqwest::RequestBuilder) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let response = request.send().await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        Ok(body)
    } else {
        let message = body.get("error").and_then(serde_json::Value::as_str).unwrap_or("request failed");
        let code = body.get("code").and_then(serde_json::Value::as_str).unwrap_or("unknown");
        Err(format!("{status} {code}: {message}").into())
    }
}

fn describe_agent(agent: &serde_json::Value) -> String {
    let id = agent.get("id").and_then(serde_json::Value::as_str).unwrap_or("?");
    let state = agent.pointer("/status/state").and_then(serde_json::Value::as_str).unwrap_or("unknown");
    let name = agent.get("name").and_then(serde_json::Value::as_str).unwrap_or("");
    format!("{id}\t{state}\t{name}")
}

fn authorized(request: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    match token.map(str::trim).filter(|token| !token.is_empty()) {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

async fn agent(server: String, token: Option<String>, command: AgentCommand) -> CliResult {
    let client = reqwest::Client::new();
    let base = format!("{}/api/v1", server.trim_end_matches('/'));
    let token = token.as_deref();
    match command {
        AgentCommand::List => {
            let agents = api_call(authorized(client.get(format!("{base}/agents")), token)).await?;
            for agent in agents.as_array().into_iter().flatten() {
                println!("{}", describe_agent(agent));
            }
        }
        AgentCommand::Start { id } => println!("{}", describe_agent(&api_call(authorized(client.post(format!("{base}/agents/{id}/start")), token)).await?)),
        AgentCommand::Stop { id } => println!("{}", describe_agent(&api_call(authorized(client.post(format!("{base}/agents/{id}/stop")), token)).await?)),
    }
    Ok(())
}
