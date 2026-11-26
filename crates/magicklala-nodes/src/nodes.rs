use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use magicklala_graph::{
    EmbeddingRequest, LlmGateway, LlmMessage, LlmRequest, LlmRole, NodeContext, NodeError, NodeExecutor, NodeOutcome, NodeSpec, TraceEvent, Value,
};
use regex::Regex;
use serde_json::json;

use crate::script::run_rhai;

pub(crate) const MAX_HTTP_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const MAX_HTTP_REDIRECTS: usize = 5;
const CROSS_ORIGIN_SAFE_HEADERS: [&str; 4] = ["accept", "accept-language", "content-type", "user-agent"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    OnStart,
    OnMessage,
    OnWebhook,
    OnTick,
    OnMemory,
    OnVariableChanged,
    Branch,
    Switch,
    Split,
    WaitForAll,
    WhileLoop,
    DoOnce,
    Subgraph,
    Delay,
    VarGet,
    VarSet,
    VarReset,
    Template,
    Replace,
    RegexMatch,
    Validate,
    JsonParse,
    JsonStringify,
    JsonDestructure,
    HttpFetch,
    GenerateText,
    StreamText,
    StructuredJson,
    Embedding,
    MemoryAdd,
    MemorySearch,
    MemoryClear,
    KnowledgeAdd,
    KnowledgeSearch,
    KnowledgeChunk,
    SendMessage,
    StreamMessage,
    RhaiScript,
}

pub(crate) struct StdNode {
    pub(crate) spec: NodeSpec,
    pub(crate) op: Op,
}

#[async_trait]
impl NodeExecutor for StdNode {
    fn spec(&self) -> &NodeSpec {
        &self.spec
    }

    async fn execute(&self, ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
        match self.op {
            Op::OnStart | Op::OnMessage | Op::OnWebhook | Op::OnTick | Op::OnMemory | Op::OnVariableChanged => trigger(self.op, ctx).await,
            Op::Branch => Ok(NodeOutcome::fire(if value(ctx, "condition").await?.is_truthy() { "true" } else { "false" })),
            Op::Switch => switch(ctx).await,
            Op::Split => {
                Ok(NodeOutcome { fire: config_list(ctx, "outputs").unwrap_or_else(|| vec!["out_0".to_owned(), "out_1".to_owned()]), ..NodeOutcome::default() })
            }
            Op::WaitForAll | Op::DoOnce => Ok(NodeOutcome::fire("out")),
            Op::Subgraph => Err(NodeError::Other("flow/subgraph runs only inside SpellEngine, which executes the child spell".to_owned())),
            Op::WhileLoop => Ok(NodeOutcome::fire(if value(ctx, "condition").await?.is_truthy() { "body" } else { "completed" })),
            Op::Delay => delay(ctx).await,
            Op::VarGet => {
                let name = required_config(ctx, "name")?;
                Ok(NodeOutcome::default().with_output("value", ctx.variable(&name).unwrap_or_default()))
            }
            Op::VarSet => {
                let name = required_config(ctx, "name")?;
                let next = value(ctx, "value").await?;
                ctx.set_variable(&name, next.clone())?;
                Ok(NodeOutcome::fire("out").with_output("value", next))
            }
            Op::VarReset => {
                let name = required_config(ctx, "name")?;
                let initial = ctx.variable_default(&name).unwrap_or_default();
                ctx.set_variable(&name, initial.clone())?;
                Ok(NodeOutcome::fire("out").with_output("value", initial))
            }
            Op::Template => template(ctx).await,
            Op::Replace => replace(ctx).await,
            Op::RegexMatch => regex_match(ctx).await,
            Op::Validate => validate(ctx).await,
            Op::JsonParse => {
                let source = required_text(ctx, "text").await?;
                let parsed: serde_json::Value = serde_json::from_str(&source).map_err(|error| NodeError::Other(format!("invalid JSON: {error}")))?;
                Ok(NodeOutcome::default().with_output("json", Value::Json(parsed)))
            }
            Op::JsonStringify => {
                let json = value(ctx, "value").await?.to_json();
                let pretty = config_bool(ctx, "pretty", false);
                let rendered = if pretty { serde_json::to_string_pretty(&json) } else { serde_json::to_string(&json) };
                Ok(NodeOutcome::default().with_output("text", rendered.map_err(|error| NodeError::Other(error.to_string()))?))
            }
            Op::JsonDestructure => {
                let json = value(ctx, "json").await?.to_json();
                let mut outcome = NodeOutcome::default();
                for key in config_list(ctx, "keys").unwrap_or_default() {
                    outcome = outcome.with_output(&key, Value::from(json.get(&key).cloned().unwrap_or_default()));
                }
                Ok(outcome)
            }
            Op::HttpFetch => http_fetch(ctx).await,
            Op::GenerateText => generate_text(ctx).await,
            Op::StreamText => stream_text(ctx).await,
            Op::StructuredJson => structured_json(ctx).await,
            Op::Embedding => embedding(ctx).await,
            Op::MemoryAdd | Op::MemorySearch | Op::MemoryClear => memory(self.op, ctx).await,
            Op::KnowledgeAdd | Op::KnowledgeSearch => knowledge(self.op, ctx).await,
            Op::KnowledgeChunk => chunk(ctx).await,
            Op::SendMessage => {
                let content = required_text(ctx, "content").await?;
                Ok(NodeOutcome::fire("out").with_output("sent", content.clone()).with_reply(content))
            }
            Op::StreamMessage => stream_message(ctx).await,
            Op::RhaiScript => run_rhai(ctx).await,
        }
    }
}

