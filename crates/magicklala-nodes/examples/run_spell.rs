use std::process::ExitCode;
use std::sync::Arc;

use magicklala_graph::{EngineServices, EventEmitter, NodeCatalog, RunTrigger, SpellEngine, SpellGraph, SpellValidator, TraceEvent, Value};
use magicklala_nodes::{HttpLlmGateway, InMemoryKnowledge, InMemoryMemory, default_registry};

struct StdoutEmitter;

impl EventEmitter for StdoutEmitter {
    fn emit(&self, event: TraceEvent) {
        if let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
        }
    }
}

struct Arguments {
    spell: String,
    payload: serde_json::Value,
    trigger: Option<String>,
    validate_only: bool,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut spell = None;
    let mut payload = serde_json::Value::Null;
    let mut trigger = None;
    let mut validate_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--payload" => {
                let raw = args.next().ok_or("--payload needs a JSON value")?;
                payload = serde_json::from_str(&raw).map_err(|error| format!("--payload is not JSON: {error}"))?;
            }
            "--trigger" => trigger = Some(args.next().ok_or("--trigger needs a node id")?),
            "--validate" => validate_only = true,
            other if spell.is_none() => spell = Some(other.to_owned()),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    Ok(Arguments { spell: spell.ok_or("usage: run_spell <spell.json> [--payload <json>] [--trigger <node>] [--validate]")?, payload, trigger, validate_only })
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
        llm: Some(Arc::new(HttpLlmGateway::from_env())),
        memory: Some(Arc::new(InMemoryMemory::new())),
        knowledge: Some(Arc::new(InMemoryKnowledge::new())),
    };
    let allow_private_http = std::env::var("MAGICKLALA_ALLOW_PRIVATE_HTTP").is_ok_and(|flag| flag == "1");
    let engine = SpellEngine::new(registry, services).with_private_http(allow_private_http);
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
