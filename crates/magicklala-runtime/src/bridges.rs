use std::sync::Arc;

use async_trait::async_trait;
use magicklala_embedder::{ChunkConfig, DocumentInput, DocumentKind, Embedder, EmbedderError, KnowledgePipeline};
use magicklala_graph::{KnowledgeHit, KnowledgeService, MemoryItem, MemoryService, NodeError};
use magicklala_store::{AgentId, KnowledgeChunk, KnowledgeDocId, NewKnowledgeDoc, NewMemory, Repository};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone)]
pub struct SharedEmbedder(pub Arc<dyn Embedder>);

#[async_trait]
impl Embedder for SharedEmbedder {
    fn model(&self) -> &str {
        self.0.model()
    }

    fn dimensions(&self) -> usize {
        self.0.dimensions()
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError> {
        self.0.embed(texts).await
    }
}

fn agent(agent_id: &str) -> Result<AgentId, NodeError> {
    Uuid::parse_str(agent_id).map(AgentId).map_err(|_| NodeError::Service(format!("agent id {agent_id:?} is not a UUID")))
}

fn service(error: impl std::fmt::Display) -> NodeError {
    NodeError::Service(error.to_string())
}

async fn embed_one(embedder: &dyn Embedder, text: &str) -> Result<Vec<f32>, NodeError> {
    embedder.embed(&[text.to_owned()]).await.map_err(service)?.into_iter().next().ok_or_else(|| NodeError::Service("embedder returned no vector".to_owned()))
}

fn clamp_limit(limit: usize) -> u32 {
    u32::try_from(limit.clamp(1, 100)).unwrap_or(100)
}

fn trusted_agent(pinned: Option<AgentId>, requested: &str) -> Result<AgentId, NodeError> {
    pinned.map_or_else(|| agent(requested), Ok)
}

pub struct StoreMemoryBridge {
    repository: Arc<dyn Repository>,
    embedder: Arc<dyn Embedder>,
    pinned: Option<AgentId>,
}

impl StoreMemoryBridge {
    pub fn new(repository: Arc<dyn Repository>, embedder: Arc<dyn Embedder>) -> Self {
        Self { repository, embedder, pinned: None }
    }

    pub fn pinned_to(mut self, agent_id: AgentId) -> Self {
        self.pinned = Some(agent_id);
        self
    }

    pub fn pinned_agent(&self) -> Option<AgentId> {
        self.pinned
    }
}

#[async_trait]
impl MemoryService for StoreMemoryBridge {
    async fn add(&self, agent_id: &str, content: &str, metadata: serde_json::Value) -> Result<String, NodeError> {
        let agent_id = trusted_agent(self.pinned, agent_id)?;
        if content.trim().is_empty() {
            return Err(NodeError::Service("memory content is empty".to_owned()));
        }
        let embedding = embed_one(self.embedder.as_ref(), content).await?;
        let record =
            self.repository.add_memory(NewMemory { agent_id, content: content.to_owned(), embedding: Some(embedding), metadata }).await.map_err(service)?;
        Ok(record.id.0.to_string())
    }

    async fn recall(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<MemoryItem>, NodeError> {
        let agent_id = trusted_agent(self.pinned, agent_id)?;
        let embedding = embed_one(self.embedder.as_ref(), query).await?;
        let hits = self.repository.search_memories(agent_id, &embedding, clamp_limit(limit)).await.map_err(service)?;
        Ok(hits
            .into_iter()
            .map(|hit| MemoryItem { id: hit.item.id.0.to_string(), content: hit.item.content, score: hit.score, metadata: hit.item.metadata })
            .collect())
    }

    async fn clear(&self, agent_id: &str) -> Result<u64, NodeError> {
        let agent_id = trusted_agent(self.pinned, agent_id)?;
        self.repository.clear_memories(agent_id).await.map_err(service)
    }
}

pub const UNBOUND_STORE_ACCESS: &str = "direct spell run requires agent_id in the same project to access memory or knowledge";

#[derive(Debug, Clone, Copy, Default)]
pub struct UnboundAgentStore;

impl UnboundAgentStore {
    fn refuse<T>() -> Result<T, NodeError> {
        Err(NodeError::InvalidConfig(UNBOUND_STORE_ACCESS.to_owned()))
    }
}

#[async_trait]
impl MemoryService for UnboundAgentStore {
    async fn add(&self, _agent_id: &str, _content: &str, _metadata: serde_json::Value) -> Result<String, NodeError> {
        Self::refuse()
    }

