use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use magicklala_graph::{EmbeddingRequest, LlmGateway, LlmRequest, LlmResponse, LlmStream, NodeError, TokenUsage};
use magicklala_store::{AgentId, LlmRequestLog, Repository};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LlmCallScope {
    pub agent_id: Option<AgentId>,
    pub run_id: Option<Uuid>,
}

pub struct MeteredLlmGateway {
    inner: Arc<dyn LlmGateway>,
    repository: Arc<dyn Repository>,
    scope: LlmCallScope,
}

impl MeteredLlmGateway {
    pub fn new(inner: Arc<dyn LlmGateway>, repository: Arc<dyn Repository>) -> Self {
        Self { inner, repository, scope: LlmCallScope::default() }
    }

    pub fn scoped(&self, scope: LlmCallScope) -> Self {
        Self { inner: Arc::clone(&self.inner), repository: Arc::clone(&self.repository), scope }
    }
}

fn error_status(error: &NodeError) -> &'static str {
    match error {
        NodeError::Cancelled => "cancelled",
        NodeError::Llm(_) => "llm_error",
        NodeError::InvalidConfig(_) => "invalid_config",
        _ => "error",
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

struct LogEntry<'a> {
    scope: LlmCallScope,
    provider: &'a str,
    model: &'a str,
    usage: TokenUsage,
    latency_ms: u64,
    status: &'a str,
}

async fn record(repository: &dyn Repository, entry: LogEntry<'_>) {
    let log = LlmRequestLog {
        id: Uuid::now_v7(),
        run_id: entry.scope.run_id,
        agent_id: entry.scope.agent_id,
        provider: entry.provider.to_owned(),
        model: entry.model.to_owned(),
        prompt_tokens: entry.usage.prompt_tokens,
        completion_tokens: entry.usage.completion_tokens,
        latency_ms: entry.latency_ms,
        status: entry.status.to_owned(),
        created_at: Utc::now(),
    };
    let _logged = repository.log_llm_request(log).await.is_ok();
}

#[async_trait]
impl LlmGateway for MeteredLlmGateway {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, NodeError> {
        let started = Instant::now();
        let result = self.inner.complete(request.clone()).await;
        let latency_ms = elapsed_ms(started);
        let (model, usage, status) = match &result {
            Ok(response) => (response.model.as_str(), response.usage, "ok"),
            Err(error) => (request.model.as_str(), TokenUsage::default(), error_status(error)),
        };
        record(self.repository.as_ref(), LogEntry { scope: self.scope, provider: &request.provider, model, usage, latency_ms, status }).await;
        result
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, NodeError> {
        let started = Instant::now();
        let opened = self.inner.stream(request.clone()).await;
        let inner = match opened {
            Ok(inner) => inner,
            Err(error) => {
                let status = error_status(&error);
                record(
                    self.repository.as_ref(),
                    LogEntry {
                        scope: self.scope,
                        provider: &request.provider,
                        model: &request.model,
                        usage: TokenUsage::default(),
                        latency_ms: elapsed_ms(started),
                        status,
                    },
                )
                .await;
                return Err(error);
            }
        };
        let tally = Arc::new(Mutex::new((TokenUsage::default(), "ok")));
        let observed = Arc::clone(&tally);
        let chunks = inner.inspect(move |chunk| {
            let mut state = observed.lock().unwrap_or_else(PoisonError::into_inner);
            match chunk {
                Ok(chunk) => {
                    if let Some(usage) = chunk.usage {
                        state.0 = usage;
                    }
                }
                Err(error) => state.1 = error_status(error),
            }
        });
        let repository = Arc::clone(&self.repository);
        let scope = self.scope;
        let finale = futures::stream::once(async move {
            let (usage, status) = *tally.lock().unwrap_or_else(PoisonError::into_inner);
            record(repository.as_ref(), LogEntry { scope, provider: &request.provider, model: &request.model, usage, latency_ms: elapsed_ms(started), status })
                .await;
            None
        });
        Ok(Box::pin(chunks.map(Some).chain(finale).filter_map(futures::future::ready)))
    }

    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, NodeError> {
        let started = Instant::now();
        let result = self.inner.embed(model, text).await;
        let status = result.as_ref().map_or_else(error_status, |_| "ok");
        let entry = LogEntry { scope: self.scope, provider: "", model, usage: TokenUsage::default(), latency_ms: elapsed_ms(started), status };
        record(self.repository.as_ref(), entry).await;
        result
    }

    async fn embed_request(&self, request: EmbeddingRequest) -> Result<Vec<Vec<f32>>, NodeError> {
        let started = Instant::now();
        let result = self.inner.embed_request(request.clone()).await;
        let status = result.as_ref().map_or_else(error_status, |_| "ok");
        let entry = LogEntry {
            scope: self.scope,
            provider: &request.provider,
            model: &request.model,
            usage: TokenUsage::default(),
            latency_ms: elapsed_ms(started),
            status,
        };
        record(self.repository.as_ref(), entry).await;
        result
    }
}
