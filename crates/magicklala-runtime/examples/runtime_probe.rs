use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use magicklala_connectors::{
    Connector, ConnectorConfig, ConnectorKind, DiscordConnector, InboundEvent, OutboundAction, SlackConnector, TickConnector, WEBHOOK_SIGNATURE_HEADER,
    WebhookConnector,
};
use magicklala_graph::{EngineServices, NodeCatalog, NodeInstance, NodeSpec, SocketSpec, SocketType, SpellGraph, SpellValidator, TraceEvent};
use magicklala_runtime::{
    AgentStatus, AgentSupervisor, CHANNEL_LANE_CAPACITY, CopilotEngine, EngineReleaseRunner, EventBus, GraphPatchOp, InMemoryEventBus, ReleaseOrigin,
    RuntimeEvent, StoreScope,
};
use magicklala_store::{AgentId, NewAgent, NewProject, NewSpell, Repository, SqliteStore};
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

fn text_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn wire(id: &str, from: (&str, &str), to: (&str, &str)) -> Value {
    json!({ "id": id, "from_node": from.0, "from_socket": from.1, "to_node": to.0, "to_socket": to.1 })
}

fn node(id: &str, kind: &str, config: Value) -> Value {
    json!({ "id": id, "kind": kind, "position": [0.0, 0.0], "config": config })
}

fn spell(name: &str, nodes: Vec<Value>, wires: Vec<Value>) -> Result<SpellGraph, String> {
    let mut graph = SpellGraph::new(name);
    graph.nodes = serde_json::from_value(Value::Array(nodes)).map_err(text_error)?;
    graph.wires = serde_json::from_value(Value::Array(wires)).map_err(text_error)?;
    Ok(graph)
}

const STALL_SCRIPT: &str = r#"if content == "stall" { 60000 } else if content == "boom" { throw "boom requested by the probe" } else { 0 }"#;

fn stall_graph() -> Result<SpellGraph, String> {
    spell(
        "probe stall",
        vec![
            node("on", "event/on_message", json!({})),
            node("plan", "code/rhai_script", json!({ "script": STALL_SCRIPT, "inputs": ["content"] })),
            node("wait", "time/delay", json!({})),
            node("send", "io/send_message", json!({})),
        ],
        vec![
            wire("w-flow-plan", ("on", "out"), ("plan", "in")),
            wire("w-content-plan", ("on", "content"), ("plan", "content")),
            wire("w-flow-wait", ("plan", "out"), ("wait", "in")),
            wire("w-ms", ("plan", "result"), ("wait", "ms")),
            wire("w-flow-send", ("wait", "out"), ("send", "in")),
            wire("w-content-send", ("on", "content"), ("send", "content")),
        ],
    )
}

fn webhook_graph() -> Result<SpellGraph, String> {
    let template = "{{method}} {{path}} amount={{body.amount}} order={{body.order}} shop={{headers.shop}} live={{query.live}}";
    spell(
        "probe webhook",
        vec![
            node("hook", "event/on_webhook", json!({})),
            node("format", "text/template", json!({ "template": template })),
            node("send", "io/send_message", json!({})),
        ],
        vec![
            wire("w-flow", ("hook", "out"), ("send", "in")),
            wire("w-method", ("hook", "method"), ("format", "method")),
            wire("w-path", ("hook", "path"), ("format", "path")),
            wire("w-body", ("hook", "body"), ("format", "body")),
            wire("w-headers", ("hook", "headers"), ("format", "headers")),
            wire("w-query", ("hook", "query"), ("format", "query")),
            wire("w-text", ("format", "text"), ("send", "content")),
        ],
    )
}

struct EngineRig {
    bus: Arc<dyn EventBus>,
    supervisor: AgentSupervisor,
    store: Arc<SqliteStore>,
    agent_id: Uuid,
    spell_id: Uuid,
    database: std::path::PathBuf,
}

