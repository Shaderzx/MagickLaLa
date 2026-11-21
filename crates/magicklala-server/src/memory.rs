use std::collections::BTreeMap;

use chrono::Utc;
use magicklala_graph::SpellGraph;
use magicklala_store::*;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::embedding::cosine;

#[derive(Default)]
struct Tables {
    projects: BTreeMap<ProjectId, Project>,
    agents: BTreeMap<AgentId, Agent>,
    spells: BTreeMap<SpellRecordId, SpellRecord>,
    releases: BTreeMap<ReleaseId, SpellRelease>,
    credentials: BTreeMap<CredentialId, (CredentialMeta, Vec<u8>)>,
    chats: Vec<ChatMessage>,
    memories: Vec<MemoryRecord>,
    knowledge_docs: BTreeMap<KnowledgeDocId, KnowledgeDoc>,
    knowledge_chunks: Vec<(AgentId, KnowledgeChunk)>,
    webhooks: BTreeMap<WebhookId, WebhookRecord>,
    webhook_deliveries: BTreeMap<(WebhookId, String), i64>,
    runs: Vec<ExecutionRun>,
    llm_logs: Vec<LlmRequestLog>,
    plugin_states: BTreeMap<(AgentId, String), PluginStateRecord>,
}

#[derive(Default)]
pub struct InMemoryRepository {
    tables: RwLock<Tables>,
}

impl InMemoryRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn graph_hash(graph: &SpellGraph) -> StoreResult<String> {
    Ok(content_hash(&serde_json::to_vec(graph)?))
}

