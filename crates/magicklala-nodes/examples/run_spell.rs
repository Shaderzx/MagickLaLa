use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use magicklala_graph::{
    CredentialResolver, EngineServices, EventEmitter, NodeCatalog, NodeError, RunId, RunTrigger, SpellEngine, SpellGraph, SpellValidator, TraceEvent, Value,
};
use magicklala_nodes::{HttpLlmGateway, InMemoryKnowledge, InMemoryMemory, default_registry};

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

struct Arguments {
    spell: String,
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
            other if spell.is_none() => spell = Some(other.to_owned()),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    let spell = spell.ok_or("usage: run_spell <spell.json> [--payload <json>] [--trigger <node>] [--run-id <uuid>] [--validate]")?;
    Ok(Arguments { spell, payload, trigger, run_id, validate_only })
}

async fn run(arguments: Arguments) -> Result<ExitCode, String> {
    let source = std::fs::read_to_string(&arguments.spell).map_err(|error| format!("cannot read {}: {error}", arguments.spell))?;
    let graph: SpellGraph = serde_json::from_str(&source).map_err(|error| format!("invalid spell: {error}"))?;
    let registry = Arc::new(default_registry());
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
    let trigger = RunTrigger { node: trigger_node.as_str().into(), socket: "out".to_owned(), payload: Value::from(arguments.payload) };
    match engine.run(&graph, trigger, Arc::new(StdoutEmitter)).await {
        Ok(_) => Ok(ExitCode::SUCCESS),
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
