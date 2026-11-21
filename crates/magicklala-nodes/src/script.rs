use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use magicklala_graph::{NodeContext, NodeError, NodeOutcome, Value};
use rhai::{Dynamic, Engine, Scope};
use tokio_util::sync::CancellationToken;

use crate::nodes::{config_list, config_text, config_u64, value};

const DEFAULT_MAX_OPERATIONS: u64 = 100_000;
const MAX_ALLOWED_OPERATIONS: u64 = 10_000_000;

fn sandboxed_engine(max_operations: u64, cancellation: CancellationToken, aborted: Arc<AtomicBool>) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(max_operations);
    engine.set_max_expr_depths(64, 32);
    engine.set_max_call_levels(32);
    engine.set_max_string_size(1 << 20);
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(10_000);
    engine.set_max_modules(0);
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    engine.on_progress(move |_| {
        if cancellation.is_cancelled() {
            aborted.store(true, Ordering::SeqCst);
            Some(Dynamic::UNIT)
        } else {
            None
        }
    });
    engine
}

fn evaluate(
    script: &str,
    bindings: Vec<(String, serde_json::Value)>,
    max_operations: u64,
    cancellation: CancellationToken,
) -> Result<serde_json::Value, NodeError> {
    let aborted = Arc::new(AtomicBool::new(false));
    let engine = sandboxed_engine(max_operations, cancellation, Arc::clone(&aborted));
    let mut scope = Scope::new();
    for (name, json) in bindings {
        let dynamic = rhai::serde::to_dynamic(json).map_err(|error| NodeError::Script(error.to_string()))?;
        scope.push_dynamic(name, dynamic);
    }
    let evaluated = engine.eval_with_scope::<Dynamic>(&mut scope, script);
    if aborted.load(Ordering::SeqCst) {
        return Err(NodeError::Cancelled);
    }
    let result = evaluated.map_err(|error| NodeError::Script(error.to_string()))?;
    rhai::serde::from_dynamic(&result).map_err(|error| NodeError::Script(error.to_string()))
}

pub(crate) async fn run_rhai(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let script =
        config_text(ctx, "script").filter(|script| !script.trim().is_empty()).ok_or_else(|| NodeError::InvalidConfig("script is required".to_owned()))?;
    let max_operations = config_u64(ctx, "max_operations", DEFAULT_MAX_OPERATIONS).clamp(1, MAX_ALLOWED_OPERATIONS);
    let mut bindings = Vec::new();
    for name in config_list(ctx, "inputs").unwrap_or_default() {
        let bound = value(ctx, &name).await?.to_json();
        bindings.push((name, bound));
    }
    let cancellation = ctx.cancellation();
    let result = tokio::task::spawn_blocking(move || evaluate(&script, bindings, max_operations, cancellation))
        .await
        .map_err(|error| NodeError::Script(error.to_string()))??;
    let mut outcome = NodeOutcome::fire("out");
    if let Some(fields) = result.as_object() {
        for (key, field) in fields {
            outcome = outcome.with_output(key, Value::from(field.clone()));
        }
    }
    Ok(outcome.with_output("result", Value::from(result)))
}