pub(crate) async fn value(ctx: &mut dyn NodeContext, name: &str) -> Result<Value, NodeError> {
    Ok(ctx.input(name).await?.unwrap_or_default())
}

async fn text(ctx: &mut dyn NodeContext, name: &str) -> Result<String, NodeError> {
    Ok(value(ctx, name).await?.to_text())
}

async fn required_text(ctx: &mut dyn NodeContext, name: &str) -> Result<String, NodeError> {
    match ctx.input(name).await? {
        None | Some(Value::Null) => Err(NodeError::MissingInput(name.to_owned())),
        Some(found) => Ok(found.to_text()),
    }
}

pub(crate) fn config_text(ctx: &dyn NodeContext, key: &str) -> Option<String> {
    ctx.node().config.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
}

fn required_config(ctx: &dyn NodeContext, key: &str) -> Result<String, NodeError> {
    config_text(ctx, key).ok_or_else(|| NodeError::InvalidConfig(format!("{key} is required")))
}

pub(crate) fn config_u64(ctx: &dyn NodeContext, key: &str, default: u64) -> Result<u64, NodeError> {
    match ctx.node().config.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(value) => {
            magicklala_graph::whole_number(value).ok_or_else(|| NodeError::Other(format!("{key} needs a whole number of at least 0, but got {value}")))
        }
    }
}

fn config_bool(ctx: &dyn NodeContext, key: &str, default: bool) -> bool {
    ctx.node().config.get(key).and_then(serde_json::Value::as_bool).unwrap_or(default)
}

pub(crate) fn config_list(ctx: &dyn NodeContext, key: &str) -> Option<Vec<String>> {
    ctx.node().config.get(key).and_then(serde_json::Value::as_array).map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
}

fn usize_from(number: u64) -> usize {
    usize::try_from(number).unwrap_or(usize::MAX)
}

async fn trigger(op: Op, ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let payload = value(ctx, "payload").await?;
    let json = payload.to_json();
    let field = |key: &str| json.get(key).cloned().unwrap_or_default();
    let field_text = |key: &str| Value::from(field(key)).to_text();
    let outcome = NodeOutcome::fire("out");
    Ok(match op {
        Op::OnStart => outcome.with_output("payload", payload),
        Op::OnMessage => {
            let content = if let Value::String(text) = &payload { text.clone() } else { field_text("content") };
            outcome.with_output("content", content).with_output("sender", field_text("sender")).with_output("payload", Value::Json(json.clone()))
        }
        Op::OnWebhook => outcome
            .with_output("method", field_text("method"))
            .with_output("path", field_text("path"))
            .with_output("headers", Value::Json(field("headers")))
            .with_output("query", Value::Json(field("query")))
            .with_output("body", Value::Json(field("body"))),
        Op::OnTick => outcome.with_output("tick", field("tick").as_i64().unwrap_or_default()).with_output("at", field_text("at")),
        Op::OnMemory => outcome.with_output("content", field_text("content")).with_output("payload", Value::Json(json.clone())),
        _ => outcome.with_output("name", field_text("name")).with_output("value", Value::from(field("value"))),
    })
}

