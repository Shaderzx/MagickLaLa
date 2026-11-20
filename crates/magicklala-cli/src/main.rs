use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use magicklala_graph::{EngineServices, NodeCatalog, NodeId, RunTrigger, SpellEngine, SpellGraph, SpellValidator, TraceEvent, Value};
use magicklala_nodes::{HttpLlmGateway, InMemoryKnowledge, InMemoryMemory, LlmGatewayConfig, default_registry};
use magicklala_server::{AppState, ServerConfig, Wiring};
use magicklala_store::{AesGcmVault, CryptoVault, MASTER_KEY_ENV, Repository, SqliteStore};

const VAULT_KEY_ENV: &str = "MAGICKLALA_VAULT_KEY";
const EMBEDDER_URL_ENV: &str = "MAGICKLALA_EMBEDDER_URL";
const EMBEDDER_MODEL_ENV: &str = "MAGICKLALA_EMBEDDER_MODEL";

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
        #[arg(long, env = "MAGICKLALA_DATABASE_URL", default_value = "sqlite://magicklala.db")]
        database_url: String,
    },
    Studio {
        #[arg(long, value_name = "PATH")]
        snap: Option<PathBuf>,
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
        Command::Studio { snap: Some(path), width, height } => Ok(magicklala_studio::render_headless_snapshot_sized(&path, (width, height))?),
        Command::Studio { snap: None, width, height } => Ok(magicklala_studio::run_studio(magicklala_studio::StudioConfig { size: (width, height) })?),
        Command::Serve { bind, database_url } => block_on(serve(ServerConfig { bind, database_url })),
        Command::Spell { command } => spell(command),
        Command::Agent { server, command } => block_on(agent(server, command)),
    }
}

fn block_on(future: impl Future<Output = CliResult>) -> CliResult {
    tokio::runtime::Runtime::new()?.block_on(future)
}

fn vault() -> Result<Arc<dyn CryptoVault>, Box<dyn std::error::Error>> {
    let fallback = std::env::var(VAULT_KEY_ENV).ok().filter(|key| !key.trim().is_empty());
    match AesGcmVault::from_env_or_key(fallback.as_deref()) {
        Ok(vault) => Ok(Arc::new(vault)),
        Err(_) => {
            eprintln!(
                "warning: neither {MASTER_KEY_ENV} nor {VAULT_KEY_ENV} is set; credentials are sealed with an ephemeral key and cannot be revealed after a restart"
            );
            Ok(Arc::new(AesGcmVault::new(&AppState::ephemeral_vault_key())))
        }
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
    let store = SqliteStore::connect(&config.database_url).await?;
    store.migrate().await?;
    let store: Arc<dyn Repository> = Arc::new(store);
    let state = AppState::wire(Wiring::new(Arc::new(default_registry()), store, vault()?).with_embedder(embedder()));
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    eprintln!("magicklala server listening on http://{} (store: sqlite {})", listener.local_addr()?, config.database_url);
    Ok(magicklala_server::serve_on(listener, state).await?)
}

fn load(file: &Path) -> Result<SpellGraph, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(file).map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    Ok(serde_json::from_str(&text).map_err(|error| format!("{} is not a spell: {error}", file.display()))?)
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
        SpellCommand::Run { file, stream, trace, input, llm_base_url, model } => {
            let mut graph = load(&file)?;
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
            let engine = SpellEngine::new(registry, services);
            let trigger = RunTrigger { node: trigger_node, socket: "out".to_owned(), payload: cli_payload(&input) };
            let emitter = Arc::new(TerminalTrace { stream, trace });
            let summary = tokio::runtime::Runtime::new()?.block_on(engine.run(&graph, trigger, emitter))?;
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

async fn agent(server: String, command: AgentCommand) -> CliResult {
    let client = reqwest::Client::new();
    let base = format!("{}/api/v1", server.trim_end_matches('/'));
    match command {
        AgentCommand::List => {
            let agents = api_call(client.get(format!("{base}/agents"))).await?;
            for agent in agents.as_array().into_iter().flatten() {
                println!("{}", describe_agent(agent));
            }
        }
        AgentCommand::Start { id } => println!("{}", describe_agent(&api_call(client.post(format!("{base}/agents/{id}/start"))).await?)),
        AgentCommand::Stop { id } => println!("{}", describe_agent(&api_call(client.post(format!("{base}/agents/{id}/stop"))).await?)),
    }
    Ok(())
}
