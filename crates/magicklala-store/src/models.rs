use chrono::{DateTime, Utc};
use magicklala_graph::{RunStatus, SpellGraph};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($($name:ident),* $(,)?) => {
        $(
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type)]
            #[serde(transparent)]
            #[sqlx(transparent)]
            pub struct $name(pub Uuid);

            impl $name {
                pub fn new() -> Self {
                    Self(Uuid::now_v7())
                }
            }

            impl Default for $name {
                fn default() -> Self {
                    Self::new()
                }
            }
        )*
    };
}

id_type!(ProjectId, AgentId, SpellRecordId, ReleaseId, CredentialId, ChatMessageId, MemoryId, KnowledgeDocId, WebhookId, PluginStateId);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewProject {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub project_id: ProjectId,
    pub name: String,
    pub description: String,
    pub default_spell_id: Option<SpellRecordId>,
    pub pinned_release_id: Option<ReleaseId>,
    pub enabled: bool,
    pub settings: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewAgent {
    pub project_id: ProjectId,
    pub name: String,
    pub description: String,
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateAgent {
    pub name: Option<String>,
    pub description: Option<String>,
    pub default_spell_id: Option<Option<SpellRecordId>>,
    pub pinned_release_id: Option<Option<ReleaseId>>,
    pub enabled: Option<bool>,
    pub settings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpellRecord {
    pub id: SpellRecordId,
    pub project_id: ProjectId,
    pub name: String,
    pub graph: SpellGraph,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewSpell {
    pub project_id: ProjectId,
    pub name: String,
    pub graph: SpellGraph,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpellRelease {
    pub id: ReleaseId,
    pub spell_id: SpellRecordId,
    pub version: u32,
    pub notes: String,
    pub graph: SpellGraph,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewSpellRelease {
    pub spell_id: SpellRecordId,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMeta {
    pub id: CredentialId,
    pub project_id: ProjectId,
    pub name: String,
    pub provider: String,
    pub hint: String,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct NewCredential {
    pub project_id: ProjectId,
    pub name: String,
    pub provider: String,
    pub secret: String,
}

impl std::fmt::Debug for NewCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewCredential")
            .field("project_id", &self.project_id)
            .field("name", &self.name)
            .field("provider", &self.provider)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: ChatMessageId,
    pub agent_id: AgentId,
    pub channel: String,
    pub role: ChatRole,
    pub sender: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewChatMessage {
    pub agent_id: AgentId,
    pub channel: String,
    pub role: ChatRole,
    pub sender: String,
    pub content: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub agent_id: AgentId,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewMemory {
    pub agent_id: AgentId,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDoc {
    pub id: KnowledgeDocId,
    pub agent_id: AgentId,
    pub title: String,
    pub source_uri: Option<String>,
    pub mime: String,
    pub content_hash: String,
    pub chunk_count: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewKnowledgeDoc {
    pub agent_id: AgentId,
    pub title: String,
    pub source_uri: Option<String>,
    pub mime: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeChunk {
    pub doc_id: KnowledgeDocId,
    pub index: u32,
    pub text: String,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit<T> {
    pub item: T,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookRecord {
    pub id: WebhookId,
    pub agent_id: AgentId,
    pub path: String,
    pub secret_credential_id: Option<CredentialId>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewWebhook {
    pub agent_id: AgentId,
    pub path: String,
    pub secret_credential_id: Option<CredentialId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRun {
    pub run_id: Uuid,
    pub agent_id: Option<AgentId>,
    pub spell_id: SpellRecordId,
    pub release_id: Option<ReleaseId>,
    pub status: RunStatus,
    pub trigger: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub nodes_executed: u64,
    pub error: Option<String>,
    pub trace: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRequestLog {
    pub id: Uuid,
    pub run_id: Option<Uuid>,
    pub agent_id: Option<AgentId>,
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub latency_ms: u64,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginStateRecord {
    pub id: PluginStateId,
    pub agent_id: AgentId,
    pub plugin: String,
    pub state: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}