async fn switch(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let selected = text(ctx, "value").await?;
    let cases = config_list(ctx, "cases").unwrap_or_default();
    let route = if cases.contains(&selected) { selected.clone() } else { "default".to_owned() };
    Ok(NodeOutcome::fire(&route))
}

async fn delay(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let millis = match ctx.input("ms").await? {
        Some(Value::Int(millis)) => u64::try_from(millis).map_err(|_| NodeError::Other(format!("ms needs a whole number of at least 0, but got {millis}")))?,
        _ => config_u64(ctx, "ms", 1000)?,
    };
    let cancellation = ctx.cancellation();
    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(millis)) => Ok(NodeOutcome::fire("out")),
        () = cancellation.cancelled() => Err(NodeError::Cancelled),
    }
}

static PLACEHOLDER_PATTERN: std::sync::LazyLock<Result<Regex, String>> =
    std::sync::LazyLock::new(|| Regex::new(r"\{\{\s*([A-Za-z0-9_\-]+)((?:\.[A-Za-z0-9_\-]+)*)\s*\}\}").map_err(|error| error.to_string()));

pub fn template_roots(source: &str) -> Vec<String> {
    let Ok(pattern) = placeholder_pattern() else { return Vec::new() };
    pattern.captures_iter(source).filter_map(|captures| captures.get(1).map(|root| root.as_str().to_owned())).fold(Vec::new(), |mut roots, root| {
        if !roots.contains(&root) {
            roots.push(root);
        }
        roots
    })
}

pub fn rename_template_root(source: &str, old: &str, new: &str) -> String {
    let Ok(pattern) = placeholder_pattern() else { return source.to_owned() };
    pattern
        .replace_all(source, |captures: &regex::Captures<'_>| {
            let whole = captures.get(0).map_or("", |whole| whole.as_str());
            match captures.get(1).filter(|root| root.as_str() == old) {
                Some(root) => {
                    let start = root.start() - captures.get(0).map_or(0, |whole| whole.start());
                    format!("{}{new}{}", &whole[..start], &whole[start + old.len()..])
                }
                None => whole.to_owned(),
            }
        })
        .into_owned()
}

pub fn is_template_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn placeholder_pattern() -> Result<&'static Regex, NodeError> {
    PLACEHOLDER_PATTERN.as_ref().map_err(|error| NodeError::Other(error.clone()))
}

async fn template(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let source = config_text(ctx, "template").unwrap_or_default();
    let pattern = placeholder_pattern()?;
    let mut resolved = std::collections::BTreeMap::new();
    let roots: Vec<String> = pattern.captures_iter(&source).filter_map(|captures| captures.get(1).map(|root| root.as_str().to_owned())).collect();
    for root in roots {
        if resolved.contains_key(&root) {
            continue;
        }
        let found = match ctx.input(&root).await? {
            Some(found) => found,
            None => ctx.variable(&root).unwrap_or_default(),
        };
        resolved.insert(root, found);
    }
    let rendered = pattern.replace_all(&source, |captures: &regex::Captures<'_>| {
        let root = captures.get(1).map_or("", |root| root.as_str());
        let path = captures.get(2).map_or("", |path| path.as_str());
        let base = resolved.get(root).cloned().unwrap_or_default();
        if path.is_empty() {
            return base.to_text();
        }
        let mut cursor = base.to_json();
        for segment in path.split('.').filter(|segment| !segment.is_empty()) {
            cursor = match segment.parse::<usize>() {
                Ok(index) => cursor.get(index).cloned().unwrap_or_default(),
                Err(_) => cursor.get(segment).cloned().unwrap_or_default(),
            };
        }
        Value::from(cursor).to_text()
    });
    Ok(NodeOutcome::default().with_output("text", rendered.into_owned()))
}

async fn replace(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let source = required_text(ctx, "text").await?;
    let find = text(ctx, "find").await?;
    let replacement = text(ctx, "replace_with").await?;
    if find.is_empty() {
        return Ok(NodeOutcome::default().with_output("text", source));
    }
    let replaced = if config_bool(ctx, "regex", false) {
        let pattern = Regex::new(&find).map_err(|error| NodeError::InvalidConfig(error.to_string()))?;
        pattern.replace_all(&source, replacement.as_str()).into_owned()
    } else {
        source.replace(&find, &replacement)
    };
    Ok(NodeOutcome::default().with_output("text", replaced))
}

