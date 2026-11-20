use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use magicklala_graph::{LlmGateway, LlmMessage, LlmRequest, LlmResponse, LlmRole, LlmStream, LlmStreamChunk, NodeError, TokenUsage};
use serde_json::{Value as Json, json};

pub const BASE_URL_ENV: &str = "MAGICKLALA_LLM_BASE_URL";
pub const API_KEY_ENV: &str = "MAGICKLALA_LLM_API_KEY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    OpenAi,
    Anthropic,
    Gemini,
    Ollama,
}

impl LlmProvider {
    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Self::Anthropic,
            "gemini" | "google" => Self::Gemini,
            "ollama" => Self::Ollama,
            _ => Self::OpenAi,
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com",
            Self::Anthropic => "https://api.anthropic.com",
            Self::Gemini => "https://generativelanguage.googleapis.com",
            Self::Ollama => "http://127.0.0.1:11434",
        }
    }

    fn key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Ollama => "OLLAMA_API_KEY",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LlmGatewayConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub request_timeout: Option<Duration>,
}

impl LlmGatewayConfig {
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var(BASE_URL_ENV).ok().filter(|url| !url.is_empty()),
            api_key: std::env::var(API_KEY_ENV).ok().filter(|key| !key.is_empty()),
            request_timeout: None,
        }
    }
}

pub struct HttpLlmGateway {
    client: reqwest::Client,
    config: LlmGatewayConfig,
}

type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>> + Send>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Framing {
    ServerSentEvents,
    JsonLines,
}

impl HttpLlmGateway {
    pub fn new(config: LlmGatewayConfig) -> Self {
        let mut builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(10));
        if let Some(timeout) = config.request_timeout {
            builder = builder.timeout(timeout);
        }
        Self { client: builder.build().unwrap_or_default(), config }
    }

    pub fn from_env() -> Self {
        Self::new(LlmGatewayConfig::from_env())
    }

    fn base_url(&self, provider: LlmProvider) -> String {
        self.config.base_url.clone().unwrap_or_else(|| provider.default_base_url().to_owned()).trim_end_matches('/').to_owned()
    }

    fn api_key(&self, provider: LlmProvider) -> Option<String> {
        self.config.api_key.clone().or_else(|| std::env::var(provider.key_env()).ok()).filter(|key| !key.is_empty())
    }

    fn endpoint(&self, provider: LlmProvider, model: &str, streaming: bool) -> String {
        let base = self.base_url(provider);
        match (provider, streaming) {
            (LlmProvider::OpenAi, _) => format!("{base}/v1/chat/completions"),
            (LlmProvider::Anthropic, _) => format!("{base}/v1/messages"),
            (LlmProvider::Gemini, false) => format!("{base}/v1beta/models/{model}:generateContent"),
            (LlmProvider::Gemini, true) => format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse"),
            (LlmProvider::Ollama, _) => format!("{base}/api/chat"),
        }
    }

    async fn post(&self, provider: LlmProvider, url: &str, body: &Json) -> Result<reqwest::Response, NodeError> {
        let mut request = self.client.post(url).json(body);
        if let Some(key) = self.api_key(provider) {
            request = match provider {
                LlmProvider::Anthropic => request.header("x-api-key", key),
                LlmProvider::Gemini => request.header("x-goog-api-key", key),
                LlmProvider::OpenAi | LlmProvider::Ollama => request.bearer_auth(key),
            };
        }
        if provider == LlmProvider::Anthropic {
            request = request.header("anthropic-version", "2023-06-01");
        }
        let response = request.send().await.map_err(|error| NodeError::Llm(format!("request to {provider:?} failed: {error}")))?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let detail: String = response.text().await.unwrap_or_default().chars().take(500).collect();
        Err(NodeError::Llm(format!("{provider:?} returned {status}: {detail}")))
    }
}

fn role_name(role: LlmRole) -> &'static str {
    match role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
        LlmRole::Tool => "tool",
    }
}

fn system_text(messages: &[LlmMessage]) -> String {
    messages.iter().filter(|message| message.role == LlmRole::System).map(|message| message.content.as_str()).collect::<Vec<_>>().join("\n\n")
}

