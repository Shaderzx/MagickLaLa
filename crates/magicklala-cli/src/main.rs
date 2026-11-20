use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use magicklala_graph::{EngineServices, NodeCatalog, NodeId, RunTrigger, SpellEngine, SpellGraph, SpellValidator, TraceEvent, Value};
use magicklala_nodes::default_registry;
use magicklala_runtime::{AgentSupervisor, InMemoryEventBus};
use magicklala_server::{AppState, ServerConfig};

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
    },
    Spell {
        #[command(subcommand)]
        command: SpellCommand,
    },
    Agent {
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
    },
    Validate {
        file: PathBuf,
    },
    Specs,
}

#[derive(Subcommand)]
enum AgentCommand {
    List,
    Start { id: uuid_arg::AgentArg },
    Stop { id: uuid_arg::AgentArg },
}

mod uuid_arg {
    #[derive(Debug, Clone, Copy)]
    pub struct AgentArg(pub uuid::Uuid);

    impl std::str::FromStr for AgentArg {
        type Err = uuid::Error;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            value.parse().map(AgentArg)
        }
    }
}

struct PrintTrace;

impl magicklala_graph::EventEmitter for PrintTrace {
    fn emit(&self, event: TraceEvent) {
        if let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
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
        Command::Studio { snap: Some(path) } => Ok(magicklala_studio::render_headless_snapshot(&path)?),
        Command::Studio { snap: None } => Ok(magicklala_studio::run_studio(magicklala_studio::StudioConfig::default())?),
        Command::Serve { bind, database_url } => block_on(serve(ServerConfig { bind, database_url })),
        Command::Spell { command } => spell(command),
        Command::Agent { command } => block_on(agent(command)),
    }
}

fn block_on(future: impl Future<Output = CliResult>) -> CliResult {
    tokio::runtime::Runtime::new()?.block_on(future)
}

fn app_state() -> AppState {
    AppState { registry: Arc::new(default_registry()), supervisor: Arc::new(AgentSupervisor::new(Arc::new(InMemoryEventBus::default()))) }
}

async fn serve(config: ServerConfig) -> CliResult {
    eprintln!("magicklala server listening on http://{}", config.bind);
    Ok(magicklala_server::serve(config, app_state()).await?)
}

fn load(file: &PathBuf) -> Result<SpellGraph, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(file)?)?)
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
        SpellCommand::Run { file, stream: _, trace: _, input } => {
            let graph = load(&file)?;
            let trigger_node = graph.nodes.first().map(|node| node.id.clone()).unwrap_or_else(|| NodeId::from("trigger"));
            let engine = SpellEngine::new(registry, EngineServices::default());
            let trigger = RunTrigger { node: trigger_node, socket: "out".to_owned(), payload: Value::from(input) };
            let summary = block_on_value(engine.run(&graph, trigger, Arc::new(PrintTrace)))??;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
    }
}

fn block_on_value<T>(future: impl Future<Output = T>) -> Result<T, std::io::Error> {
    Ok(tokio::runtime::Runtime::new()?.block_on(future))
}

async fn agent(command: AgentCommand) -> CliResult {
    let state = app_state();
    match command {
        AgentCommand::List => {
            for (id, status) in state.supervisor.list().await {
                println!("{id}\t{}", serde_json::to_string(&status)?);
            }
            Ok(())
        }
        AgentCommand::Start { id } => Ok(state.supervisor.start(id.0).await?),
        AgentCommand::Stop { id } => Ok(state.supervisor.stop(id.0).await?),
    }
}