async fn regex_match(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let source = required_text(ctx, "text").await?;
    let pattern_text = text(ctx, "pattern").await?;
    let pattern = Regex::new(&pattern_text).map_err(|error| NodeError::InvalidConfig(error.to_string()))?;
    let captures = pattern.captures(&source);
    let groups: Vec<Value> =
        captures.as_ref().map(|found| found.iter().skip(1).map(|group| Value::from(group.map_or("", |group| group.as_str()))).collect()).unwrap_or_default();
    let first = captures.as_ref().and_then(|found| found.get(0)).map_or("", |whole| whole.as_str()).to_owned();
    Ok(NodeOutcome::default().with_output("matched", captures.is_some()).with_output("first", first).with_output("captures", Value::Array(groups)))
}

async fn validate(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let candidate = value(ctx, "value").await?;
    let json = candidate.to_json();
    let mut problems = Vec::new();
    if config_bool(ctx, "non_empty", true) && !candidate.is_truthy() && !matches!(candidate, Value::Bool(_) | Value::Int(_) | Value::Float(_)) {
        problems.push(Value::from("value is empty"));
    }
    for key in config_list(ctx, "required_keys").unwrap_or_default() {
        if json.get(&key).is_none_or(serde_json::Value::is_null) {
            problems.push(Value::from(format!("missing key {key}")));
        }
    }
    if let Some(pattern_text) = config_text(ctx, "pattern") {
        let pattern = Regex::new(&pattern_text).map_err(|error| NodeError::InvalidConfig(error.to_string()))?;
        if !pattern.is_match(&candidate.to_text()) {
            problems.push(Value::from(format!("value does not match {pattern_text}")));
        }
    }
    Ok(NodeOutcome::default().with_output("valid", problems.is_empty()).with_output("errors", Value::Array(problems)))
}

fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [first, second, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || first == 0
                || (first == 100 && (64..128).contains(&second))
                || (first == 198 && (18..20).contains(&second))
                || first >= 240
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_private_address(IpAddr::V4(mapped));
            }
            let segments = v6.segments();
            let head = segments[0];
            let nat64 = head == 0x64 && segments[1] == 0xff9b && (segments[2..6] == [0; 4] || segments[2] == 1);
            let ipv4_compatible = segments[..6] == [0; 6];
            let discard = segments[..4] == [0x100, 0, 0, 0];
            let documentation = head == 0x2001 && segments[1] == 0xdb8;
            let six_to_four_private =
                head == 0x2002 && is_private_address(IpAddr::V4(std::net::Ipv4Addr::from((u32::from(segments[1]) << 16) | u32::from(segments[2]))));
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (head & 0xfe00) == 0xfc00
                || (head & 0xffc0) == 0xfe80
                || (head & 0xffc0) == 0xfec0
                || nat64
                || ipv4_compatible
                || discard
                || documentation
                || six_to_four_private
        }
    }
}

async fn resolve_target(url: &reqwest::Url, allow_private: bool) -> Result<Option<(String, Vec<SocketAddr>)>, NodeError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(NodeError::InvalidConfig(format!("net/http_fetch only supports http and https, not {}", url.scheme())));
    }
    let host = url.host_str().ok_or_else(|| NodeError::InvalidConfig(format!("{url} has no host")))?.to_owned();
    let port = url.port_or_known_default().unwrap_or(80);
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<SocketAddr> = match bare.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((bare, port)).await.map_err(|error| NodeError::Service(format!("cannot resolve {host}: {error}")))?.collect(),
    };
    if addresses.is_empty() {
        return Err(NodeError::Service(format!("cannot resolve {host}: no addresses")));
    }
    if allow_private {
        return Ok(None);
    }
    if let Some(blocked) = addresses.iter().find(|address| is_private_address(address.ip())) {
        return Err(NodeError::Service(format!("net/http_fetch refuses to connect to private address {} for host {host}", blocked.ip())));
    }
    Ok(if bare.parse::<IpAddr>().is_ok() { None } else { Some((bare.to_owned(), addresses)) })
}