fn conversation(messages: &[LlmMessage]) -> impl Iterator<Item = &LlmMessage> {
    messages.iter().filter(|message| message.role != LlmRole::System)
}

fn request_body(provider: LlmProvider, request: &LlmRequest, streaming: bool) -> Json {
    match provider {
        LlmProvider::OpenAi => {
            let messages: Vec<Json> = request.messages.iter().map(|message| json!({ "role": role_name(message.role), "content": message.content })).collect();
            let mut body = json!({ "model": request.model, "messages": messages, "stream": streaming });
            insert_optional(&mut body, "temperature", request.temperature.map(Json::from));
            insert_optional(&mut body, "max_tokens", request.max_tokens.map(Json::from));
            if !request.stop.is_empty() {
                body["stop"] = json!(request.stop);
            }
            if streaming {
                body["stream_options"] = json!({ "include_usage": true });
            }
            body
        }
        LlmProvider::Anthropic => {
            let messages: Vec<Json> = conversation(&request.messages)
                .map(|message| json!({ "role": if message.role == LlmRole::Assistant { "assistant" } else { "user" }, "content": message.content }))
                .collect();
            let mut body = json!({ "model": request.model, "messages": messages, "max_tokens": request.max_tokens.unwrap_or(1024), "stream": streaming });
            let system = system_text(&request.messages);
            if !system.is_empty() {
                body["system"] = json!(system);
            }
            insert_optional(&mut body, "temperature", request.temperature.map(Json::from));
            if !request.stop.is_empty() {
                body["stop_sequences"] = json!(request.stop);
            }
            body
        }
        LlmProvider::Gemini => {
            let contents: Vec<Json> = conversation(&request.messages)
                .map(|message| json!({ "role": if message.role == LlmRole::Assistant { "model" } else { "user" }, "parts": [{ "text": message.content }] }))
                .collect();
            let mut generation = json!({});
            insert_optional(&mut generation, "temperature", request.temperature.map(Json::from));
            insert_optional(&mut generation, "maxOutputTokens", request.max_tokens.map(Json::from));
            if !request.stop.is_empty() {
                generation["stopSequences"] = json!(request.stop);
            }
            let mut body = json!({ "contents": contents, "generationConfig": generation });
            let system = system_text(&request.messages);
            if !system.is_empty() {
                body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
            }
            body
        }
        LlmProvider::Ollama => {
            let messages: Vec<Json> = request.messages.iter().map(|message| json!({ "role": role_name(message.role), "content": message.content })).collect();
            let mut options = json!({});
            insert_optional(&mut options, "temperature", request.temperature.map(Json::from));
            insert_optional(&mut options, "num_predict", request.max_tokens.map(Json::from));
            if !request.stop.is_empty() {
                options["stop"] = json!(request.stop);
            }
            json!({ "model": request.model, "messages": messages, "stream": streaming, "options": options })
        }
    }
}

fn insert_optional(target: &mut Json, key: &str, value: Option<Json>) {
    if let (Some(value), Some(object)) = (value, target.as_object_mut()) {
        object.insert(key.to_owned(), value);
    }
}

fn number(json: &Json, pointer: &str) -> u32 {
    json.pointer(pointer).and_then(Json::as_u64).and_then(|count| u32::try_from(count).ok()).unwrap_or_default()
}

fn text_at(json: &Json, pointer: &str) -> String {
    json.pointer(pointer).and_then(Json::as_str).unwrap_or_default().to_owned()
}

fn gemini_text(json: &Json) -> String {
    json.pointer("/candidates/0/content/parts")
        .and_then(Json::as_array)
        .map(|parts| parts.iter().filter_map(|part| part.get("text").and_then(Json::as_str)).collect::<String>())
        .unwrap_or_default()
}