fn secret_hint(secret: &str) -> String {
    let tail: String = secret.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

fn require_name(name: &str, what: &str) -> StoreResult<()> {
    if name.trim().is_empty() { Err(StoreError::Invalid(format!("{what} name must not be empty"))) } else { Ok(()) }
}

fn ranked<T>(mut hits: Vec<SearchHit<T>>, limit: u32) -> Vec<SearchHit<T>> {
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    hits
}

impl Tables {
    fn project(&self, id: ProjectId) -> StoreResult<&Project> {
        self.projects.get(&id).ok_or_else(|| StoreError::NotFound(format!("project {}", id.0)))
    }

    fn agent(&self, id: AgentId) -> StoreResult<&Agent> {
        self.agents.get(&id).ok_or_else(|| StoreError::NotFound(format!("agent {}", id.0)))
    }

    fn spell(&self, id: SpellRecordId) -> StoreResult<&SpellRecord> {
        self.spells.get(&id).ok_or_else(|| StoreError::NotFound(format!("spell {}", id.0)))
    }
}

#[async_trait::async_trait]
impl Repository for InMemoryRepository {
    async fn create_project(&self, input: NewProject) -> StoreResult<Project> {
        require_name(&input.name, "project")?;
        let mut tables = self.tables.write().await;
        if tables.projects.values().any(|project| project.name == input.name) {
            return Err(StoreError::Conflict(format!("project {} already exists", input.name)));
        }
        let now = Utc::now();
        let project = Project { id: ProjectId::new(), name: input.name, description: input.description, created_at: now, updated_at: now };
        tables.projects.insert(project.id, project.clone());
        Ok(project)
    }

    async fn list_projects(&self) -> StoreResult<Vec<Project>> {
        Ok(self.tables.read().await.projects.values().cloned().collect())
    }

    async fn create_agent(&self, input: NewAgent) -> StoreResult<Agent> {
        require_name(&input.name, "agent")?;
        let mut tables = self.tables.write().await;
        tables.project(input.project_id)?;
        let now = Utc::now();
        let agent = Agent {
            id: AgentId::new(),
            project_id: input.project_id,
            name: input.name,
            description: input.description,
            default_spell_id: None,
            pinned_release_id: None,
            enabled: true,
            autostart: false,
            settings: input.settings,
            created_at: now,
            updated_at: now,
        };
        tables.agents.insert(agent.id, agent.clone());
        Ok(agent)
    }

    async fn update_agent(&self, id: AgentId, input: UpdateAgent) -> StoreResult<Agent> {
        let mut tables = self.tables.write().await;
        let mut agent = tables.agent(id)?.clone();
        if let Some(name) = &input.name {
            require_name(name, "agent")?;
        }
        if let Some(Some(spell)) = input.default_spell_id
            && tables.spell(spell)?.project_id != agent.project_id
        {
            return Err(StoreError::Invalid("default spell belongs to another project".to_owned()));
        }
        if let Some(Some(release)) = input.pinned_release_id {
            let pinned = tables.releases.get(&release).ok_or_else(|| StoreError::NotFound(format!("release {}", release.0)))?;
            if tables.spell(pinned.spell_id)?.project_id != agent.project_id {
                return Err(StoreError::Invalid("pinned release belongs to another project".to_owned()));
            }
        }
        if let Some(name) = input.name {
            agent.name = name;
        }
        if let Some(description) = input.description {
            agent.description = description;
        }
        if let Some(spell) = input.default_spell_id {
            agent.default_spell_id = spell;
        }
        if let Some(release) = input.pinned_release_id {
            agent.pinned_release_id = release;
        }
        if let Some(enabled) = input.enabled {
            agent.enabled = enabled;
        }
        if let Some(autostart) = input.autostart {
            agent.autostart = autostart;
        }
        if let Some(settings) = input.settings {
            agent.settings = settings;
        }
        agent.updated_at = Utc::now();
        tables.agents.insert(id, agent.clone());
        Ok(agent)
    }

    async fn get_agent(&self, id: AgentId) -> StoreResult<Agent> {
        self.tables.read().await.agent(id).cloned()
    }

    async fn list_agents(&self, project: ProjectId) -> StoreResult<Vec<Agent>> {
        let tables = self.tables.read().await;
        tables.project(project)?;
        Ok(tables.agents.values().filter(|agent| agent.project_id == project).cloned().collect())
    }

    async fn delete_agent(&self, id: AgentId) -> StoreResult<()> {
        let mut tables = self.tables.write().await;
        tables.agents.remove(&id).ok_or_else(|| StoreError::NotFound(format!("agent {}", id.0)))?;
        tables.webhooks.retain(|_, webhook| webhook.agent_id != id);
        let webhooks: Vec<WebhookId> = tables.webhooks.keys().copied().collect();
        tables.webhook_deliveries.retain(|(webhook, _), _| webhooks.contains(webhook));
        tables.memories.retain(|memory| memory.agent_id != id);
        tables.knowledge_chunks.retain(|(agent, _)| *agent != id);
        tables.knowledge_docs.retain(|_, doc| doc.agent_id != id);
        Ok(())
    }

    async fn create_spell(&self, input: NewSpell) -> StoreResult<SpellRecord> {
        require_name(&input.name, "spell")?;
        let mut tables = self.tables.write().await;
        tables.project(input.project_id)?;
        let now = Utc::now();
        let record = SpellRecord {
            id: SpellRecordId::new(),
            project_id: input.project_id,
            name: input.name,
            content_hash: graph_hash(&input.graph)?,
            graph: input.graph,
            created_at: now,
            updated_at: now,
        };
        tables.spells.insert(record.id, record.clone());
        Ok(record)
    }

    async fn save_spell_graph(&self, id: SpellRecordId, graph: SpellGraph) -> StoreResult<SpellRecord> {
        let hash = graph_hash(&graph)?;
        let mut tables = self.tables.write().await;
        let record = tables.spells.get_mut(&id).ok_or_else(|| StoreError::NotFound(format!("spell {}", id.0)))?;
        record.graph = graph;
        record.content_hash = hash;
        record.updated_at = Utc::now();
        Ok(record.clone())
    }

    async fn get_spell(&self, id: SpellRecordId) -> StoreResult<SpellRecord> {
        self.tables.read().await.spell(id).cloned()
    }

    async fn list_spells(&self, project: ProjectId) -> StoreResult<Vec<SpellRecord>> {
        let tables = self.tables.read().await;
        tables.project(project)?;
        Ok(tables.spells.values().filter(|spell| spell.project_id == project).cloned().collect())
    }

    async fn create_release(&self, input: NewSpellRelease) -> StoreResult<SpellRelease> {
        self.create_release_with_graph(input, None).await
    }

    async fn create_release_with_graph(&self, input: NewSpellRelease, graph: Option<SpellGraph>) -> StoreResult<SpellRelease> {
        let mut tables = self.tables.write().await;
        let spell = tables.spell(input.spell_id)?.clone();
        let (graph, content_hash) = match graph {
            Some(snapshot) if snapshot.id != spell.graph.id => return Err(StoreError::Invalid("release graph belongs to a different spell".to_owned())),
            Some(snapshot) => {
                let hash = graph_hash(&snapshot)?;
                (snapshot, hash)
            }
            None => (spell.graph, spell.content_hash),
        };
        let previous = tables.releases.values().filter(|release| release.spell_id == input.spell_id).map(|release| release.version).max().unwrap_or(0);
        let release =
            SpellRelease { id: ReleaseId::new(), spell_id: spell.id, version: previous + 1, notes: input.notes, graph, content_hash, created_at: Utc::now() };
        tables.releases.insert(release.id, release.clone());
        Ok(release)
    }

    async fn list_releases(&self, spell: SpellRecordId) -> StoreResult<Vec<SpellRelease>> {
        let tables = self.tables.read().await;
        tables.spell(spell)?;
        Ok(tables.releases.values().filter(|release| release.spell_id == spell).cloned().collect())
    }

    async fn put_credential(&self, input: NewCredential, vault: &dyn CryptoVault) -> StoreResult<CredentialMeta> {
        require_name(&input.name, "credential")?;
        if input.secret.is_empty() {
            return Err(StoreError::Invalid("credential secret must not be empty".to_owned()));
        }
        let sealed = vault.encrypt(input.secret.as_bytes())?;
        let mut tables = self.tables.write().await;
        tables.project(input.project_id)?;
        if tables.credentials.values().any(|(meta, _)| meta.project_id == input.project_id && meta.name == input.name) {
            return Err(StoreError::Conflict(format!("credential {} already exists in this project", input.name)));
        }
        let meta = CredentialMeta {
            id: CredentialId::new(),
            project_id: input.project_id,
            name: input.name,
            provider: input.provider,
            hint: secret_hint(&input.secret),
            created_at: Utc::now(),
            rotated_at: None,
        };
        tables.credentials.insert(meta.id, (meta.clone(), sealed));
        Ok(meta)
    }

    async fn reveal_credential(&self, id: CredentialId, vault: &dyn CryptoVault) -> StoreResult<String> {
        let tables = self.tables.read().await;
        let (_, sealed) = tables.credentials.get(&id).ok_or_else(|| StoreError::NotFound(format!("credential {}", id.0)))?;
        String::from_utf8(vault.decrypt(sealed)?).map_err(|_| StoreError::Crypto)
    }

    async fn list_credentials(&self, project: ProjectId) -> StoreResult<Vec<CredentialMeta>> {
        let tables = self.tables.read().await;
        tables.project(project)?;
        Ok(tables.credentials.values().filter(|(meta, _)| meta.project_id == project).map(|(meta, _)| meta.clone()).collect())
    }

    async fn delete_credential(&self, id: CredentialId) -> StoreResult<()> {
        let mut tables = self.tables.write().await;
        let signing: Vec<String> =
            tables.webhooks.values().filter(|webhook| webhook.secret_credential_id == Some(id)).map(|webhook| webhook.id.0.to_string()).collect();
        if !signing.is_empty() {
            return Err(StoreError::Conflict(format!("credential {} signs webhook {}; delete those webhooks first", id.0, signing.join(", "))));
        }
        tables.credentials.remove(&id).map(|_| ()).ok_or_else(|| StoreError::NotFound(format!("credential {}", id.0)))
    }

    async fn delete_webhook(&self, id: WebhookId) -> StoreResult<()> {
        let mut tables = self.tables.write().await;
        tables.webhook_deliveries.retain(|(webhook, _), _| *webhook != id);
        tables.webhooks.remove(&id).map(|_| ()).ok_or_else(|| StoreError::NotFound(format!("webhook {}", id.0)))
    }

    async fn clear_memories(&self, agent: AgentId) -> StoreResult<u64> {
        let mut tables = self.tables.write().await;
        let before = tables.memories.len();
        tables.memories.retain(|memory| memory.agent_id != agent);
        Ok(u64::try_from(before - tables.memories.len()).unwrap_or(u64::MAX))
    }

    async fn append_chat(&self, input: NewChatMessage) -> StoreResult<ChatMessage> {
        let mut tables = self.tables.write().await;
        tables.agent(input.agent_id)?;
        let message = ChatMessage {
            id: ChatMessageId::new(),
            agent_id: input.agent_id,
            channel: input.channel,
            role: input.role,
            sender: input.sender,
            content: input.content,
            metadata: input.metadata,
            created_at: Utc::now(),
        };
        tables.chats.push(message.clone());
        Ok(message)
    }

    async fn chat_history(&self, agent: AgentId, channel: &str, limit: u32) -> StoreResult<Vec<ChatMessage>> {
        let tables = self.tables.read().await;
        let matching: Vec<_> = tables.chats.iter().filter(|message| message.agent_id == agent && message.channel == channel).cloned().collect();
        let skip = matching.len().saturating_sub(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(matching.into_iter().skip(skip).collect())
    }

    async fn add_memory(&self, input: NewMemory) -> StoreResult<MemoryRecord> {
        let mut tables = self.tables.write().await;
        tables.agent(input.agent_id)?;
        let record = MemoryRecord {
            id: MemoryId::new(),
            agent_id: input.agent_id,
            content: input.content,
            embedding: input.embedding,
            metadata: input.metadata,
            created_at: Utc::now(),
        };
        tables.memories.push(record.clone());
        Ok(record)
    }

    async fn search_memories(&self, agent: AgentId, embedding: &[f32], limit: u32) -> StoreResult<Vec<SearchHit<MemoryRecord>>> {
        let tables = self.tables.read().await;
        tables.agent(agent)?;
        let hits = tables
            .memories
            .iter()
            .filter(|memory| memory.agent_id == agent)
            .map(|memory| SearchHit { score: memory.embedding.as_deref().map_or(0.0, |stored| cosine(stored, embedding)), item: memory.clone() })
            .filter(|hit| hit.score > 0.0)
            .collect();
        Ok(ranked(hits, limit))
    }

    async fn add_knowledge_doc(&self, input: NewKnowledgeDoc, chunks: Vec<KnowledgeChunk>) -> StoreResult<KnowledgeDoc> {
        let mut tables = self.tables.write().await;
        tables.agent(input.agent_id)?;
        let doc = KnowledgeDoc {
            id: KnowledgeDocId::new(),
            agent_id: input.agent_id,
            title: input.title,
            source_uri: input.source_uri,
            mime: input.mime,
            content_hash: input.content_hash,
            chunk_count: u32::try_from(chunks.len()).unwrap_or(u32::MAX),
            created_at: Utc::now(),
        };
        let agent = doc.agent_id;
        tables.knowledge_chunks.extend(chunks.into_iter().map(|chunk| (agent, KnowledgeChunk { doc_id: doc.id, ..chunk })));
        tables.knowledge_docs.insert(doc.id, doc.clone());
        Ok(doc)
    }

    async fn search_knowledge(&self, agent: AgentId, query: &str, embedding: &[f32], limit: u32) -> StoreResult<Vec<SearchHit<KnowledgeChunk>>> {
        let tables = self.tables.read().await;
        tables.agent(agent)?;
        let needle = query.to_lowercase();
        let hits = tables
            .knowledge_chunks
            .iter()
            .filter(|(owner, _)| *owner == agent)
            .map(|(_, chunk)| {
                let semantic = chunk.embedding.as_deref().map_or(0.0, |stored| cosine(stored, embedding));
                let lexical = if !needle.is_empty() && chunk.text.to_lowercase().contains(&needle) { 0.5 } else { 0.0 };
                SearchHit { score: semantic + lexical, item: chunk.clone() }
            })
            .filter(|hit| hit.score > 0.0)
            .collect();
        Ok(ranked(hits, limit))
    }

    async fn create_webhook(&self, input: NewWebhook) -> StoreResult<WebhookRecord> {
        let path = normalize_webhook_path(&input.path)?;
        let mut tables = self.tables.write().await;
        tables.agent(input.agent_id)?;
        if tables.webhooks.values().any(|webhook| webhook.path == path) {
            return Err(StoreError::Conflict(format!("webhook path {path} is already taken")));
        }
        let record = WebhookRecord {
            id: WebhookId::new(),
            agent_id: input.agent_id,
            path,
            secret_credential_id: input.secret_credential_id,
            enabled: true,
            created_at: Utc::now(),
        };
        tables.webhooks.insert(record.id, record.clone());
        Ok(record)
    }

    async fn find_webhook(&self, path: &str) -> StoreResult<WebhookRecord> {
        let path = normalize_webhook_path(path)?;
        let tables = self.tables.read().await;
        tables.webhooks.values().find(|webhook| webhook.path == path).cloned().ok_or_else(|| StoreError::NotFound(format!("webhook {path}")))
    }

    async fn list_webhooks(&self, agent: Option<AgentId>) -> StoreResult<Vec<WebhookRecord>> {
        let tables = self.tables.read().await;
        let mut webhooks: Vec<WebhookRecord> =
            tables.webhooks.values().filter(|webhook| agent.is_none_or(|agent| webhook.agent_id == agent)).cloned().collect();
        webhooks.sort_by(|left, right| left.created_at.cmp(&right.created_at).then_with(|| left.path.cmp(&right.path)));
        Ok(webhooks)
    }

    async fn record_webhook_delivery(&self, webhook: WebhookId, delivery_key: &str, received_at: i64, prune_before: i64) -> StoreResult<bool> {
        require_name(delivery_key, "webhook delivery")?;
        let mut tables = self.tables.write().await;
        if !tables.webhooks.contains_key(&webhook) {
            return Err(StoreError::Invalid(format!("webhook {} delivery references a missing record", webhook.0)));
        }
        tables.webhook_deliveries.retain(|_, received| *received >= prune_before);
        let key = (webhook, delivery_key.to_owned());
        if tables.webhook_deliveries.contains_key(&key) {
            return Ok(false);
        }
        tables.webhook_deliveries.insert(key, received_at);
        Ok(true)
    }

    async fn forget_webhook_delivery(&self, webhook: WebhookId, delivery_key: &str) -> StoreResult<()> {
        self.tables.write().await.webhook_deliveries.remove(&(webhook, delivery_key.to_owned()));
        Ok(())
    }

    async fn record_run(&self, run: ExecutionRun) -> StoreResult<()> {
        self.tables.write().await.runs.push(run);
        Ok(())
    }

    async fn list_runs(&self, agent: AgentId, limit: u32) -> StoreResult<Vec<ExecutionRun>> {
        let tables = self.tables.read().await;
        tables.agent(agent)?;
        Ok(tables.runs.iter().rev().filter(|run| run.agent_id == Some(agent)).take(usize::try_from(limit).unwrap_or(usize::MAX)).cloned().collect())
    }

    async fn find_run_by_correlation(&self, agent: AgentId, correlation_id: &str) -> StoreResult<Option<ExecutionRun>> {
        let tables = self.tables.read().await;
        let matches =
            |run: &&ExecutionRun| run.agent_id == Some(agent) && run.trace.get("correlation_id").and_then(serde_json::Value::as_str) == Some(correlation_id);
        Ok(tables.runs.iter().rev().find(matches).cloned())
    }

    async fn log_llm_request(&self, log: LlmRequestLog) -> StoreResult<()> {
        self.tables.write().await.llm_logs.push(log);
        Ok(())
    }

    async fn get_plugin_state(&self, agent: AgentId, plugin: &str) -> StoreResult<Option<PluginStateRecord>> {
        Ok(self.tables.read().await.plugin_states.get(&(agent, plugin.to_owned())).cloned())
    }

    async fn put_plugin_state(&self, state: PluginStateRecord) -> StoreResult<()> {
        self.tables.write().await.plugin_states.insert((state.agent_id, state.plugin.clone()), state);
        Ok(())
    }
}
