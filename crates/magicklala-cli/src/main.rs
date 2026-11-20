use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use magicklala_graph::{EngineServices, NodeCatalog, NodeId, RunTrigger, SpellEngine, SpellGraph, SpellValidator, TraceEvent, Value};
use magicklala_nodes::default_registry;
use magicklala_runtime::{AgentSupervisor, InMemoryEventBus};
use magicklala_server::{AppState, ServerConfig};

const VAULT_KEY_ENV: &str = "MAGICKLALA_VAULT_KEY";

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
        Command::Studio { snap: Some(path), .. } => Ok(magicklala_studio::render_headless_snapshot(&path)?),
        Command::Studio { snap: None, width, height } => Ok(magicklala_studio::run_studio(magicklala_studio::StudioConfig { size: (width, height) })?),
        Command::Serve { bind, database_url } => block_on(serve(ServerConfig { bind, database_url })),
        Command::Spell { command } => spell(command),
        Command::Agent { server, command } => block_on(agent(server, command)),
    }
}

fn block_on(future: impl Future<Output = CliResult>) -> CliResult {
    tokio::runtime::Runtime::new()?.block_on(future)
}

fn vault_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    match std::env::var(VAULT_KEY_ENV) {
        Ok(encoded) => {
            let bytes = hex::decode(encoded.trim())?;
            Ok(<[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| format!("{VAULT_KEY_ENV} must be 64 hex characters (32 bytes)"))?)
        }
        Err(_) => {
            eprintln!("warning: {VAULT_KEY_ENV} is not set; credentials are sealed with an ephemeral key and cannot be revealed after a restart");
            Ok(AppState::ephemeral_vault_key())
        }
    }
}

async fn serve(config: ServerConfig) -> CliResult {
    let supervisor = Arc::new(AgentSupervisor::new(Arc::new(InMemoryEventBus::default())));
    let state = AppState::in_memory(Arc::new(default_registry()), supervisor, vault_key()?);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    eprintln!("magicklala server listening on http://{} (store: in-memory, requested {})", listener.local_addr()?, config.database_url);
    Ok(magicklala_server::serve_on(listener, state).await?)
}

fn load(file: &Path) -> Result<SpellGraph, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(file).map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    Ok(serde_json::from_str(&text).map_err(|error| format!("{} is not a spell: {error}", file.display()))?)
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
            let graph = load(&file)?;
            if let Some(url) = &llm_base_url {
                eprintln!("llm: {url} model {}", model.as_deref().unwrap_or("default"));
            }
            let trigger_node = graph.nodes.first().map(|node| node.id.clone()).unwrap_or_else(|| NodeId::from("trigger"));
            let engine = SpellEngine::new(registry, EngineServices::default());
            let trigger = RunTrigger { node: trigger_node, socket: "out".to_owned(), payload: Value::from(input) };
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
