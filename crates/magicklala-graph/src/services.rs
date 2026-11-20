use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{NodeError, NodeInstance, NodeSpec, RunId, TraceEvent, Value};

#[async_trait]
pub trait NodeExecutor: Send + Sync {
    fn spec(&self) -> &NodeSpec;
    async fn execute(&self, ctx: &mut dyn NodeContext) -> Result<NodeOutcome, NodeError>;
}

#[async_trait]
pub trait NodeContext: Send {
    fn run_id(&self) -> RunId;
    fn node(&self) -> &NodeInstance;
    fn trigger_socket(&self) -> Option<&str>;
    async fn input(&mut self, socket: &str) -> Result<Option<Value>, NodeError>;
    fn variable(&self, name: &str) -> Option<Value>;
    fn variable_default(&self, _name: &str) -> Option<Value> {
        None
    }
    fn set_variable(&mut self, name: &str, value: Value) -> Result<(), NodeError>;
    fn llm(&self) -> Option<Arc<dyn LlmGateway>>;
    fn memory(&self) -> Option<Arc<dyn MemoryService>>;
    fn knowledge(&self) -> Option<Arc<dyn KnowledgeService>>;
    fn events(&self) -> Arc<dyn EventEmitter>;
    fn cancellation(&self) -> CancellationToken;
    fn agent_id(&self) -> Option<&str> {
        None
    }
    fn allow_private_http(&self) -> bool {
        false
    }
}

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    async fn resolve(&self, credential_id: &str) -> Result<String, NodeError>;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeOutcome {
    pub outputs: BTreeMap<String, Value>,
    pub fire: Vec<String>,
}

impl NodeOutcome {
    pub fn fire(socket: &str) -> Self {
        Self { outputs: BTreeMap::new(), fire: vec![socket.to_owned()] }
    }

    pub fn with_output(mut self, socket: &str, value: impl Into<Value>) -> Self {
        self.outputs.insert(socket.to_owned(), value.into());
        self
    }
}

pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: TraceEvent);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    pub provider: String,
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stop: Vec<String>,
    #[serde(default)]
    pub credential_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    pub text: String,
    pub model: String,
    pub usage: TokenUsage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmStreamChunk {
    pub delta: String,
    pub done: bool,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

pub type LlmStream = Pin<Box<dyn Stream<Item = Result<LlmStreamChunk, NodeError>> + Send>>;

#[async_trait]
pub trait LlmGateway: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, NodeError>;
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, NodeError>;
    async fn embed(&self, _model: &str, _text: &str) -> Result<Vec<f32>, NodeError> {
        Err(NodeError::Llm("this gateway does not support embeddings".to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: String,
    pub content: String,
    pub score: f32,
    pub metadata: serde_json::Value,
}

#[async_trait]
pub trait MemoryService: Send + Sync {
    async fn add(&self, agent_id: &str, content: &str, metadata: serde_json::Value) -> Result<String, NodeError>;
    async fn recall(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<MemoryItem>, NodeError>;
    async fn clear(&self, _agent_id: &str) -> Result<u64, NodeError> {
        Err(NodeError::Service("this memory service does not support clearing".to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHit {
    pub doc_id: String,
    pub chunk_index: u32,
    pub text: String,
    pub score: f32,
}

#[async_trait]
pub trait KnowledgeService: Send + Sync {
    async fn add(&self, agent_id: &str, title: &str, content: &str) -> Result<String, NodeError>;
    async fn search(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<KnowledgeHit>, NodeError>;
}
