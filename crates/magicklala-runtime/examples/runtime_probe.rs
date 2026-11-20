use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use magicklala_connectors::{
    Connector, ConnectorConfig, ConnectorKind, DiscordConnector, InboundEvent, OutboundAction, SlackConnector, TickConnector, WEBHOOK_SIGNATURE_HEADER,
    WebhookConnector,
};
use magicklala_graph::{NodeInstance, NodeSpec, SocketSpec, SocketType, SpellGraph, SpellValidator, TraceEvent};
use magicklala_runtime::{AgentStatus, AgentSupervisor, CopilotEngine, EventBus, GraphPatchOp, InMemoryEventBus, RuntimeEvent};
use serde_json::{Value, json};
use uuid::Uuid;

const PROBE_DEADLINE: Duration = Duration::from_secs(30);

type ProbeResult = Result<Value, String>;

fn flag(args: &[String], name: &str) -> Result<String, String> {
    args.windows(2).find(|pair| pair[0] == name).map(|pair| pair[1].clone()).ok_or_else(|| format!("missing {name}"))
}

fn stdin_text() -> Result<String, String> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|error| error.to_string())?;
    Ok(text)
}

fn connector_config(kind: ConnectorKind, settings: Value) -> ConnectorConfig {
    ConnectorConfig { id: Uuid::now_v7(), agent_id: Uuid::now_v7(), kind, enabled: true, credential_id: None, settings }
}

async fn tick(args: &[String]) -> ProbeResult {
    let interval_ms: u64 = flag(args, "--interval-ms")?.parse().map_err(|_| "bad --interval-ms".to_owned())?;
    let count: usize = flag(args, "--count")?.parse().map_err(|_| "bad --count".to_owned())?;
    let connector = TickConnector::new(connector_config(ConnectorKind::Tick, json!({ "interval_ms": interval_ms }))).map_err(|error| error.to_string())?;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    connector.start(sender).await.map_err(|error| error.to_string())?;
    let mut ticks = Vec::new();
    while ticks.len() < count {
        let event =
            tokio::time::timeout(PROBE_DEADLINE, receiver.recv()).await.map_err(|_| "tick never arrived".to_owned())?.ok_or("tick stream closed early")?;
        ticks.push(event);
    }
    connector.stop().await.map_err(|error| error.to_string())?;
    let mut buffered_before_stop = 0_u32;
    let closed = loop {
        match tokio::time::timeout(PROBE_DEADLINE, receiver.recv()).await {
            Ok(Some(_)) => buffered_before_stop += 1,
            Ok(None) => break true,
            Err(_) => break false,
        }
    };
    Ok(json!({ "ticks": ticks, "status_after_stop": connector.status(), "stream_closed_after_stop": closed, "buffered_before_stop": buffered_before_stop }))
}

async fn webhook(args: &[String]) -> ProbeResult {
    let secret = flag(args, "--secret")?;
    let signature = flag(args, "--signature")?;
    let body = stdin_text()?;
    let connector = WebhookConnector::new(connector_config(ConnectorKind::Webhook, json!({})), Some(secret));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    connector.start(sender).await.map_err(|error| error.to_string())?;
    let outcome = match connector.receive(body.as_bytes(), Some(&signature)).await {
        Ok(_) => {
            let event = receiver.recv().await.ok_or("accepted event never reached the agent")?;
            json!({ "accepted": true, "header": WEBHOOK_SIGNATURE_HEADER, "event": event })
        }
        Err(error) => json!({ "accepted": false, "header": WEBHOOK_SIGNATURE_HEADER, "error": error.to_string() }),
    };
    connector.stop().await.map_err(|error| error.to_string())?;
    Ok(outcome)
}

fn outbound(args: &[String]) -> Result<OutboundAction, String> {
    Ok(OutboundAction { connector_id: Uuid::now_v7(), channel: flag(args, "--channel")?, content: flag(args, "--content")?, payload: Value::Null })
}

async fn deliver(connector: &dyn Connector, action: OutboundAction) -> ProbeResult {
    Ok(match connector.send(action).await {
        Ok(()) => json!({ "sent": true }),
        Err(error) => json!({ "sent": false, "error": error.to_string() }),
    })
}

async fn discord(args: &[String]) -> ProbeResult {
    let connector =
        DiscordConnector::new(connector_config(ConnectorKind::Discord, json!({ "base_url": flag(args, "--base-url")? })), Some(flag(args, "--token")?));
    deliver(&connector, outbound(args)?).await
}

async fn slack(args: &[String]) -> ProbeResult {
    let connector =
        SlackConnector::new(connector_config(ConnectorKind::Slack, json!({ "base_url": flag(args, "--base-url")? })), Some(flag(args, "--token")?), None);
    deliver(&connector, outbound(args)?).await
}