impl EngineRig {
    async fn start(graph: SpellGraph, run_timeout: Option<Duration>) -> Result<Self, String> {
        let database = std::env::temp_dir().join(format!("runtime-probe-{}.db", Uuid::now_v7()));
        let store = Arc::new(SqliteStore::connect(&format!("sqlite://{}", database.display())).await.map_err(text_error)?);
        store.migrate().await.map_err(text_error)?;
        let project = store.create_project(NewProject { name: "runtime probe".to_owned(), description: String::new() }).await.map_err(text_error)?;
        let agent = store
            .create_agent(NewAgent { project_id: project.id, name: "probe agent".to_owned(), description: String::new(), settings: json!({}) })
            .await
            .map_err(text_error)?;
        let record = store.create_spell(NewSpell { project_id: project.id, name: graph.name.clone(), graph: graph.clone() }).await.map_err(text_error)?;
        let repository: Arc<dyn Repository> = store.clone();
        let catalog: Arc<dyn NodeCatalog> = Arc::new(magicklala_nodes::default_registry());
        let runner =
            EngineReleaseRunner::scoped(catalog, EngineServices::default(), StoreScope { repository: Arc::clone(&repository), llm: None, embedder: None });
        let bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::default());
        let mut supervisor = AgentSupervisor::with_runner(Arc::clone(&bus), Arc::new(runner)).with_repository(repository);
        if let Some(run_timeout) = run_timeout {
            supervisor = supervisor.with_run_timeout(run_timeout);
        }
        let agent_id = agent.id.0;
        supervisor.register(agent_id).await;
        supervisor.start(agent_id).await.map_err(text_error)?;
        supervisor.deploy_release_from(agent_id, graph, ReleaseOrigin { spell_id: record.id, release_id: None }).await.map_err(text_error)?;
        Ok(Self { bus, supervisor, store, agent_id, spell_id: record.id.0, database })
    }

    fn inbound(&self, kind: ConnectorKind, channel: &str, content: &str, payload: Value) -> InboundEvent {
        InboundEvent {
            connector_id: Uuid::nil(),
            agent_id: self.agent_id,
            kind,
            channel: channel.to_owned(),
            sender: "probe".to_owned(),
            content: content.to_owned(),
            payload,
            received_at: chrono::Utc::now(),
        }
    }

    async fn finish(self) -> Result<(), String> {
        let _stopped = self.supervisor.stop(self.agent_id).await;
        self.store.pool().close().await;
        for suffix in ["", "-wal", "-shm"] {
            let _removed = std::fs::remove_file(format!("{}{suffix}", self.database.display()));
        }
        Ok(())
    }
}

