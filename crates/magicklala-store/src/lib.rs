mod models;
mod sqlite;
mod vault;

use std::str::FromStr;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use thiserror::Error;

pub use models::*;
pub use sqlite::{POSTGRES_SCHEMA, cosine_similarity};
pub use vault::{AesGcmVault, CryptoVault, MASTER_KEY_ENV};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("crypto failure")]
    Crypto,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
}

pub type StoreResult<T> = Result<T, StoreError>;

pub fn is_in_memory_database_url(url: &str) -> bool {
    let url = url.trim();
    let target = url.strip_prefix("sqlite:").unwrap_or(url);
    let target = target.strip_prefix("//").unwrap_or(target);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    path == ":memory:" || query.split('&').filter_map(|pair| pair.split_once('=')).any(|(key, value)| key == "mode" && value == "memory")
}

pub fn normalize_webhook_path(raw: &str) -> StoreResult<String> {
    let segments: Vec<&str> = raw.trim().split('/').map(str::trim).filter(|segment| !segment.is_empty()).collect();
    if segments.is_empty() {
        return Err(StoreError::Invalid("webhook path cannot be empty".to_owned()));
    }
    if segments.iter().any(|segment| *segment == ".." || *segment == ".") {
        return Err(StoreError::Invalid("webhook path cannot contain \"..\" segments".to_owned()));
    }
    if segments.iter().any(|segment| segment.chars().any(|c| c.is_control() || c == '\\' || c == '?' || c == '#')) {
        return Err(StoreError::Invalid("webhook path cannot contain control characters, backslashes, query or fragment markers".to_owned()));
    }
    Ok(segments.join("/"))
}

#[async_trait::async_trait]
pub trait Repository: Send + Sync {
    async fn create_project(&self, input: NewProject) -> StoreResult<Project>;
    async fn list_projects(&self) -> StoreResult<Vec<Project>>;
    async fn create_agent(&self, input: NewAgent) -> StoreResult<Agent>;
    async fn update_agent(&self, id: AgentId, input: UpdateAgent) -> StoreResult<Agent>;
    async fn get_agent(&self, id: AgentId) -> StoreResult<Agent>;
    async fn list_agents(&self, project: ProjectId) -> StoreResult<Vec<Agent>>;
    async fn delete_agent(&self, id: AgentId) -> StoreResult<()>;
    async fn create_spell(&self, input: NewSpell) -> StoreResult<SpellRecord>;
    async fn save_spell_graph(&self, id: SpellRecordId, graph: magicklala_graph::SpellGraph) -> StoreResult<SpellRecord>;
    async fn get_spell(&self, id: SpellRecordId) -> StoreResult<SpellRecord>;
    async fn list_spells(&self, project: ProjectId) -> StoreResult<Vec<SpellRecord>>;
    async fn create_release(&self, input: NewSpellRelease) -> StoreResult<SpellRelease>;
    async fn create_release_with_graph(&self, input: NewSpellRelease, graph: Option<magicklala_graph::SpellGraph>) -> StoreResult<SpellRelease>;
    async fn list_releases(&self, spell: SpellRecordId) -> StoreResult<Vec<SpellRelease>>;
    async fn put_credential(&self, input: NewCredential, vault: &dyn CryptoVault) -> StoreResult<CredentialMeta>;
    async fn reveal_credential(&self, id: CredentialId, vault: &dyn CryptoVault) -> StoreResult<String>;
    async fn list_credentials(&self, project: ProjectId) -> StoreResult<Vec<CredentialMeta>>;
    async fn delete_credential(&self, id: CredentialId) -> StoreResult<()>;
    async fn delete_webhook(&self, id: WebhookId) -> StoreResult<()> {
        Err(StoreError::Invalid(format!("deleting webhook {} is not supported", id.0)))
    }
    async fn clear_memories(&self, _agent: AgentId) -> StoreResult<u64> {
        Err(StoreError::Invalid("clearing memories is not supported".to_owned()))
    }
    async fn append_chat(&self, input: NewChatMessage) -> StoreResult<ChatMessage>;
    async fn chat_history(&self, agent: AgentId, channel: &str, limit: u32) -> StoreResult<Vec<ChatMessage>>;
    async fn add_memory(&self, input: NewMemory) -> StoreResult<MemoryRecord>;
    async fn search_memories(&self, agent: AgentId, embedding: &[f32], limit: u32) -> StoreResult<Vec<SearchHit<MemoryRecord>>>;
    async fn add_knowledge_doc(&self, input: NewKnowledgeDoc, chunks: Vec<KnowledgeChunk>) -> StoreResult<KnowledgeDoc>;
    async fn search_knowledge(&self, agent: AgentId, query: &str, embedding: &[f32], limit: u32) -> StoreResult<Vec<SearchHit<KnowledgeChunk>>>;
    async fn create_webhook(&self, input: NewWebhook) -> StoreResult<WebhookRecord>;
    async fn find_webhook(&self, path: &str) -> StoreResult<WebhookRecord>;
    async fn record_run(&self, run: ExecutionRun) -> StoreResult<()>;
    async fn list_runs(&self, agent: AgentId, limit: u32) -> StoreResult<Vec<ExecutionRun>>;
    async fn find_run_by_correlation(&self, _agent: AgentId, _correlation_id: &str) -> StoreResult<Option<ExecutionRun>> {
        Ok(None)
    }
    async fn log_llm_request(&self, log: LlmRequestLog) -> StoreResult<()>;
    async fn get_plugin_state(&self, agent: AgentId, plugin: &str) -> StoreResult<Option<PluginStateRecord>>;
    async fn put_plugin_state(&self, state: PluginStateRecord) -> StoreResult<()>;
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn connect(url: &str) -> StoreResult<Self> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(10));
        let pool = if is_in_memory_database_url(url) {
            SqlitePoolOptions::new().max_connections(1).min_connections(1).idle_timeout(None).max_lifetime(None)
        } else {
            SqlitePoolOptions::new().max_connections(8)
        }
        .connect_with(options)
        .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