fn reply_graph(template: &str) -> SpellGraph {
    let mut graph = SpellGraph::new("probe reply");
    graph.nodes.push(NodeInstance { id: "reply".into(), kind: "probe.reply".to_owned(), position: [0.0, 0.0], config: json!({ "template": template }) });
    graph
}

async fn supervisor() -> ProbeResult {
    let bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::default());
    let supervisor = AgentSupervisor::new(Arc::clone(&bus));
    let agent_id = Uuid::now_v7();
    let connector_id = Uuid::now_v7();
    supervisor.register(agent_id).await;
    let mut events = bus.subscribe();
    supervisor.start(agent_id).await.map_err(|error| error.to_string())?;
    supervisor.deploy_release(agent_id, reply_graph("v1:{content}")).await.map_err(|error| error.to_string())?;
    let inbound = |channel: &str, content: &str| InboundEvent {
        connector_id,
        agent_id,
        kind: ConnectorKind::Studio,
        channel: channel.to_owned(),
        sender: "probe".to_owned(),
        content: content.to_owned(),
        payload: Value::Null,
        received_at: chrono::Utc::now(),
    };
    for (channel, content) in [("alpha", "a1"), ("beta", "b1"), ("alpha", "a2"), ("beta", "b2"), ("alpha", "a3")] {
        supervisor.dispatch_inbound(inbound(channel, content)).await.map_err(|error| error.to_string())?;
    }
    let release_version = supervisor.deploy_release(agent_id, reply_graph("v2:{content}")).await.map_err(|error| error.to_string())?;
    for (channel, content) in [("alpha", "a4"), ("beta", "b3"), ("alpha", "a5")] {
        supervisor.dispatch_inbound(inbound(channel, content)).await.map_err(|error| error.to_string())?;
    }
    let mut outbound = Vec::new();
    let mut status_changes = Vec::new();
    let mut trace_runs_finished = 0_u32;
    while outbound.len() < 8 {
        match tokio::time::timeout(PROBE_DEADLINE, events.recv()).await.map_err(|_| "agent replies never arrived".to_owned())? {
            Ok(RuntimeEvent::Outbound { action }) => outbound.push(action),
            Ok(RuntimeEvent::AgentStatusChanged { status, .. }) => status_changes.push(status),
            Ok(RuntimeEvent::Trace { event: TraceEvent::RunFinished { .. }, .. }) => trace_runs_finished += 1,
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let status_after_swap = supervisor.status(agent_id).await;
    let stats = supervisor.run_stats(agent_id);
    supervisor.stop(agent_id).await.map_err(|error| error.to_string())?;
    let stopped = supervisor.status(agent_id).await == Some(AgentStatus::Stopped);
    Ok(json!({
        "outbound": outbound,
        "status_changes": status_changes,
        "status_after_swap": status_after_swap,
        "release_version": release_version,
        "runs_completed": stats.runs_completed,
        "runs_failed": stats.runs_failed,
        "trace_runs_finished": trace_runs_finished,
        "stopped_cleanly": stopped,
    }))
}

fn probe_specs() -> Vec<NodeSpec> {
    let spec = |kind: &str, is_trigger: bool, inputs: Vec<SocketSpec>, outputs: Vec<SocketSpec>| NodeSpec {
        kind: kind.to_owned(),
        title: kind.to_owned(),
        category: "probe".to_owned(),
        description: String::new(),
        inputs,
        outputs,
        config: Vec::new(),
        is_trigger,
    };
    vec![
        spec("probe.trigger", true, Vec::new(), vec![SocketSpec::output("out", SocketType::Flow)]),
        spec("probe.reply", false, vec![SocketSpec::input("in", SocketType::Flow)], Vec::new()),
    ]
}

fn copilot() -> ProbeResult {
    let input: Value = serde_json::from_str(&stdin_text()?).map_err(|error| error.to_string())?;
    let graph: SpellGraph = serde_json::from_value(input["graph"].clone()).map_err(|error| error.to_string())?;
    let ops: Vec<GraphPatchOp> = serde_json::from_value(input["ops"].clone()).map_err(|error| error.to_string())?;
    let original = graph.clone();
    let engine = CopilotEngine::new(SpellValidator::new(probe_specs()));
    Ok(match engine.apply(&graph, &ops) {
        Ok(patched) => json!({ "accepted": true, "validation_errors": [], "graph": patched, "original_unchanged": graph == original }),
        Err(rejection) => json!({ "accepted": false, "validation_errors": rejection.errors, "graph": Value::Null, "original_unchanged": graph == original }),
    })
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("tick") => tick(&args).await,
        Some("webhook") => webhook(&args).await,
        Some("discord") => discord(&args).await,
        Some("slack") => slack(&args).await,
        Some("supervisor") => supervisor().await,
        Some("copilot") => copilot(),
        _ => Err("usage: runtime_probe <tick|webhook|discord|slack|supervisor|copilot> [flags]".to_owned()),
    };
    match result {
        Ok(output) => {
            println!("{output}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("runtime_probe: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
