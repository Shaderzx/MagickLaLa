use std::sync::Arc;

use magicklala_graph::{DYNAMIC_SOCKET, FieldKind, NodeConfigField, NodeExecutor, NodeSpec, SocketSpec, SocketType};
use serde_json::json;

use crate::nodes::{Op, StdNode};

struct Def {
    spec: NodeSpec,
}

impl Def {
    fn new(kind: &str, title: &str, category: &str, description: &str) -> Self {
        Self {
            spec: NodeSpec {
                kind: kind.to_owned(),
                title: title.to_owned(),
                category: category.to_owned(),
                description: description.to_owned(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                config: Vec::new(),
                is_trigger: false,
            },
        }
    }

    fn trigger(mut self) -> Self {
        self.spec.is_trigger = true;
        self.output("out", SocketType::Flow)
    }

    fn flow_in(self) -> Self {
        self.input("in", SocketType::Flow)
    }

    fn flow_out(self) -> Self {
        self.output("out", SocketType::Flow)
    }

    fn input(mut self, name: &str, ty: SocketType) -> Self {
        self.spec.inputs.push(SocketSpec::input(name, ty));
        self
    }

    fn required(mut self, name: &str, ty: SocketType) -> Self {
        let mut socket = SocketSpec::input(name, ty);
        socket.required = true;
        self.spec.inputs.push(socket);
        self
    }

    fn output(mut self, name: &str, ty: SocketType) -> Self {
        self.spec.outputs.push(SocketSpec::output(name, ty));
        self
    }

    fn field(mut self, key: &str, kind: FieldKind, default: Option<serde_json::Value>, required: bool) -> Self {
        self.spec.config.push(NodeConfigField { key: key.to_owned(), label: key.replace('_', " "), kind, default, required, help: String::new() });
        self
    }

    fn model_fields(self) -> Self {
        self.field(
            "provider",
            FieldKind::Select { options: ["openai", "anthropic", "gemini", "ollama"].map(String::from).to_vec() },
            Some(json!("openai")),
            false,
        )
        .field("model", FieldKind::Model { provider: None }, Some(json!("gpt-4o-mini")), false)
        .field("temperature", FieldKind::Number { min: Some(0.0), max: Some(2.0) }, None, false)
        .field("max_tokens", FieldKind::Number { min: Some(1.0), max: None }, None, false)
        .field("credential_id", FieldKind::Credential { provider: "llm".to_owned() }, None, false)
    }

    fn embedding_fields(self) -> Self {
        self.field("provider", FieldKind::Select { options: ["openai", "gemini", "ollama"].map(String::from).to_vec() }, None, false)
            .field("model", FieldKind::Model { provider: None }, Some(json!("text-embedding-3-small")), false)
            .field("credential_id", FieldKind::Credential { provider: "llm".to_owned() }, None, false)
    }

    fn build(self, op: Op) -> Arc<dyn NodeExecutor> {
        Arc::new(StdNode { spec: self.spec, op })
    }
}

fn message_trigger(kind: &str) -> Arc<dyn NodeExecutor> {
    Def::new(kind, "On Message", "events", "Fires when a message arrives")
        .trigger()
        .output("content", SocketType::String)
        .output("sender", SocketType::String)
        .output("payload", SocketType::Json)
        .build(Op::OnMessage)
}

fn generate_text(kind: &str) -> Arc<dyn NodeExecutor> {
    Def::new(kind, "Generate Text", "ai", "Asks the language model for a completion")
        .flow_in()
        .required("prompt", SocketType::String)
        .input("system", SocketType::String)
        .flow_out()
        .output("text", SocketType::String)
        .output("model", SocketType::String)
        .model_fields()
        .build(Op::GenerateText)
}

fn send_message(kind: &str) -> Arc<dyn NodeExecutor> {
    Def::new(kind, "Send Message", "io", "Sends a message back to the conversation")
        .flow_in()
        .required("content", SocketType::String)
        .flow_out()
        .output("sent", SocketType::String)
        .build(Op::SendMessage)
}

fn events() -> Vec<Arc<dyn NodeExecutor>> {
    vec![
        Def::new("event/on_start", "On Start", "events", "Fires once when the spell starts").trigger().output("payload", SocketType::Any).build(Op::OnStart),
        message_trigger("event/on_message"),
        message_trigger("core/on_message"),
        Def::new("event/on_webhook", "On Webhook", "events", "Fires when an HTTP webhook is received")
            .trigger()
            .output("method", SocketType::String)
            .output("path", SocketType::String)
            .output("headers", SocketType::Json)
            .output("query", SocketType::Json)
            .output("body", SocketType::Json)
            .build(Op::OnWebhook),
        Def::new("event/on_tick", "On Tick", "events", "Fires on a schedule")
            .trigger()
            .output("tick", SocketType::Int)
            .output("at", SocketType::String)
            .field("interval_ms", FieldKind::Number { min: Some(1.0), max: None }, Some(json!(60_000)), false)
            .build(Op::OnTick),
        Def::new("event/on_memory", "On Memory", "events", "Fires when a memory is stored")
            .trigger()
            .output("content", SocketType::String)
            .output("payload", SocketType::Json)
            .build(Op::OnMemory),
        Def::new("event/on_variable_changed", "On Variable Changed", "events", "Fires when a run variable changes")
            .trigger()
            .output("name", SocketType::String)
            .output("value", SocketType::Any)
            .build(Op::OnVariableChanged),
    ]
}

fn flow() -> Vec<Arc<dyn NodeExecutor>> {
    vec![
        Def::new("flow/branch", "Branch", "flow", "Routes flow on a condition")
            .flow_in()
            .required("condition", SocketType::Bool)
            .output("true", SocketType::Flow)
            .output("false", SocketType::Flow)
            .build(Op::Branch),
        Def::new("flow/switch", "Switch", "flow", "Routes flow to the output named after the value")
            .flow_in()
            .required("value", SocketType::Any)
            .output("default", SocketType::Flow)
            .output(DYNAMIC_SOCKET, SocketType::Flow)
            .field("cases", FieldKind::Json, Some(json!([])), true)
            .build(Op::Switch),
        Def::new("flow/split", "Split", "flow", "Fires every output at the same time")
            .flow_in()
            .output("out_0", SocketType::Flow)
            .output("out_1", SocketType::Flow)
            .output(DYNAMIC_SOCKET, SocketType::Flow)
            .field("outputs", FieldKind::Json, Some(json!(["out_0", "out_1"])), false)
            .build(Op::Split),
        Def::new("flow/wait_for_all", "Wait For All", "flow", "Fires once every incoming branch has arrived").flow_in().flow_out().build(Op::WaitForAll),
        Def::new("flow/while_loop", "While Loop", "flow", "Repeats the body while the condition holds")
            .flow_in()
            .required("condition", SocketType::Bool)
            .output("body", SocketType::Flow)
            .output("completed", SocketType::Flow)
            .field("max_iterations", FieldKind::Number { min: Some(1.0), max: None }, Some(json!(1000)), false)
            .build(Op::WhileLoop),
        Def::new("flow/subgraph", "Subgraph", "flow", "Runs another spell with the payload and returns its result, outputs and replies")
            .flow_in()
            .input("payload", SocketType::Any)
            .flow_out()
            .output("result", SocketType::Any)
            .output("outputs", SocketType::Json)
            .field("spell_id", FieldKind::Text, None, false)
            .build(Op::Subgraph),
        Def::new("flow/do_once", "Do Once", "flow", "Lets flow through only the first time until reset")
            .flow_in()
            .input("reset", SocketType::Flow)
            .flow_out()
            .build(Op::DoOnce),
        Def::new("time/delay", "Delay", "time", "Waits before continuing")
            .flow_in()
            .input("ms", SocketType::Int)
            .flow_out()
            .field("ms", FieldKind::Number { min: Some(0.0), max: None }, Some(json!(1000)), false)
            .build(Op::Delay),
    ]
}

fn variables() -> Vec<Arc<dyn NodeExecutor>> {
    let name = || FieldKind::Text;
    vec![
        Def::new("var/get", "Get Variable", "variables", "Reads a run variable")
            .output("value", SocketType::Any)
            .field("name", name(), None, true)
            .build(Op::VarGet),
        Def::new("var/set", "Set Variable", "variables", "Writes a run variable")
            .flow_in()
            .input("value", SocketType::Any)
            .flow_out()
            .output("value", SocketType::Any)
            .field("name", name(), None, true)
            .build(Op::VarSet),
        Def::new("var/reset", "Reset Variable", "variables", "Restores a run variable to its default")
            .flow_in()
            .flow_out()
            .output("value", SocketType::Any)
            .field("name", name(), None, true)
            .build(Op::VarReset),
    ]
}

fn text_and_logic() -> Vec<Arc<dyn NodeExecutor>> {
    vec![
        Def::new("text/template", "Template", "text", "Fills {{name}} placeholders from inputs and variables")
            .input(DYNAMIC_SOCKET, SocketType::Any)
            .output("text", SocketType::String)
            .field("template", FieldKind::TextArea, Some(json!("")), true)
            .build(Op::Template),
        Def::new("text/replace", "Replace", "text", "Replaces text, optionally with a regular expression")
            .required("text", SocketType::String)
            .input("find", SocketType::String)
            .input("replace_with", SocketType::String)
            .output("text", SocketType::String)
            .field("regex", FieldKind::Boolean, Some(json!(false)), false)
            .build(Op::Replace),
        Def::new("logic/regex_match", "Regex Match", "logic", "Tests text against a regular expression")
            .required("text", SocketType::String)
            .input("pattern", SocketType::String)
            .output("matched", SocketType::Bool)
            .output("first", SocketType::String)
            .output("captures", SocketType::Array)
            .build(Op::RegexMatch),
        Def::new("logic/validate", "Validate", "logic", "Checks a value against simple rules")
            .input("value", SocketType::Any)
            .output("valid", SocketType::Bool)
            .output("errors", SocketType::Array)
            .field("required_keys", FieldKind::Json, Some(json!([])), false)
            .field("pattern", FieldKind::Text, None, false)
            .field("non_empty", FieldKind::Boolean, Some(json!(true)), false)
            .build(Op::Validate),
        Def::new("json/parse", "Parse JSON", "json", "Parses text into JSON")
            .required("text", SocketType::String)
            .output("json", SocketType::Json)
            .build(Op::JsonParse),
        Def::new("json/stringify", "Stringify JSON", "json", "Turns a value into JSON text")
            .input("value", SocketType::Any)
            .output("text", SocketType::String)
            .field("pretty", FieldKind::Boolean, Some(json!(false)), false)
            .build(Op::JsonStringify),
        Def::new("json/destructure", "Destructure JSON", "json", "Splits a JSON object into one output per key")
            .required("json", SocketType::Json)
            .output(DYNAMIC_SOCKET, SocketType::Any)
            .field("keys", FieldKind::Json, Some(json!([])), true)
            .build(Op::JsonDestructure),
    ]
}

fn network_and_ai() -> Vec<Arc<dyn NodeExecutor>> {
    vec![
        Def::new("net/http_fetch", "HTTP Fetch", "network", "Calls an HTTP endpoint")
            .flow_in()
            .required("url", SocketType::String)
            .input("body", SocketType::Any)
            .flow_out()
            .output("status", SocketType::Int)
            .output("body", SocketType::String)
            .output("json", SocketType::Json)
            .field("method", FieldKind::Select { options: ["GET", "POST", "PUT", "PATCH", "DELETE"].map(String::from).to_vec() }, Some(json!("GET")), false)
            .field("headers", FieldKind::Json, Some(json!({})), false)
            .field("timeout_ms", FieldKind::Number { min: Some(1.0), max: None }, Some(json!(30_000)), false)
            .build(Op::HttpFetch),
        generate_text("ai/generate_text"),
        generate_text("core/generate_text"),
        Def::new("ai/stream_text", "Stream Text", "ai", "Streams a completion chunk by chunk")
            .flow_in()
            .required("prompt", SocketType::String)
            .input("system", SocketType::String)
            .flow_out()
            .output("text", SocketType::String)
            .model_fields()
            .build(Op::StreamText),
        Def::new("ai/structured_json", "Structured JSON", "ai", "Asks the model for JSON that matches a schema")
            .flow_in()
            .required("prompt", SocketType::String)
            .input("system", SocketType::String)
            .flow_out()
            .output("json", SocketType::Json)
            .model_fields()
            .field("schema", FieldKind::Json, Some(json!({})), false)
            .build(Op::StructuredJson),
        Def::new("ai/embedding", "Embedding", "ai", "Turns text into an embedding vector")
            .flow_in()
            .required("text", SocketType::String)
            .flow_out()
            .output("embedding", SocketType::Embedding)
            .embedding_fields()
            .build(Op::Embedding),
    ]
}

fn memory_and_knowledge() -> Vec<Arc<dyn NodeExecutor>> {
    let agent = |def: Def| def.field("agent_id", FieldKind::Text, None, false);
    vec![
        agent(Def::new("memory/add", "Add Memory", "memory", "Stores a memory"))
            .flow_in()
            .required("content", SocketType::String)
            .input("metadata", SocketType::Json)
            .flow_out()
            .output("id", SocketType::String)
            .build(Op::MemoryAdd),
        agent(Def::new("memory/search", "Search Memory", "memory", "Recalls related memories"))
            .flow_in()
            .required("query", SocketType::String)
            .flow_out()
            .output("results", SocketType::Array)
            .output("text", SocketType::String)
            .field("limit", FieldKind::Number { min: Some(1.0), max: Some(100.0) }, Some(json!(5)), false)
            .build(Op::MemorySearch),
        agent(Def::new("memory/clear", "Clear Memory", "memory", "Forgets every memory of the agent"))
            .flow_in()
            .flow_out()
            .output("removed", SocketType::Int)
            .build(Op::MemoryClear),
        agent(Def::new("knowledge/add", "Add Knowledge", "knowledge", "Stores a document in the knowledge base"))
            .flow_in()
            .input("title", SocketType::String)
            .required("content", SocketType::String)
            .flow_out()
            .output("doc_id", SocketType::String)
            .build(Op::KnowledgeAdd),
        agent(Def::new("knowledge/search", "Search Knowledge", "knowledge", "Finds matching knowledge chunks"))
            .flow_in()
            .required("query", SocketType::String)
            .flow_out()
            .output("results", SocketType::Array)
            .output("text", SocketType::String)
            .field("limit", FieldKind::Number { min: Some(1.0), max: Some(100.0) }, Some(json!(5)), false)
            .build(Op::KnowledgeSearch),
        Def::new("knowledge/chunk", "Chunk Text", "knowledge", "Splits text into overlapping chunks")
            .required("text", SocketType::String)
            .input("size", SocketType::Int)
            .input("overlap", SocketType::Int)
            .output("chunks", SocketType::Array)
            .build(Op::KnowledgeChunk),
    ]
}

fn io_and_script() -> Vec<Arc<dyn NodeExecutor>> {
    vec![
        send_message("io/send_message"),
        send_message("core/send_message"),
        Def::new("io/stream_message", "Stream Message", "io", "Streams a message to the conversation word by word")
            .flow_in()
            .required("content", SocketType::String)
            .flow_out()
            .output("sent", SocketType::String)
            .build(Op::StreamMessage),
        Def::new("code/rhai_script", "Rhai Script", "code", "Runs a sandboxed Rhai script")
            .flow_in()
            .input(DYNAMIC_SOCKET, SocketType::Any)
            .flow_out()
            .output("result", SocketType::Any)
            .output(DYNAMIC_SOCKET, SocketType::Any)
            .field("script", FieldKind::Code { language: "rhai".to_owned() }, Some(json!("")), true)
            .field("inputs", FieldKind::Json, Some(json!([])), false)
            .field("max_operations", FieldKind::Number { min: Some(1.0), max: Some(10_000_000.0) }, Some(json!(100_000)), false)
            .build(Op::RhaiScript),
    ]
}

pub(crate) fn standard_nodes() -> Vec<Arc<dyn NodeExecutor>> {
    [events(), flow(), variables(), text_and_logic(), network_and_ai(), memory_and_knowledge(), io_and_script()].into_iter().flatten().collect()
}
