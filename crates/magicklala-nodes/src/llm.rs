use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use magicklala_graph::{
    CredentialResolver, EmbeddingRequest, LlmGateway, LlmMessage, LlmRequest, LlmResponse, LlmRole, LlmStream, LlmStreamChunk, NodeError, TokenUsage,
};
use serde_json::{Value as Json, json};

pub const BASE_URL_ENV: &str = "MAGICKLALA_LLM_BASE_URL";
pub const API_KEY_ENV: &str = "MAGICKLALA_LLM_API_KEY";
pub const MAX_STREAM_FRAME_BYTES: usize = 1_048_576;

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

    fn has_completion_marker(self) -> bool {
        matches!(self, Self::OpenAi | Self::Anthropic | Self::Ollama)
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
    credentials: Option<Arc<dyn CredentialResolver>>,
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
        Self { client: builder.build().unwrap_or_default(), config, credentials: None }
    }

    pub fn from_env() -> Self {
        Self::new(LlmGatewayConfig::from_env())
    }

    pub fn with_credential_resolver(mut self, resolver: Arc<dyn CredentialResolver>) -> Self {
        self.credentials = Some(resolver);
        self
    }

    async fn request_key(&self, provider: LlmProvider, credential_id: Option<&str>) -> Result<Option<String>, NodeError> {
        let Some(credential_id) = credential_id.filter(|id| !id.is_empty()) else {
            return Ok(self.api_key(provider));
        };
        let resolver = self
            .credentials
            .as_ref()
            .ok_or_else(|| NodeError::InvalidConfig(format!("credential {credential_id} cannot be resolved: no credential resolver is configured")))?;
        match resolver.resolve(credential_id).await {
            Ok(secret) if !secret.is_empty() => Ok(Some(secret)),
            Ok(_) => Err(NodeError::InvalidConfig(format!("credential {credential_id} cannot be resolved: it is empty"))),
            Err(error) => Err(NodeError::InvalidConfig(format!("credential {credential_id} cannot be resolved: {error}"))),
        }
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

    async fn post(&self, provider: LlmProvider, url: &str, body: &Json, key: Option<String>) -> Result<reqwest::Response, NodeError> {
        let mut request = self.client.post(url).json(body);
        if let Some(key) = key {
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
        LlmProvider::Gemini => {
            let usage = json.get("usageMetadata").filter(|usage| usage.is_object()).map(|_| TokenUsage {
                prompt_tokens: number(json, "/usageMetadata/promptTokenCount"),
                completion_tokens: number(json, "/usageMetadata/candidatesTokenCount"),
            });
            Some(LlmStreamChunk { delta: gemini_text(json), done: false, usage })
        }
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
    finish_reason_seen: bool,
}

fn frame_too_large() -> NodeError {
    NodeError::Llm("stream frame exceeds 1 MiB".to_owned())
}

fn gemini_finish_reason(json: &Json) -> bool {
    json.get("candidates").and_then(Json::as_array).is_some_and(|candidates| {
        candidates.iter().any(|candidate| candidate.get("finishReason").and_then(Json::as_str).is_some_and(|reason| !reason.is_empty()))
    })
}

fn sse_data(event: &str) -> Option<String> {
    let data: Vec<&str> = event
        .split('\n')
        .filter_map(|line| line.strip_prefix("data"))
        .filter_map(|rest| if rest.is_empty() { Some("") } else { rest.strip_prefix(':') })
        .map(|value| value.strip_prefix(' ').unwrap_or(value))
        .collect();
    if data.is_empty() { None } else { Some(data.join("\n")) }
}

impl StreamDecoder {
    fn new(bytes: ByteStream, provider: LlmProvider) -> Self {
        let framing = if provider == LlmProvider::Ollama { Framing::JsonLines } else { Framing::ServerSentEvents };
        Self { bytes, buffer: Vec::new(), pending: VecDeque::new(), provider, framing, finished: false, completed: false, finish_reason_seen: false }
    }

    fn take_frame(&mut self) -> Result<Option<String>, NodeError> {
        let delimiter: &[u8] = match self.framing {
            Framing::ServerSentEvents => b"\n\n",
            Framing::JsonLines => b"\n",
        };
        let searchable = self.buffer.len().min(MAX_STREAM_FRAME_BYTES + delimiter.len());
        let Some(end) = self.buffer[..searchable].windows(delimiter.len()).position(|window| window == delimiter) else {
            return if self.buffer.len() > MAX_STREAM_FRAME_BYTES { Err(frame_too_large()) } else { Ok(None) };
        };
        if end > MAX_STREAM_FRAME_BYTES {
            return Err(frame_too_large());
        }
        let frame: Vec<u8> = self.buffer.drain(..end + delimiter.len()).collect();
        Ok(Some(String::from_utf8_lossy(&frame[..end]).into_owned()))
    }

    fn ingest(&mut self, bytes: &[u8]) {
        self.buffer.extend(bytes.iter().copied().filter(|byte| *byte != b'\r'));
    }

    fn accept_frame(&mut self, frame: &str) -> Result<(), NodeError> {
        let payload = match self.framing {
            Framing::ServerSentEvents => match sse_data(frame) {
                Some(data) => data,
                None => return Ok(()),
            },
            Framing::JsonLines => frame.trim().to_owned(),
        };
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(());
        }
        if payload == "[DONE]" {
            return self.accept_done_marker();
        }
        let json: Json = serde_json::from_str(payload).map_err(|error| NodeError::Llm(format!("malformed stream event: {error}")))?;
        if let Some(message) = json.pointer("/error/message").and_then(Json::as_str) {
            return Err(NodeError::Llm(message.to_owned()));
        }
        if self.provider == LlmProvider::Gemini && gemini_finish_reason(&json) {
            self.finish_reason_seen = true;
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

    fn accept_done_marker(&mut self) -> Result<(), NodeError> {
        match self.provider {
            LlmProvider::OpenAi => {
                self.finish();
                Ok(())
            }
            LlmProvider::Gemini if self.finish_reason_seen => Ok(()),
            LlmProvider::Gemini => Err(NodeError::Llm("gemini stream ended without a finishReason: [DONE] is not a gemini end marker".to_owned())),
            LlmProvider::Anthropic | LlmProvider::Ollama if self.completed => Ok(()),
            LlmProvider::Anthropic => Err(NodeError::Llm("anthropic stream sent [DONE] instead of message_stop".to_owned())),
            LlmProvider::Ollama => Err(NodeError::Llm("ollama stream sent [DONE] instead of a done: true message".to_owned())),
        }
    }

    fn finish(&mut self) {
        if !self.completed {
            self.pending.push_back(LlmStreamChunk { delta: String::new(), done: true, usage: None });
            self.completed = true;
        }
        self.finished = true;
    }

    fn close(&mut self) -> Result<(), NodeError> {
        if self.buffer.len() > MAX_STREAM_FRAME_BYTES {
            return Err(frame_too_large());
        }
        let rest = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
        if !rest.trim().is_empty() {
            self.accept_frame(&rest)?;
        }
        if self.completed {
            self.finished = true;
            return Ok(());
        }
        if self.provider.has_completion_marker() {
            return Err(NodeError::Llm("upstream stream closed before completion marker".to_owned()));
        }
        if self.provider == LlmProvider::Gemini && !self.finish_reason_seen {
            return Err(NodeError::Llm("gemini stream ended without a finishReason".to_owned()));
        }
        self.finish();
        Ok(())
    }

    async fn next_chunk(mut self) -> Result<Option<(LlmStreamChunk, Self)>, NodeError> {
        loop {
            if let Some(chunk) = self.pending.pop_front() {
                return Ok(Some((chunk, self)));
            }
            if self.finished {
                return Ok(None);
            }
            if let Some(frame) = self.take_frame()? {
                self.accept_frame(&frame)?;
                continue;
            }
            match self.bytes.next().await {
                Some(Ok(bytes)) => self.ingest(&bytes),
                Some(Err(error)) => return Err(NodeError::Llm(error)),
                None => self.close()?,
            }
        }
    }
}

#[async_trait]
impl LlmGateway for HttpLlmGateway {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, NodeError> {
        let provider = LlmProvider::from_name(&request.provider);
        let key = self.request_key(provider, request.credential_id.as_deref()).await?;
        let url = self.endpoint(provider, &request.model, false);
        let response = self.post(provider, &url, &request_body(provider, &request, false), key).await?;
        let json: Json = response.json().await.map_err(|error| NodeError::Llm(format!("invalid response body: {error}")))?;
        Ok(parse_response(provider, &request, &json))
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, NodeError> {
        let provider = LlmProvider::from_name(&request.provider);
        let key = self.request_key(provider, request.credential_id.as_deref()).await?;
        let url = self.endpoint(provider, &request.model, true);
        let response = self.post(provider, &url, &request_body(provider, &request, true), key).await?;
        let bytes: ByteStream = Box::pin(response.bytes_stream().map(|item| item.map(|bytes| bytes.to_vec()).map_err(|error| error.to_string())));
        let decoder = StreamDecoder::new(bytes, provider);
        Ok(Box::pin(futures::stream::try_unfold(decoder, StreamDecoder::next_chunk)))
    }

    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, NodeError> {
        let request = EmbeddingRequest { provider: String::new(), model: model.to_owned(), inputs: vec![text.to_owned()], credential_id: None };
        self.embed_request(request).await?.into_iter().next().ok_or_else(|| NodeError::Llm("embedding response had no vector".to_owned()))
    }

    async fn embed_request(&self, request: EmbeddingRequest) -> Result<Vec<Vec<f32>>, NodeError> {
        let provider = embedding_provider(&request);
        if provider == LlmProvider::Anthropic {
            return Err(NodeError::InvalidConfig("anthropic does not provide embeddings".to_owned()));
        }
        let key = self.request_key(provider, request.credential_id.as_deref()).await?;
        if request.inputs.is_empty() {
            return Ok(Vec::new());
        }
        let base = self.base_url(provider);
        let model = &request.model;
        let (url, body) = match provider {
            LlmProvider::Gemini => {
                let requests: Vec<Json> =
                    request.inputs.iter().map(|input| json!({ "model": format!("models/{model}"), "content": { "parts": [{ "text": input }] } })).collect();
                (format!("{base}/v1beta/models/{model}:batchEmbedContents"), json!({ "requests": requests }))
            }
            LlmProvider::Ollama => (format!("{base}/api/embed"), json!({ "model": model, "input": request.inputs })),
            LlmProvider::OpenAi | LlmProvider::Anthropic => (format!("{base}/v1/embeddings"), json!({ "model": model, "input": request.inputs })),
        };
        let response = self.post(provider, &url, &body, key).await?;
        let json: Json = response.json().await.map_err(|error| NodeError::Llm(format!("invalid embedding body: {error}")))?;
        let vectors = embedding_vectors(provider, &json)?;
        if vectors.len() != request.inputs.len() {
            return Err(NodeError::Llm(format!("embedding response had {} vectors for {} inputs", vectors.len(), request.inputs.len())));
        }
        Ok(vectors)
    }
}

fn embedding_provider(request: &EmbeddingRequest) -> LlmProvider {
    if !request.provider.is_empty() {
        return LlmProvider::from_name(&request.provider);
    }
    if std::env::var("MAGICKLALA_EMBEDDING_PROVIDER").is_ok_and(|name| name == "ollama") {
        return LlmProvider::Ollama;
    }
    if request.model.starts_with("gemini") || request.model.starts_with("text-embedding-004") { LlmProvider::Gemini } else { LlmProvider::OpenAi }
}

fn float_vector(values: &Json) -> Option<Vec<f32>> {
    values.as_array().map(|values| values.iter().filter_map(Json::as_f64).map(|value| value as f32).collect())
}

fn embedding_vectors(provider: LlmProvider, json: &Json) -> Result<Vec<Vec<f32>>, NodeError> {
    let missing = || NodeError::Llm("embedding response had no vector".to_owned());
    match provider {
        LlmProvider::Gemini => json
            .get("embeddings")
            .and_then(Json::as_array)
            .ok_or_else(missing)?
            .iter()
            .map(|embedding| embedding.get("values").and_then(float_vector).ok_or_else(missing))
            .collect(),
        LlmProvider::Ollama => {
            json.get("embeddings").and_then(Json::as_array).ok_or_else(missing)?.iter().map(|vector| float_vector(vector).ok_or_else(missing)).collect()
        }
        LlmProvider::OpenAi | LlmProvider::Anthropic => {
            let mut rows: Vec<(u64, Vec<f32>)> = json
                .get("data")
                .and_then(Json::as_array)
                .ok_or_else(missing)?
                .iter()
                .enumerate()
                .map(|(position, row)| {
                    let index = row.get("index").and_then(Json::as_u64).unwrap_or(position as u64);
                    row.get("embedding").and_then(float_vector).map(|vector| (index, vector)).ok_or_else(missing)
                })
                .collect::<Result<_, _>>()?;
            rows.sort_by_key(|(index, _)| *index);
            Ok(rows.into_iter().map(|(_, vector)| vector).collect())
        }
    }
}