async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, NodeError> {
    let too_large = || NodeError::Service(format!("response exceeds {MAX_HTTP_RESPONSE_BYTES} bytes"));
    if response.content_length().is_some_and(|length| length > MAX_HTTP_RESPONSE_BYTES as u64) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| NodeError::Service(error.to_string()))? {
        if body.len() + chunk.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

struct FetchPlan {
    method: reqwest::Method,
    headers: Vec<(String, String)>,
    body: Value,
    timeout: Duration,
    allow_private: bool,
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme() && left.host_str() == right.host_str() && left.port_or_known_default() == right.port_or_known_default()
}

fn is_cross_origin_safe_header(name: &str) -> bool {
    CROSS_ORIGIN_SAFE_HEADERS.iter().any(|safe| name.eq_ignore_ascii_case(safe))
}

impl FetchPlan {
    fn has_body(&self) -> bool {
        match &self.body {
            Value::Null => false,
            Value::String(text) => !text.is_empty(),
            _ => true,
        }
    }

    async fn send(&self, url: &reqwest::Url, method: &reqwest::Method, headers: &[(String, String)], with_body: bool) -> Result<reqwest::Response, NodeError> {
        let mut builder = reqwest::Client::builder().no_proxy().timeout(self.timeout).redirect(reqwest::redirect::Policy::none());
        if let Some((host, addresses)) = resolve_target(url, self.allow_private).await? {
            builder = builder.resolve_to_addrs(&host, &addresses);
        }
        let client = builder.build().map_err(|error| NodeError::Service(error.to_string()))?;
        let mut request = client.request(method.clone(), url.clone());
        for (name, header) in headers {
            request = request.header(name, header);
        }
        if with_body {
            request = match &self.body {
                Value::Null => request,
                Value::String(text) => request.body(text.clone()),
                other => request.json(&other.to_json()),
            };
        }
        request.send().await.map_err(|error| NodeError::Service(error.to_string()))
    }

    async fn follow(&self, start: reqwest::Url) -> Result<reqwest::Response, NodeError> {
        let mut url = start;
        let mut method = self.method.clone();
        let mut with_body = true;
        let mut headers = self.headers.clone();
        for _ in 0..=MAX_HTTP_REDIRECTS {
            let response = self.send(&url, &method, &headers, with_body).await?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let Some(location) = response.headers().get(reqwest::header::LOCATION).and_then(|value| value.to_str().ok()) else {
                return Ok(response);
            };
            let next = url.join(location).map_err(|error| NodeError::Service(format!("invalid redirect location: {error}")))?;
            if !matches!(response.status().as_u16(), 307 | 308) {
                method = reqwest::Method::GET;
                with_body = false;
            }
            if !same_origin(&url, &next) {
                if with_body && self.has_body() {
                    return Err(NodeError::Service(format!("refused cross-origin redirect with request body from {url} to {next}")));
                }
                headers.retain(|(name, _)| is_cross_origin_safe_header(name));
            }
            url = next;
        }
        Err(NodeError::Service(format!("more than {MAX_HTTP_REDIRECTS} redirects")))
    }
}

async fn http_fetch(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let url = required_text(ctx, "url").await?;
    let body = value(ctx, "body").await?;
    let method_name = config_text(ctx, "method").unwrap_or_else(|| "GET".to_owned()).to_uppercase();
    let method = reqwest::Method::from_bytes(method_name.as_bytes()).map_err(|error| NodeError::InvalidConfig(error.to_string()))?;
    let target = reqwest::Url::parse(&url).map_err(|error| NodeError::InvalidConfig(format!("invalid url {url}: {error}")))?;
    let headers = ctx
        .node()
        .config
        .get("headers")
        .and_then(serde_json::Value::as_object)
        .map(|headers| headers.iter().map(|(name, header)| (name.clone(), Value::from(header.clone()).to_text())).collect())
        .unwrap_or_default();
    let plan =
        FetchPlan { method, headers, body, timeout: Duration::from_millis(config_u64(ctx, "timeout_ms", 30_000)?), allow_private: ctx.allow_private_http() };
    let cancellation = ctx.cancellation();
    let fetched = async {
        let response = plan.follow(target).await?;
        let status = i64::from(response.status().as_u16());
        Ok::<_, NodeError>((status, read_capped(response).await?))
    };
    let (status, bytes) = tokio::select! {
        fetched = fetched => fetched?,
        () = cancellation.cancelled() => return Err(NodeError::Cancelled),
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    Ok(NodeOutcome::fire("out").with_output("status", status).with_output("body", text).with_output("json", Value::Json(json)))
}

async fn embedding(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let text = required_text(ctx, "text").await?;
    let request = EmbeddingRequest {
        provider: config_text(ctx, "provider").unwrap_or_default(),
        model: config_text(ctx, "model").unwrap_or_else(|| "text-embedding-3-small".to_owned()),
        inputs: vec![text],
        credential_id: config_text(ctx, "credential_id"),
    };
    let llm = gateway(ctx)?;
    let cancellation = ctx.cancellation();
    let vectors = tokio::select! {
        vectors = llm.embed_request(request) => vectors?,
        () = cancellation.cancelled() => return Err(NodeError::Cancelled),
    };
    let vector = vectors.into_iter().next().ok_or_else(|| NodeError::Llm("embedding response had no vector".to_owned()))?;
    Ok(NodeOutcome::fire("out").with_output("embedding", Value::Embedding(vector)))
}

fn gateway(ctx: &dyn NodeContext) -> Result<Arc<dyn LlmGateway>, NodeError> {
    ctx.llm().ok_or_else(|| NodeError::Service("no LLM gateway is configured".to_owned()))
}

async fn llm_request(ctx: &mut dyn NodeContext, extra_system: Option<String>) -> Result<LlmRequest, NodeError> {
    let prompt = required_text(ctx, "prompt").await?;
    let system = [text(ctx, "system").await?, extra_system.unwrap_or_default()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n\n");
    let mut messages = Vec::new();
    if !system.is_empty() {
        messages.push(LlmMessage { role: LlmRole::System, content: system });
    }
    messages.push(LlmMessage { role: LlmRole::User, content: prompt });
    let config = &ctx.node().config;
    Ok(LlmRequest {
        provider: config_text(ctx, "provider").unwrap_or_else(|| "openai".to_owned()),
        model: config_text(ctx, "model").unwrap_or_else(|| "gpt-4o-mini".to_owned()),
        messages,
        temperature: config.get("temperature").and_then(serde_json::Value::as_f64).map(|temperature| temperature as f32),
        max_tokens: config.get("max_tokens").and_then(magicklala_graph::whole_number).and_then(|tokens| u32::try_from(tokens).ok()),
        stop: config_list(ctx, "stop").unwrap_or_default(),
        credential_id: config_text(ctx, "credential_id"),
    })
}

async fn generate_text(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let request = llm_request(ctx, None).await?;
    let llm = gateway(ctx)?;
    let cancellation = ctx.cancellation();
    let response = tokio::select! {
        response = llm.complete(request) => response?,
        () = cancellation.cancelled() => return Err(NodeError::Cancelled),
    };
    Ok(NodeOutcome::fire("out").with_output("text", response.text).with_output("model", response.model))
}

async fn stream_text(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let request = llm_request(ctx, None).await?;
    let llm = gateway(ctx)?;
    let cancellation = ctx.cancellation();
    let events = ctx.events();
    let run_id = ctx.run_id();
    let node_id = ctx.node().id.clone();
    let mut stream = llm.stream(request).await?;
    let mut full = String::new();
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = cancellation.cancelled() => return Err(NodeError::Cancelled),
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk?;
        if !chunk.delta.is_empty() {
            full.push_str(&chunk.delta);
            events.emit(TraceEvent::StreamChunk { run_id, node_id: node_id.clone(), delta: chunk.delta, subgraph_path: Vec::new() });
        }
        if chunk.done {
            break;
        }
    }
    Ok(NodeOutcome::fire("out").with_output("text", full))
}

fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let without_open = trimmed.strip_prefix("```json").or_else(|| trimmed.strip_prefix("```")).unwrap_or(trimmed);
    without_open.strip_suffix("```").unwrap_or(without_open).trim()
}

async fn structured_json(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let schema = ctx.node().config.get("schema").cloned().unwrap_or_else(|| json!({}));
    let instruction = format!("Reply with only a JSON value that matches this JSON schema, with no prose: {schema}");
    let request = llm_request(ctx, Some(instruction)).await?;
    let response = gateway(ctx)?.complete(request).await?;
    let parsed: serde_json::Value =
        serde_json::from_str(strip_code_fence(&response.text)).map_err(|error| NodeError::Llm(format!("model did not return valid JSON: {error}")))?;
    Ok(NodeOutcome::fire("out").with_output("json", Value::Json(parsed)))
}

fn agent_id(ctx: &dyn NodeContext) -> String {
    if let Some(bound) = ctx.agent_id() {
        return bound.to_owned();
    }
    config_text(ctx, "agent_id")
        .or_else(|| ctx.variable("agent_id").map(|found| found.to_text()))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "default".to_owned())
}