fn parse_response(provider: LlmProvider, request: &LlmRequest, json: &Json) -> LlmResponse {
    let (text, finish, usage, model) = match provider {
        LlmProvider::OpenAi => (
            text_at(json, "/choices/0/message/content"),
            json.pointer("/choices/0/finish_reason").and_then(Json::as_str).map(str::to_owned),
            TokenUsage { prompt_tokens: number(json, "/usage/prompt_tokens"), completion_tokens: number(json, "/usage/completion_tokens") },
            text_at(json, "/model"),
        ),
        LlmProvider::Anthropic => (
            json.get("content")
                .and_then(Json::as_array)
                .map(|blocks| blocks.iter().filter_map(|block| block.get("text").and_then(Json::as_str)).collect::<String>())
                .unwrap_or_default(),
            json.get("stop_reason").and_then(Json::as_str).map(str::to_owned),
            TokenUsage { prompt_tokens: number(json, "/usage/input_tokens"), completion_tokens: number(json, "/usage/output_tokens") },
            text_at(json, "/model"),
        ),
        LlmProvider::Gemini => (
            gemini_text(json),
            json.pointer("/candidates/0/finishReason").and_then(Json::as_str).map(str::to_owned),
            TokenUsage {
                prompt_tokens: number(json, "/usageMetadata/promptTokenCount"),
                completion_tokens: number(json, "/usageMetadata/candidatesTokenCount"),
            },
            text_at(json, "/modelVersion"),
        ),
        LlmProvider::Ollama => (
            text_at(json, "/message/content"),
            json.get("done_reason").and_then(Json::as_str).map(str::to_owned),
            TokenUsage { prompt_tokens: number(json, "/prompt_eval_count"), completion_tokens: number(json, "/eval_count") },
            text_at(json, "/model"),
        ),
    };
    LlmResponse { text, model: if model.is_empty() { request.model.clone() } else { model }, usage, finish_reason: finish }
}

fn chunk_from_event(provider: LlmProvider, json: &Json) -> Option<LlmStreamChunk> {
    match provider {
        LlmProvider::OpenAi => {
            let usage = json
                .get("usage")
                .filter(|usage| usage.is_object())
                .map(|_| TokenUsage { prompt_tokens: number(json, "/usage/prompt_tokens"), completion_tokens: number(json, "/usage/completion_tokens") });
            Some(LlmStreamChunk { delta: text_at(json, "/choices/0/delta/content"), done: false, usage })
        }
        LlmProvider::Anthropic => match json.get("type").and_then(Json::as_str) {
            Some("content_block_delta") => Some(LlmStreamChunk { delta: text_at(json, "/delta/text"), done: false, usage: None }),
            Some("message_delta") => Some(LlmStreamChunk {
                delta: String::new(),
                done: false,
                usage: Some(TokenUsage { prompt_tokens: 0, completion_tokens: number(json, "/usage/output_tokens") }),
            }),
            Some("message_stop") => Some(LlmStreamChunk { delta: String::new(), done: true, usage: None }),
            _ => None,
        },
        LlmProvider::Gemini => Some(LlmStreamChunk { delta: gemini_text(json), done: false, usage: None }),
        LlmProvider::Ollama => {
            let done = json.get("done").and_then(Json::as_bool).unwrap_or(false);
            let usage = done.then(|| TokenUsage { prompt_tokens: number(json, "/prompt_eval_count"), completion_tokens: number(json, "/eval_count") });
            Some(LlmStreamChunk { delta: text_at(json, "/message/content"), done, usage })
        }
    }
}

struct StreamDecoder {
    bytes: ByteStream,
    buffer: Vec<u8>,
    pending: VecDeque<LlmStreamChunk>,
    provider: LlmProvider,
    framing: Framing,
    finished: bool,
    completed: bool,
}

impl StreamDecoder {
    fn take_line(&mut self) -> Option<String> {
        let end = self.buffer.iter().position(|byte| *byte == b'\n')?;
        let line: Vec<u8> = self.buffer.drain(..=end).collect();
        Some(String::from_utf8_lossy(&line).trim().to_owned())
    }