async fn completions(events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>, done: impl Fn(&[Value]) -> bool) -> Result<(Vec<Value>, Vec<Value>), String> {
    let mut completed = Vec::new();
    let mut outbound = Vec::new();
    while !done(&completed) {
        match tokio::time::timeout(PROBE_DEADLINE, events.recv()).await.map_err(|_| "run completions never arrived".to_owned())? {
            Ok(event @ RuntimeEvent::RunCompleted { .. }) => completed.push(serde_json::to_value(event).map_err(text_error)?),
            Ok(RuntimeEvent::Outbound { action }) => {
                outbound.push(json!({ "channel": action.channel, "content": action.content, "correlation_id": action.payload["correlation_id"] }))
            }
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok((completed, outbound))
}

async fn run_started(events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>) -> Result<(), String> {
    loop {
        match tokio::time::timeout(PROBE_DEADLINE, events.recv()).await.map_err(|_| "the first run never started".to_owned())? {
            Ok(RuntimeEvent::Trace { event: TraceEvent::RunStarted { .. }, .. }) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
}

async fn correlated_runs() -> ProbeResult {
    let rig = EngineRig::start(stall_graph()?, None).await?;
    let mut events = rig.bus.subscribe();
    let ok_correlation = format!("ok-{}", Uuid::now_v7());
    let failing_correlation = format!("fail-{}", Uuid::now_v7());
    let failing = rig.inbound(ConnectorKind::Studio, "fail-lane", "boom", json!({ "correlation_id": failing_correlation }));
    let succeeding = rig.inbound(ConnectorKind::Studio, "ok-lane", "hello there", json!({ "correlation_id": ok_correlation }));
    rig.supervisor.dispatch_inbound(failing).await.map_err(text_error)?;
    rig.supervisor.dispatch_inbound(succeeding).await.map_err(text_error)?;
    let (completed, outbound) = completions(&mut events, |completed| completed.len() >= 2).await?;
    let recorded_runs = rig.store.list_runs(AgentId(rig.agent_id), 10).await.map_err(text_error)?;
    let active_runs = rig.supervisor.run_stats(rig.agent_id).active_runs;
    let output = json!({
        "agent_id": rig.agent_id,
        "spell_id": rig.spell_id,
        "ok_correlation": ok_correlation,
        "failing_correlation": failing_correlation,
        "completed": completed,
        "outbound": outbound,
        "recorded_runs": recorded_runs,
        "active_runs": active_runs,
    });
    rig.finish().await?;
    Ok(output)
}

async fn webhook_run(args: &[String]) -> ProbeResult {
    let correlation = flag(args, "--correlation")?;
    let mut request: Value = serde_json::from_str(&stdin_text()?).map_err(text_error)?;
    request["correlation_id"] = Value::String(correlation);
    let rig = EngineRig::start(webhook_graph()?, None).await?;
    let mut events = rig.bus.subscribe();
    let channel = request["path"].as_str().unwrap_or("webhook").to_owned();
    rig.supervisor.dispatch_inbound(rig.inbound(ConnectorKind::Webhook, &channel, "", request)).await.map_err(text_error)?;
    let (completed, _) = completions(&mut events, |completed| !completed.is_empty()).await?;
    let output = json!({ "agent_id": rig.agent_id, "completed": completed });
    rig.finish().await?;
    Ok(output)
}

async fn forget(args: &[String]) -> ProbeResult {
    let timeout_ms: u64 = flag(args, "--timeout-ms")?.parse().map_err(|_| "bad --timeout-ms".to_owned())?;
    let rig = EngineRig::start(stall_graph()?, Some(Duration::from_millis(timeout_ms))).await?;
    let mut events = rig.bus.subscribe();
    let correlation = format!("stall-{}", Uuid::now_v7());
    rig.supervisor
        .dispatch_inbound(rig.inbound(ConnectorKind::Studio, "stuck", "stall", json!({ "correlation_id": correlation })))
        .await
        .map_err(text_error)?;
    let (completed, _) = completions(&mut events, |completed| !completed.is_empty()).await?;
    let stats = rig.supervisor.run_stats(rig.agent_id);
    rig.supervisor.forget(rig.agent_id).await;
    let dispatch_after_forget = match rig.supervisor.dispatch_inbound(rig.inbound(ConnectorKind::Studio, "stuck", "after", json!({}))).await {
        Ok(()) => Value::Null,
        Err(error) => Value::String(error.to_string()),
    };
    let output = json!({
        "agent_id": rig.agent_id,
        "correlation": correlation,
        "completed": completed,
        "active_runs_after_timeout": stats.active_runs,
        "runs_failed": stats.runs_failed,
        "status_after_forget": rig.supervisor.status(rig.agent_id).await,
        "listed_after_forget": rig.supervisor.list().await,
        "release_after_forget": rig.supervisor.release(rig.agent_id).await,
        "dispatch_after_forget": dispatch_after_forget,
    });
    rig.finish().await?;
    Ok(output)
}

async fn busy() -> ProbeResult {
    let rig = EngineRig::start(stall_graph()?, None).await?;
    let mut events = rig.bus.subscribe();
    let dispatched = CHANNEL_LANE_CAPACITY + 8;
    let mut stalled_correlations = Vec::new();
    for index in 0..dispatched {
        let correlation = format!("stalled-{index}");
        rig.supervisor
            .dispatch_inbound(rig.inbound(ConnectorKind::Studio, "stalled-lane", "stall", json!({ "correlation_id": correlation })))
            .await
            .map_err(text_error)?;
        stalled_correlations.push(correlation);
        if index == 0 {
            run_started(&mut events).await?;
        }
    }
    let sibling_correlation = format!("sibling-{}", Uuid::now_v7());
    rig.supervisor
        .dispatch_inbound(rig.inbound(ConnectorKind::Studio, "sibling-lane", "sibling ping", json!({ "correlation_id": sibling_correlation })))
        .await
        .map_err(text_error)?;
    let sibling = sibling_correlation.clone();
    let (completed, _) = completions(&mut events, |completed| completed.iter().any(|event| event["correlation_id"] == sibling.as_str())).await?;
    let stats = rig.supervisor.run_stats(rig.agent_id);
    let stalled_started = stats.runs_started.saturating_sub(1);
    let output = json!({
        "agent_id": rig.agent_id,
        "completed": completed,
        "dispatched_on_stalled_lane": dispatched,
        "queued_on_stalled_lane": u64::try_from(CHANNEL_LANE_CAPACITY).unwrap_or(u64::MAX) + stalled_started,
        "stalled_correlations": stalled_correlations,
        "sibling_correlation": sibling_correlation,
    });
    rig.finish().await?;
    Ok(output)
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
        Some("correlated-runs") => correlated_runs().await,
        Some("webhook-run") => webhook_run(&args).await,
        Some("forget") => forget(&args).await,
        Some("busy") => busy().await,
        _ => Err("usage: runtime_probe <tick|webhook|discord|slack|supervisor|copilot|correlated-runs|webhook-run|forget|busy> [flags]".to_owned()),
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