async fn memory(op: Op, ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let service = ctx.memory().ok_or_else(|| NodeError::Service("no memory service is configured".to_owned()))?;
    let agent = agent_id(ctx);
    match op {
        Op::MemoryAdd => {
            let content = required_text(ctx, "content").await?;
            let metadata = value(ctx, "metadata").await?.to_json();
            let id = service.add(&agent, &content, metadata).await?;
            Ok(NodeOutcome::fire("out").with_output("id", id))
        }
        Op::MemorySearch => {
            let query = required_text(ctx, "query").await?;
            let items = service.recall(&agent, &query, usize_from(config_u64(ctx, "limit", 5)?)).await?;
            let text = items.iter().map(|item| item.content.clone()).collect::<Vec<_>>().join("\n");
            let results = items
                .iter()
                .map(|item| Value::Json(json!({ "id": item.id, "content": item.content, "score": item.score, "metadata": item.metadata })))
                .collect();
            Ok(NodeOutcome::fire("out").with_output("results", Value::Array(results)).with_output("text", text))
        }
        _ => {
            let removed = service.clear(&agent).await?;
            Ok(NodeOutcome::fire("out").with_output("removed", i64::try_from(removed).unwrap_or(i64::MAX)))
        }
    }
}

async fn knowledge(op: Op, ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let service = ctx.knowledge().ok_or_else(|| NodeError::Service("no knowledge service is configured".to_owned()))?;
    let agent = agent_id(ctx);
    if op == Op::KnowledgeAdd {
        let title = text(ctx, "title").await?;
        let content = required_text(ctx, "content").await?;
        let doc_id = service.add(&agent, &title, &content).await?;
        return Ok(NodeOutcome::fire("out").with_output("doc_id", doc_id));
    }
    let query = required_text(ctx, "query").await?;
    let hits = service.search(&agent, &query, usize_from(config_u64(ctx, "limit", 5)?)).await?;
    let text = hits.iter().map(|hit| hit.text.clone()).collect::<Vec<_>>().join("\n");
    let results =
        hits.iter().map(|hit| Value::Json(json!({ "doc_id": hit.doc_id, "chunk_index": hit.chunk_index, "text": hit.text, "score": hit.score }))).collect();
    Ok(NodeOutcome::fire("out").with_output("results", Value::Array(results)).with_output("text", text))
}