    async fn recall(&self, _agent_id: &str, _query: &str, _limit: usize) -> Result<Vec<MemoryItem>, NodeError> {
        Self::refuse()
    }

    async fn clear(&self, _agent_id: &str) -> Result<u64, NodeError> {
        Self::refuse()
    }
}

#[async_trait]
impl KnowledgeService for UnboundAgentStore {
    async fn add(&self, _agent_id: &str, _title: &str, _content: &str) -> Result<String, NodeError> {
        Self::refuse()
    }

    async fn search(&self, _agent_id: &str, _query: &str, _limit: usize) -> Result<Vec<KnowledgeHit>, NodeError> {
        Self::refuse()
    }
}

pub struct StoreKnowledgeBridge {
    repository: Arc<dyn Repository>,
    embedder: Arc<dyn Embedder>,
    chunking: ChunkConfig,
    pinned: Option<AgentId>,
}

impl StoreKnowledgeBridge {
    pub fn new(repository: Arc<dyn Repository>, embedder: Arc<dyn Embedder>) -> Self {
        Self { repository, embedder, chunking: ChunkConfig::default(), pinned: None }
    }

    pub fn pinned_to(mut self, agent_id: AgentId) -> Self {
        self.pinned = Some(agent_id);
        self
    }

    pub fn pinned_agent(&self) -> Option<AgentId> {
        self.pinned
    }

    pub fn with_chunking(mut self, chunking: ChunkConfig) -> Self {
        self.chunking = chunking;
        self
    }
}

#[async_trait]
impl KnowledgeService for StoreKnowledgeBridge {
    async fn add(&self, agent_id: &str, title: &str, content: &str) -> Result<String, NodeError> {
        let agent_id = trusted_agent(self.pinned, agent_id)?;
        let pipeline = KnowledgePipeline::new(SharedEmbedder(Arc::clone(&self.embedder)), self.chunking).map_err(service)?;
        let document = DocumentInput { title: title.to_owned(), kind: DocumentKind::Markdown, source_uri: None, bytes: content.as_bytes().to_vec() };
        let doc_id = KnowledgeDocId::new();
        let chunks = pipeline
            .ingest(&document)
            .await
            .map_err(service)?
            .into_iter()
            .map(|chunk| KnowledgeChunk { doc_id, index: chunk.index, text: chunk.text, embedding: Some(chunk.embedding) })
            .collect();
        let content_hash = hex::encode(Sha256::digest(content.as_bytes()));
        let doc = NewKnowledgeDoc { agent_id, title: title.to_owned(), source_uri: None, mime: "text/markdown".to_owned(), content_hash };
        let stored = self.repository.add_knowledge_doc(doc, chunks).await.map_err(service)?;
        Ok(stored.id.0.to_string())
    }

    async fn search(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<KnowledgeHit>, NodeError> {
        let agent_id = trusted_agent(self.pinned, agent_id)?;
        let embedding = embed_one(self.embedder.as_ref(), query).await?;
        let hits = self.repository.search_knowledge(agent_id, query, &embedding, clamp_limit(limit)).await.map_err(service)?;
        Ok(hits
            .into_iter()
            .map(|hit| KnowledgeHit { doc_id: hit.item.doc_id.0.to_string(), chunk_index: hit.item.index, text: hit.item.text, score: hit.score })
            .collect())
    }
}
