use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use magicklala_graph::{
    CredentialResolver, EngineServices, EventEmitter, NodeCatalog, NodeContext, NodeError, NodeExecutor, NodeOutcome, NodeSpec, RunId, RunTrigger, SocketSpec,
    SocketType, SpellEngine, SpellGraph, SpellId, SpellValidator, SubgraphResolver, TraceEvent, Value,
};
use magicklala_nodes::{HttpLlmGateway, InMemoryKnowledge, InMemoryMemory, NodeRegistry, default_registry};

struct StdoutEmitter;

impl EventEmitter for StdoutEmitter {
    fn emit(&self, event: TraceEvent) {
        if let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
        }
    }
}

const CREDENTIALS_FILE_ENV: &str = "MAGICKLALA_CREDENTIALS_FILE";

struct FileCredentials(BTreeMap<String, String>);

#[async_trait]
impl CredentialResolver for FileCredentials {
    async fn resolve(&self, credential_id: &str) -> Result<String, NodeError> {
        self.0.get(credential_id).cloned().ok_or_else(|| NodeError::Service(format!("credential {credential_id} is not in {CREDENTIALS_FILE_ENV}")))
    }
}

fn credentials_from_env() -> Result<Option<Arc<dyn CredentialResolver>>, String> {
    let Some(path) = std::env::var(CREDENTIALS_FILE_ENV).ok().filter(|path| !path.is_empty()) else {
        return Ok(None);
    };
    let source = std::fs::read_to_string(&path).map_err(|error| format!("cannot read {path}: {error}"))?;
    let secrets: BTreeMap<String, String> = serde_json::from_str(&source).map_err(|error| format!("{path} is not a JSON object of strings: {error}"))?;
    Ok(Some(Arc::new(FileCredentials(secrets))))
}

struct FileSubgraphs(BTreeMap<(SpellId, Option<u32>), SpellGraph>);

impl SubgraphResolver for FileSubgraphs {
    fn resolve(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<SpellGraph> {
        self.0.get(&(spell_id, release_version)).cloned()
    }
}

struct SubgraphFile {
    release_version: Option<u32>,
    path: String,
}

fn release_file(raw: &str) -> Result<SubgraphFile, String> {
    let (version, path) = raw.split_once('=').ok_or("--subgraph-release needs <version>=<spell.json>")?;
    let version =
        version.parse::<u32>().ok().filter(|version| *version > 0).ok_or_else(|| format!("--subgraph-release version {version:?} is not a positive number"))?;
    Ok(SubgraphFile { release_version: Some(version), path: path.to_owned() })
}

fn load_subgraphs(files: &[SubgraphFile]) -> Result<Option<Arc<dyn SubgraphResolver>>, String> {
    if files.is_empty() {
        return Ok(None);
    }
    let mut graphs = BTreeMap::new();
    for SubgraphFile { release_version, path } in files {
        let source = std::fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
        let graph: SpellGraph = serde_json::from_str(&source).map_err(|error| format!("invalid subgraph {path}: {error}"))?;
        graphs.insert((graph.id, *release_version), graph);
    }
    Ok(Some(Arc::new(FileSubgraphs(graphs))))
}

struct TallyProbe(NodeSpec);

impl TallyProbe {
    fn new() -> Self {
        Self(NodeSpec {
            kind: "probe/tally".to_owned(),
            title: "Tally Probe".to_owned(),
            category: "probe".to_owned(),
            description: "Counts its own runs in the hits variable and announces it before joining inputs a and b".to_owned(),
            inputs: vec![SocketSpec::input("a", SocketType::String), SocketSpec::input("b", SocketType::String)],
            outputs: vec![SocketSpec::output("text", SocketType::String)],
            config: Vec::new(),
            is_trigger: false,
        })
    }
}

fn text_of(value: Option<Value>) -> String {
    match value {
        Some(Value::String(text)) => text,
        Some(other) => other.to_json().to_string(),
        None => String::new(),
    }
}

#[async_trait]
impl NodeExecutor for TallyProbe {
    fn spec(&self) -> &NodeSpec {
        &self.0
    }