pub(crate) fn split_chunks(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    let size = size.max(1);
    let stride = size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let end = (start + size).min(characters.len());
        chunks.push(characters[start..end].iter().collect());
        if end == characters.len() {
            break;
        }
        start += stride;
    }
    chunks
}

async fn chunk(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let source = required_text(ctx, "text").await?;
    let size = match value(ctx, "size").await? {
        Value::Int(size) if size > 0 => usize::try_from(size).unwrap_or(500),
        _ => 500,
    };
    let overlap = match value(ctx, "overlap").await? {
        Value::Int(overlap) if overlap >= 0 => usize::try_from(overlap).unwrap_or(50),
        _ => 50,
    };
    let chunks = split_chunks(&source, size, overlap).into_iter().map(Value::String).collect();
    Ok(NodeOutcome::default().with_output("chunks", Value::Array(chunks)))
}

async fn stream_message(ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError> {
    let content = required_text(ctx, "content").await?;
    let events = ctx.events();
    let run_id = ctx.run_id();
    let node_id = ctx.node().id.clone();
    for word in content.split_inclusive(' ') {
        events.emit(TraceEvent::StreamChunk { run_id, node_id: node_id.clone(), delta: word.to_owned(), subgraph_path: Vec::new() });
    }
    Ok(NodeOutcome::fire("out").with_output("sent", content.clone()).with_reply(content))
}