    fn accept_line(&mut self, line: &str) -> Result<(), NodeError> {
        let payload = match self.framing {
            Framing::ServerSentEvents => match line.strip_prefix("data:") {
                Some(data) => data.trim(),
                None => return Ok(()),
            },
            Framing::JsonLines => line,
        };
        if payload.is_empty() {
            return Ok(());
        }
        if payload == "[DONE]" {
            self.finish();
            return Ok(());
        }
        let json: Json = serde_json::from_str(payload).map_err(|error| NodeError::Llm(format!("malformed stream event: {error}")))?;
        if let Some(message) = json.pointer("/error/message").and_then(Json::as_str) {
            return Err(NodeError::Llm(message.to_owned()));
        }
        if let Some(chunk) = chunk_from_event(self.provider, &json) {
            let done = chunk.done;
            if !chunk.delta.is_empty() || chunk.usage.is_some() || done {
                self.pending.push_back(chunk);
            }
            if done {
                self.completed = true;
                self.finished = true;
            }
        }
        Ok(())
    }

    fn finish(&mut self) {
        if !self.completed {
            self.pending.push_back(LlmStreamChunk { delta: String::new(), done: true, usage: None });
            self.completed = true;
        }
        self.finished = true;
    }

    async fn next_chunk(mut self) -> Result<Option<(LlmStreamChunk, Self)>, NodeError> {
        loop {
            if let Some(chunk) = self.pending.pop_front() {
                return Ok(Some((chunk, self)));
            }
            if self.finished {
                return Ok(None);
            }
            if let Some(line) = self.take_line() {
                self.accept_line(&line)?;
                continue;
            }
            match self.bytes.next().await {
                Some(Ok(bytes)) => self.buffer.extend_from_slice(&bytes),
                Some(Err(error)) => return Err(NodeError::Llm(error)),
                None => {
                    let rest = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).trim().to_owned();
                    if !rest.is_empty() {
                        self.accept_line(&rest)?;
                    }
                    self.finish();
                }
            }
        }
    }
}

#[async_trait]
impl LlmGateway for HttpLlmGateway {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, NodeError> {
        let provider = LlmProvider::from_name(&request.provider);
        let url = self.endpoint(provider, &request.model, false);
        let response = self.post(provider, &url, &request_body(provider, &request, false)).await?;
        let json: Json = response.json().await.map_err(|error| NodeError::Llm(format!("invalid response body: {error}")))?;
        Ok(parse_response(provider, &request, &json))
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, NodeError> {
        let provider = LlmProvider::from_name(&request.provider);
        let url = self.endpoint(provider, &request.model, true);
        let response = self.post(provider, &url, &request_body(provider, &request, true)).await?;
        let bytes: ByteStream = Box::pin(response.bytes_stream().map(|item| item.map(|bytes| bytes.to_vec()).map_err(|error| error.to_string())));
        let framing = if provider == LlmProvider::Ollama { Framing::JsonLines } else { Framing::ServerSentEvents };
        let decoder = StreamDecoder { bytes, buffer: Vec::new(), pending: VecDeque::new(), provider, framing, finished: false, completed: false };
        Ok(Box::pin(futures::stream::try_unfold(decoder, StreamDecoder::next_chunk)))
    }

    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, NodeError> {
        let provider = LlmProvider::from_name(if model.starts_with("gemini") || model.starts_with("text-embedding-004") { "gemini" } else { "openai" });
        let provider = if std::env::var("MAGICKLALA_EMBEDDING_PROVIDER").is_ok_and(|name| name == "ollama") { LlmProvider::Ollama } else { provider };
        let base = self.base_url(provider);
        let (url, body, pointer) = match provider {
            LlmProvider::Gemini => {
                (format!("{base}/v1beta/models/{model}:embedContent"), json!({ "content": { "parts": [{ "text": text }] } }), "/embedding/values")
            }
            LlmProvider::Ollama => (format!("{base}/api/embed"), json!({ "model": model, "input": text }), "/embeddings/0"),
            LlmProvider::OpenAi | LlmProvider::Anthropic => (format!("{base}/v1/embeddings"), json!({ "model": model, "input": text }), "/data/0/embedding"),
        };
        let response = self.post(provider, &url, &body).await?;
        let json: Json = response.json().await.map_err(|error| NodeError::Llm(format!("invalid embedding body: {error}")))?;
        json.pointer(pointer)
            .and_then(Json::as_array)
            .map(|values| values.iter().filter_map(Json::as_f64).map(|value| value as f32).collect())
            .ok_or_else(|| NodeError::Llm("embedding response had no vector".to_owned()))
    }
}