    async fn execute(&self, ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
        let hits = match ctx.variable("hits") {
            Some(Value::Int(previous)) => previous + 1,
            _ => 1,
        };
        ctx.set_variable("hits", Value::Int(hits))?;
        ctx.events().emit(TraceEvent::StreamChunk {
            run_id: ctx.run_id(),
            node_id: ctx.node().id.clone(),
            delta: format!("entered {hits}"),
            subgraph_path: Vec::new(),
        });
        let a = text_of(ctx.input("a").await?);
        let b = text_of(ctx.input("b").await?);
        Ok(NodeOutcome::default().with_output("text", Value::String(format!("{a}+{b}"))))
    }
}

fn registry(probe_nodes: bool) -> NodeRegistry {
    let mut registry = default_registry();
    if probe_nodes {
        registry.register(Arc::new(TallyProbe::new()));
    }
    registry
}

struct Arguments {
    spell: String,
    step_limit: Option<u64>,
    subgraphs: Vec<SubgraphFile>,
    probe_nodes: bool,
    payload: serde_json::Value,
    trigger: Option<String>,
    run_id: Option<RunId>,
    validate_only: bool,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut spell = None;
    let mut payload = serde_json::Value::Null;
    let mut trigger = None;
    let mut run_id = None;
    let mut validate_only = false;
    let mut step_limit = None;
    let mut subgraphs = Vec::new();
    let mut probe_nodes = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--payload" => {
                let raw = args.next().ok_or("--payload needs a JSON value")?;
                payload = serde_json::from_str(&raw).map_err(|error| format!("--payload is not JSON: {error}"))?;
            }
            "--trigger" => trigger = Some(args.next().ok_or("--trigger needs a node id")?),
            "--run-id" => {
                let raw = args.next().ok_or("--run-id needs a UUID")?;
                run_id = Some(serde_json::from_value::<RunId>(serde_json::Value::String(raw)).map_err(|error| format!("--run-id is not a UUID: {error}"))?);
            }
            "--validate" => validate_only = true,
            "--step-limit" => {
                let raw = args.next().ok_or("--step-limit needs a number")?;
                step_limit = Some(raw.parse::<u64>().map_err(|error| format!("--step-limit is not a number: {error}"))?);
            }
            "--subgraph" => subgraphs.push(SubgraphFile { release_version: None, path: args.next().ok_or("--subgraph needs a spell file")? }),
            "--subgraph-release" => subgraphs.push(release_file(&args.next().ok_or("--subgraph-release needs <version>=<spell.json>")?)?),
            "--probe-nodes" => probe_nodes = true,
            other if spell.is_none() => spell = Some(other.to_owned()),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    let spell = spell.ok_or(
        "usage: run_spell <spell.json> [--payload <json>] [--trigger <node>] [--run-id <uuid>] [--step-limit <n>] [--subgraph <spell.json>]... [--subgraph-release <version>=<spell.json>]... [--probe-nodes] [--validate]",
    )?;
    Ok(Arguments { spell, step_limit, subgraphs, probe_nodes, payload, trigger, run_id, validate_only })
}

async fn run(arguments: Arguments) -> Result<ExitCode, String> {
    let source = std::fs::read_to_string(&arguments.spell).map_err(|error| format!("cannot read {}: {error}", arguments.spell))?;
    let graph: SpellGraph = serde_json::from_str(&source).map_err(|error| format!("invalid spell: {error}"))?;
    let registry = Arc::new(registry(arguments.probe_nodes));
    if arguments.validate_only {
        let errors = SpellValidator::new(registry.specs()).validate(&graph).err().unwrap_or_default();
        println!("{}", serde_json::json!({ "valid": errors.is_empty(), "errors": errors }));
        return Ok(if errors.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(2) });
    }
    let trigger_node = match arguments.trigger {
        Some(node) => node,
        None => graph
            .nodes
            .iter()
            .find(|node| registry.get(&node.kind).is_some_and(|executor| executor.spec().is_trigger))
            .map(|node| node.id.0.clone())
            .ok_or("spell has no trigger node")?,
    };
    let services = EngineServices {
        llm: Some(Arc::new(match credentials_from_env()? {
            Some(resolver) => HttpLlmGateway::from_env().with_credential_resolver(resolver),
            None => HttpLlmGateway::from_env(),
        })),
        memory: Some(Arc::new(InMemoryMemory::new())),
        knowledge: Some(Arc::new(InMemoryKnowledge::new())),
    };
    let allow_private_http = std::env::var("MAGICKLALA_ALLOW_PRIVATE_HTTP").is_ok_and(|flag| flag == "1");
    let engine = SpellEngine::new(registry, services).with_private_http(allow_private_http);
    let engine = match arguments.run_id {
        Some(run_id) => engine.with_run_id(run_id),
        None => engine,
    };
    let engine = match arguments.step_limit {
        Some(step_limit) => engine.with_step_limit(step_limit),
        None => engine,
    };
    let engine = match load_subgraphs(&arguments.subgraphs)? {
        Some(resolver) => engine.with_subgraph_resolver(resolver),
        None => engine,
    };
    let trigger = RunTrigger { node: trigger_node.as_str().into(), socket: "out".to_owned(), payload: Value::from(arguments.payload) };
    match engine.run(&graph, trigger, Arc::new(StdoutEmitter)).await {
        Ok(summary) => {
            println!("{}", serde_json::json!({ "run_summary": summary }));
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("{error}");
            Ok(ExitCode::from(1))
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let outcome = match parse_arguments() {
        Ok(arguments) => run(arguments).await,
        Err(error) => Err(error),
    };
    outcome.unwrap_or_else(|error| {
        eprintln!("{error}");
        ExitCode::from(64)
    })
}
