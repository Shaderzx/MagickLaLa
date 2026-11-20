use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, SubsecRound, Utc};
use magicklala_graph::{RunStatus, SpellGraph};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};
use uuid::Uuid;

use crate::{
    Agent, AgentId, ChatMessage, ChatMessageId, ChatRole, CredentialId, CredentialMeta, CryptoVault, ExecutionRun, KnowledgeChunk, KnowledgeDoc,
    KnowledgeDocId, LlmRequestLog, MemoryId, MemoryRecord, NewAgent, NewChatMessage, NewCredential, NewKnowledgeDoc, NewMemory, NewProject, NewSpell,
    NewSpellRelease, NewWebhook, PluginStateId, PluginStateRecord, Project, ProjectId, ReleaseId, Repository, SearchHit, SpellRecord, SpellRecordId,
    SpellRelease, SqliteStore, StoreError, StoreResult, UpdateAgent, WebhookId, WebhookRecord, normalize_webhook_path,
};

const SQLITE_MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_protect_webhook_secrets.sql")),
    (3, include_str!("../migrations/0003_webhook_deliveries_and_autostart.sql")),
    (4, include_str!("../migrations/0004_run_reply_column.sql")),
];
const CREDENTIAL_SIGNS_WEBHOOK: &str = "credential is referenced by a webhook";
pub const POSTGRES_SCHEMA: &str = include_str!("../migrations_pg/0001_initial.sql");
pub const POSTGRES_MIGRATIONS: &[(i64, &str)] = &[
    (1, POSTGRES_SCHEMA),
    (2, include_str!("../migrations_pg/0002_webhook_deliveries_and_autostart.sql")),
    (3, include_str!("../migrations_pg/0003_run_reply_column.sql")),
];
const KEYWORD_WEIGHT: f32 = 0.5;
const VECTOR_WEIGHT: f32 = 0.5;
const KEYWORD_CANDIDATES_PER_RESULT: i64 = 10;
const MAX_KEYWORD_CANDIDATES: i64 = 200;
const VECTOR_CANDIDATE_LIMIT: i64 = 2000;

const AGENT_COLUMNS: &str = "id, project_id, name, description, default_spell_id, pinned_release_id, enabled, autostart, settings, created_at, updated_at";
const WEBHOOK_COLUMNS: &str = "id, agent_id, path, secret_credential_id, enabled, created_at";
const SPELL_COLUMNS: &str = "id, project_id, name, graph, content_hash, created_at, updated_at";
const RELEASE_COLUMNS: &str = "id, spell_id, version, notes, graph, content_hash, created_at";
const CREDENTIAL_COLUMNS: &str = "id, project_id, name, provider, hint, created_at, rotated_at";
const RUN_COLUMNS: &str = "run_id, agent_id, spell_id, release_id, status, trigger_kind, started_at, finished_at, nodes_executed, error, reply, trace";

impl SqliteStore {
    pub async fn migrate(&self) -> StoreResult<()> {
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY NOT NULL, applied_at TEXT NOT NULL)")
            .execute(&self.pool)
            .await?;
        for (version, script) in SQLITE_MIGRATIONS {
            let mut transaction = self.pool.begin().await?;
            let claimed = sqlx::query("INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?) ON CONFLICT (version) DO NOTHING")
                .bind(version)
                .bind(stamp(Utc::now()))
                .execute(&mut *transaction)
                .await?;
            if claimed.rows_affected() == 0 {
                transaction.rollback().await?;
                continue;
            }
            sqlx::raw_sql(script).execute(&mut *transaction).await?;
            transaction.commit().await?;
        }
        Ok(())
    }

    pub async fn create_release_with_graph(&self, input: NewSpellRelease, graph: Option<SpellGraph>) -> StoreResult<SpellRelease> {
        let snapshot = graph.as_ref().map(graph_fingerprint).transpose()?;
        let expected_graph_id = graph.as_ref().map(|graph| graph.id.0.to_string());
        let (snapshot_json, snapshot_hash) = snapshot.unzip();
        let spell_id = input.spell_id.0.to_string();
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(&format!(
            "INSERT INTO spell_releases ({RELEASE_COLUMNS}) \
             SELECT ?, s.id, COALESCE((SELECT MAX(version) FROM spell_releases WHERE spell_id = s.id), 0) + 1, ?, COALESCE(?, s.graph), COALESCE(?, s.content_hash), ? \
             FROM spells s WHERE s.id = ? AND (? IS NULL OR json_extract(s.graph, '$.id') = ?) \
             RETURNING {RELEASE_COLUMNS}"
        ))
        .bind(ReleaseId::new().0.to_string())
        .bind(&input.notes)
        .bind(snapshot_json)
        .bind(snapshot_hash)
        .bind(stamp(Utc::now()))
        .bind(&spell_id)
        .bind(&expected_graph_id)
        .bind(&expected_graph_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| classify(error, &format!("release of spell {spell_id}")))?;
        let Some(row) = row else {
            let exists: Option<String> = sqlx::query_scalar("SELECT id FROM spells WHERE id = ?").bind(&spell_id).fetch_optional(&mut *transaction).await?;
            transaction.rollback().await?;
            return Err(match exists {
                Some(_) => StoreError::Invalid("release graph belongs to a different spell".to_owned()),
                None => StoreError::NotFound(format!("spell {spell_id}")),
            });
        };
        let release = release_from(&row)?;
        transaction.commit().await?;
        Ok(release)
    }

    pub async fn is_on_disk(&self) -> StoreResult<bool> {
        let rows = sqlx::query("PRAGMA database_list").fetch_all(&self.pool).await?;
        Ok(rows.iter().any(|row| {
            row.try_get::<String, _>("name").ok().as_deref() == Some("main") && row.try_get::<String, _>("file").ok().is_some_and(|file| !file.is_empty())
        }))
    }

    pub async fn journal_mode(&self) -> StoreResult<String> {
        Ok(sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&self.pool).await?)
    }

    pub async fn table_names(&self) -> StoreResult<Vec<String>> {
        Ok(sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").fetch_all(&self.pool).await?)
    }

    pub async fn list_knowledge_docs(&self, agent: AgentId) -> StoreResult<Vec<KnowledgeDoc>> {
        let rows = sqlx::query("SELECT id, agent_id, title, source_uri, mime, content_hash, chunk_count, created_at FROM knowledge_docs WHERE agent_id = ? ORDER BY created_at, title")
            .bind(agent.0.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(doc_from).collect()
    }

    pub async fn delete_knowledge_doc(&self, id: KnowledgeDocId) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM knowledge_docs WHERE id = ?").bind(id.0.to_string()).execute(&self.pool).await?;
        ensure_found(result.rows_affected(), format!("document {}", id.0))
    }

    async fn agent_project(&self, id: AgentId) -> StoreResult<ProjectId> {
        let project: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM agents WHERE id = ?").bind(id.0.to_string()).fetch_optional(&self.pool).await?;
        project.map(|raw| parse_uuid(&raw).map(ProjectId)).ok_or_else(|| StoreError::NotFound(format!("agent {}", id.0)))?
    }

    async fn project_of_release(&self, release: ReleaseId) -> StoreResult<ProjectId> {
        let project: Option<String> = sqlx::query_scalar("SELECT s.project_id FROM spell_releases r JOIN spells s ON s.id = r.spell_id WHERE r.id = ?")
            .bind(release.0.to_string())
            .fetch_optional(&self.pool)
            .await?;
        project.map(|raw| parse_uuid(&raw).map(ProjectId)).ok_or_else(|| StoreError::NotFound(format!("release {}", release.0)))?
    }
}

fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn parse_uuid(raw: &str) -> StoreResult<Uuid> {
    Uuid::parse_str(raw).map_err(|error| StoreError::Invalid(format!("bad uuid {raw}: {error}")))
}

fn uuid_column(row: &SqliteRow, column: &str) -> StoreResult<Uuid> {
    let raw: String = row.try_get(column)?;
    parse_uuid(&raw)
}

fn optional_uuid_column(row: &SqliteRow, column: &str) -> StoreResult<Option<Uuid>> {
    let raw: Option<String> = row.try_get(column)?;
    raw.as_deref().map(parse_uuid).transpose()
}

fn time_column(row: &SqliteRow, column: &str) -> StoreResult<DateTime<Utc>> {
    let raw: String = row.try_get(column)?;
    parse_time(&raw)
}

fn optional_time_column(row: &SqliteRow, column: &str) -> StoreResult<Option<DateTime<Utc>>> {
    let raw: Option<String> = row.try_get(column)?;
    raw.as_deref().map(parse_time).transpose()
}

fn parse_time(raw: &str) -> StoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw).map(|time| time.with_timezone(&Utc)).map_err(|error| StoreError::Invalid(format!("bad timestamp {raw}: {error}")))
}

fn json_column<T: serde::de::DeserializeOwned>(row: &SqliteRow, column: &str) -> StoreResult<T> {
    let raw: String = row.try_get(column)?;
    Ok(serde_json::from_str(&raw)?)
}

fn u32_column(row: &SqliteRow, column: &str) -> StoreResult<u32> {
    let raw: i64 = row.try_get(column)?;
    u32::try_from(raw).map_err(|_| StoreError::Invalid(format!("{column} out of range")))
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks::<4>().0.iter().copied().map(f32::from_le_bytes).collect()
}

fn vector_dimensions(vector: &[f32]) -> i64 {
    i64::try_from(vector.len()).unwrap_or(i64::MAX)
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let (dot, left_norm, right_norm) = left.iter().zip(right).fold((0.0f64, 0.0f64, 0.0f64), |(dot, ln, rn), (l, r)| {
        let (l, r) = (f64::from(*l), f64::from(*r));
        (dot + l * r, ln + l * l, rn + r * r)
    });
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    (dot / (left_norm.sqrt() * right_norm.sqrt())) as f32
}

fn graph_fingerprint(graph: &SpellGraph) -> StoreResult<(String, String)> {
    let json = serde_json::to_string(graph)?;
    let hash = hex::encode(Sha256::digest(json.as_bytes()));
    Ok((json, hash))
}

fn classify(error: sqlx::Error, subject: &str) -> StoreError {
    if let sqlx::Error::Database(database) = &error {
        if database.is_unique_violation() {
            return StoreError::Conflict(format!("{subject} already exists"));
        }
        if database.is_foreign_key_violation() {
            return StoreError::Invalid(format!("{subject} references a missing record"));
        }
    }
    StoreError::Database(error)
}

fn protected_credential(error: sqlx::Error) -> StoreError {
    match &error {
        sqlx::Error::Database(database) if database.message().contains(CREDENTIAL_SIGNS_WEBHOOK) => StoreError::Conflict(CREDENTIAL_SIGNS_WEBHOOK.to_owned()),
        _ => StoreError::Database(error),
    }
}

pub(crate) fn signing_conflict(credential: CredentialId, webhooks: &[String]) -> StoreError {
    StoreError::Conflict(format!("credential {} signs webhook {}; delete those webhooks first", credential.0, webhooks.join(", ")))
}

fn require_text(value: &str, field: &str) -> StoreResult<()> {
    if value.trim().is_empty() {
        return Err(StoreError::Invalid(format!("{field} must not be empty")));
    }
    Ok(())
}

fn ensure_found(rows_affected: u64, subject: String) -> StoreResult<()> {
    if rows_affected == 0 {
        return Err(StoreError::NotFound(subject));
    }
    Ok(())
}

fn secret_hint(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() < 8 {
        return "…".to_owned();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("…{tail}")
}

fn role_name(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
    }
}

fn parse_role(raw: &str) -> StoreResult<ChatRole> {
    match raw {
        "system" => Ok(ChatRole::System),
        "user" => Ok(ChatRole::User),
        "assistant" => Ok(ChatRole::Assistant),
        "tool" => Ok(ChatRole::Tool),
        other => Err(StoreError::Invalid(format!("unknown chat role {other}"))),
    }
}

fn status_name(status: RunStatus) -> StoreResult<String> {
    match serde_json::to_value(status)? {
        serde_json::Value::String(name) => Ok(name),
        other => Err(StoreError::Invalid(format!("unexpected run status encoding {other}"))),
    }
}

fn parse_status(raw: String) -> StoreResult<RunStatus> {
    Ok(serde_json::from_value(serde_json::Value::String(raw))?)
}

fn fts_query(query: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for term in query.split(|c: char| !c.is_alphanumeric()).filter(|term| term.chars().count() > 1) {
        let term = term.to_lowercase();
        if !terms.contains(&term) {
            terms.push(term);
        }
    }
    if terms.is_empty() {
        return None;
    }
    Some(terms.iter().map(|term| format!("\"{term}\"")).collect::<Vec<_>>().join(" OR "))
}

fn sql_limit(limit: u32) -> i64 {
    i64::from(limit.max(1))
}

fn keyword_candidate_limit(limit: u32) -> i64 {
    sql_limit(limit).saturating_mul(KEYWORD_CANDIDATES_PER_RESULT).min(MAX_KEYWORD_CANDIDATES)
}

fn project_from(row: &SqliteRow) -> StoreResult<Project> {
    Ok(Project {
        id: ProjectId(uuid_column(row, "id")?),
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at: time_column(row, "created_at")?,
        updated_at: time_column(row, "updated_at")?,
    })
}

fn agent_from(row: &SqliteRow) -> StoreResult<Agent> {
    Ok(Agent {
        id: AgentId(uuid_column(row, "id")?),
        project_id: ProjectId(uuid_column(row, "project_id")?),
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        default_spell_id: optional_uuid_column(row, "default_spell_id")?.map(SpellRecordId),
        pinned_release_id: optional_uuid_column(row, "pinned_release_id")?.map(ReleaseId),
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        autostart: row.try_get::<i64, _>("autostart")? != 0,
        settings: json_column(row, "settings")?,
        created_at: time_column(row, "created_at")?,
        updated_at: time_column(row, "updated_at")?,
    })
}

fn spell_from(row: &SqliteRow) -> StoreResult<SpellRecord> {
    Ok(SpellRecord {
        id: SpellRecordId(uuid_column(row, "id")?),
        project_id: ProjectId(uuid_column(row, "project_id")?),
        name: row.try_get("name")?,
        graph: json_column(row, "graph")?,
        content_hash: row.try_get("content_hash")?,
        created_at: time_column(row, "created_at")?,
        updated_at: time_column(row, "updated_at")?,
    })
}

fn release_from(row: &SqliteRow) -> StoreResult<SpellRelease> {
    Ok(SpellRelease {
        id: ReleaseId(uuid_column(row, "id")?),
        spell_id: SpellRecordId(uuid_column(row, "spell_id")?),
        version: u32_column(row, "version")?,
        notes: row.try_get("notes")?,
        graph: json_column(row, "graph")?,
        content_hash: row.try_get("content_hash")?,
        created_at: time_column(row, "created_at")?,
    })
}

fn credential_from(row: &SqliteRow) -> StoreResult<CredentialMeta> {
    Ok(CredentialMeta {
        id: CredentialId(uuid_column(row, "id")?),
        project_id: ProjectId(uuid_column(row, "project_id")?),
        name: row.try_get("name")?,
        provider: row.try_get("provider")?,
        hint: row.try_get("hint")?,
        created_at: time_column(row, "created_at")?,
        rotated_at: optional_time_column(row, "rotated_at")?,
    })
}

fn chat_from(row: &SqliteRow) -> StoreResult<ChatMessage> {
    let role: String = row.try_get("role")?;
    Ok(ChatMessage {
        id: ChatMessageId(uuid_column(row, "id")?),
        agent_id: AgentId(uuid_column(row, "agent_id")?),
        channel: row.try_get("channel")?,
        role: parse_role(&role)?,
        sender: row.try_get("sender")?,
        content: row.try_get("content")?,
        metadata: json_column(row, "metadata")?,
        created_at: time_column(row, "created_at")?,
    })
}

fn memory_from(row: &SqliteRow) -> StoreResult<MemoryRecord> {
    let embedding: Option<Vec<u8>> = row.try_get("embedding")?;
    Ok(MemoryRecord {
        id: MemoryId(uuid_column(row, "id")?),
        agent_id: AgentId(uuid_column(row, "agent_id")?),
        content: row.try_get("content")?,
        embedding: embedding.as_deref().map(decode_vector),
        metadata: json_column(row, "metadata")?,
        created_at: time_column(row, "created_at")?,
    })
}

fn doc_from(row: &SqliteRow) -> StoreResult<KnowledgeDoc> {
    Ok(KnowledgeDoc {
        id: KnowledgeDocId(uuid_column(row, "id")?),
        agent_id: AgentId(uuid_column(row, "agent_id")?),
        title: row.try_get("title")?,
        source_uri: row.try_get("source_uri")?,
        mime: row.try_get("mime")?,
        content_hash: row.try_get("content_hash")?,
        chunk_count: u32_column(row, "chunk_count")?,
        created_at: time_column(row, "created_at")?,
    })
}

fn webhook_from(row: &SqliteRow) -> StoreResult<WebhookRecord> {
    Ok(WebhookRecord {
        id: WebhookId(uuid_column(row, "id")?),
        agent_id: AgentId(uuid_column(row, "agent_id")?),
        path: row.try_get("path")?,
        secret_credential_id: optional_uuid_column(row, "secret_credential_id")?.map(CredentialId),
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        created_at: time_column(row, "created_at")?,
    })
}

fn run_from(row: &SqliteRow) -> StoreResult<ExecutionRun> {
    let nodes: i64 = row.try_get("nodes_executed")?;
    Ok(ExecutionRun {
        run_id: uuid_column(row, "run_id")?,
        agent_id: optional_uuid_column(row, "agent_id")?.map(AgentId),
        spell_id: SpellRecordId(uuid_column(row, "spell_id")?),
        release_id: optional_uuid_column(row, "release_id")?.map(ReleaseId),
        status: parse_status(row.try_get("status")?)?,
        trigger: row.try_get("trigger_kind")?,
        started_at: time_column(row, "started_at")?,
        finished_at: optional_time_column(row, "finished_at")?,
        nodes_executed: u64::try_from(nodes).unwrap_or(0),
        error: row.try_get("error")?,
        reply: row.try_get("reply")?,
        trace: json_column(row, "trace")?,
    })
}

fn plugin_state_from(row: &SqliteRow) -> StoreResult<PluginStateRecord> {
    Ok(PluginStateRecord {
        id: PluginStateId(uuid_column(row, "id")?),
        agent_id: AgentId(uuid_column(row, "agent_id")?),
        plugin: row.try_get("plugin")?,
        state: json_column(row, "state")?,
        updated_at: time_column(row, "updated_at")?,
    })
}

#[async_trait::async_trait]
impl Repository for SqliteStore {
    async fn create_project(&self, input: NewProject) -> StoreResult<Project> {
        require_text(&input.name, "project name")?;
        let now = Utc::now();
        let project = Project { id: ProjectId::new(), name: input.name, description: input.description, created_at: now, updated_at: now };
        sqlx::query("INSERT INTO projects (id, name, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?)")
            .bind(project.id.0.to_string())
            .bind(&project.name)
            .bind(&project.description)
            .bind(stamp(now))
            .bind(stamp(now))
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, &format!("project {}", project.name)))?;
        Ok(project)
    }

    async fn list_projects(&self) -> StoreResult<Vec<Project>> {
        let rows = sqlx::query("SELECT id, name, description, created_at, updated_at FROM projects ORDER BY created_at, name").fetch_all(&self.pool).await?;
        rows.iter().map(project_from).collect()
    }

    async fn create_agent(&self, input: NewAgent) -> StoreResult<Agent> {
        require_text(&input.name, "agent name")?;
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
        sqlx::query(&format!("INSERT INTO agents ({AGENT_COLUMNS}) VALUES (?, ?, ?, ?, NULL, NULL, 1, 0, ?, ?, ?)"))
            .bind(agent.id.0.to_string())
            .bind(agent.project_id.0.to_string())
            .bind(&agent.name)
            .bind(&agent.description)
            .bind(serde_json::to_string(&agent.settings)?)
            .bind(stamp(now))
            .bind(stamp(now))
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, &format!("agent {}", agent.name)))?;
        Ok(agent)
    }

    async fn update_agent(&self, id: AgentId, input: UpdateAgent) -> StoreResult<Agent> {
        if let Some(name) = &input.name {
            require_text(name, "agent name")?;
        }
        let project = self.agent_project(id).await?;
        if let Some(Some(spell)) = input.default_spell_id
            && self.get_spell(spell).await?.project_id != project
        {
            return Err(StoreError::Invalid("default spell belongs to another project".to_owned()));
        }
        if let Some(Some(release)) = input.pinned_release_id
            && self.project_of_release(release).await? != project
        {
            return Err(StoreError::Invalid("pinned release belongs to another project".to_owned()));
        }
        let subject = format!("agent {}", input.name.as_deref().unwrap_or(&id.0.to_string()));
        let mut update = QueryBuilder::<Sqlite>::new("UPDATE agents SET updated_at = ");
        update.push_bind(stamp(Utc::now()));
        if let Some(name) = input.name {
            update.push(", name = ").push_bind(name);
        }
        if let Some(description) = input.description {
            update.push(", description = ").push_bind(description);
        }
        if let Some(default_spell) = input.default_spell_id {
            update.push(", default_spell_id = ").push_bind(default_spell.map(|spell| spell.0.to_string()));
        }
        if let Some(pinned) = input.pinned_release_id {
            update.push(", pinned_release_id = ").push_bind(pinned.map(|release| release.0.to_string()));
        }
        if let Some(enabled) = input.enabled {
            update.push(", enabled = ").push_bind(i64::from(enabled));
        }
        if let Some(autostart) = input.autostart {
            update.push(", autostart = ").push_bind(i64::from(autostart));
        }
        if let Some(settings) = input.settings {
            update.push(", settings = ").push_bind(serde_json::to_string(&settings)?);
        }
        update.push(" WHERE id = ").push_bind(id.0.to_string());
        update.push(format!(" RETURNING {AGENT_COLUMNS}"));
        let row = update.build().fetch_optional(&self.pool).await.map_err(|error| classify(error, &subject))?;
        row.map(|row| agent_from(&row)).ok_or_else(|| StoreError::NotFound(format!("agent {}", id.0)))?
    }

    async fn get_agent(&self, id: AgentId) -> StoreResult<Agent> {
        let row = sqlx::query(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?")).bind(id.0.to_string()).fetch_optional(&self.pool).await?;
        row.map(|row| agent_from(&row)).ok_or_else(|| StoreError::NotFound(format!("agent {}", id.0)))?
    }

    async fn list_agents(&self, project: ProjectId) -> StoreResult<Vec<Agent>> {
        let rows = sqlx::query(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE project_id = ? ORDER BY created_at, name"))
            .bind(project.0.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(agent_from).collect()
    }

    async fn delete_agent(&self, id: AgentId) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM agents WHERE id = ?").bind(id.0.to_string()).execute(&self.pool).await?;
        ensure_found(result.rows_affected(), format!("agent {}", id.0))
    }

    async fn create_spell(&self, input: NewSpell) -> StoreResult<SpellRecord> {
        require_text(&input.name, "spell name")?;
        let (graph_json, content_hash) = graph_fingerprint(&input.graph)?;
        let now = Utc::now();
        let spell = SpellRecord {
            id: SpellRecordId::new(),
            project_id: input.project_id,
            name: input.name,
            graph: input.graph,
            content_hash,
            created_at: now,
            updated_at: now,
        };
        sqlx::query(&format!("INSERT INTO spells ({SPELL_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?)"))
            .bind(spell.id.0.to_string())
            .bind(spell.project_id.0.to_string())
            .bind(&spell.name)
            .bind(graph_json)
            .bind(&spell.content_hash)
            .bind(stamp(now))
            .bind(stamp(now))
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, &format!("spell {}", spell.name)))?;
        Ok(spell)
    }

    async fn save_spell_graph(&self, id: SpellRecordId, graph: SpellGraph) -> StoreResult<SpellRecord> {
        let (graph_json, content_hash) = graph_fingerprint(&graph)?;
        let result = sqlx::query("UPDATE spells SET graph = ?, content_hash = ?, updated_at = ? WHERE id = ?")
            .bind(graph_json)
            .bind(content_hash)
            .bind(stamp(Utc::now()))
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await?;
        ensure_found(result.rows_affected(), format!("spell {}", id.0))?;
        self.get_spell(id).await
    }

    async fn get_spell(&self, id: SpellRecordId) -> StoreResult<SpellRecord> {
        let row = sqlx::query(&format!("SELECT {SPELL_COLUMNS} FROM spells WHERE id = ?")).bind(id.0.to_string()).fetch_optional(&self.pool).await?;
        row.map(|row| spell_from(&row)).ok_or_else(|| StoreError::NotFound(format!("spell {}", id.0)))?
    }

    async fn list_spells(&self, project: ProjectId) -> StoreResult<Vec<SpellRecord>> {
        let rows = sqlx::query(&format!("SELECT {SPELL_COLUMNS} FROM spells WHERE project_id = ? ORDER BY created_at, name"))
            .bind(project.0.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(spell_from).collect()
    }

    async fn create_release(&self, input: NewSpellRelease) -> StoreResult<SpellRelease> {
        SqliteStore::create_release_with_graph(self, input, None).await
    }

    async fn create_release_with_graph(&self, input: NewSpellRelease, graph: Option<SpellGraph>) -> StoreResult<SpellRelease> {
        SqliteStore::create_release_with_graph(self, input, graph).await
    }

    async fn list_releases(&self, spell: SpellRecordId) -> StoreResult<Vec<SpellRelease>> {
        let rows = sqlx::query(&format!("SELECT {RELEASE_COLUMNS} FROM spell_releases WHERE spell_id = ? ORDER BY version"))
            .bind(spell.0.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(release_from).collect()
    }

    async fn put_credential(&self, input: NewCredential, vault: &dyn CryptoVault) -> StoreResult<CredentialMeta> {
        require_text(&input.name, "credential name")?;
        require_text(&input.secret, "credential secret")?;
        let ciphertext = vault.encrypt(input.secret.as_bytes())?;
        let row = sqlx::query(&format!(
            "INSERT INTO credentials (id, project_id, name, provider, hint, ciphertext, created_at, rotated_at) VALUES (?, ?, ?, ?, ?, ?, ?, NULL) \
             ON CONFLICT (project_id, name) DO UPDATE SET provider = excluded.provider, hint = excluded.hint, ciphertext = excluded.ciphertext, rotated_at = excluded.created_at \
             RETURNING {CREDENTIAL_COLUMNS}"
        ))
        .bind(CredentialId::new().0.to_string())
        .bind(input.project_id.0.to_string())
        .bind(&input.name)
        .bind(&input.provider)
        .bind(secret_hint(&input.secret))
        .bind(ciphertext)
        .bind(stamp(Utc::now()))
        .fetch_one(&self.pool)
        .await
        .map_err(|error| classify(error, &format!("credential {}", input.name)))?;
        credential_from(&row)
    }

    async fn reveal_credential(&self, id: CredentialId, vault: &dyn CryptoVault) -> StoreResult<String> {
        let sealed: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT ciphertext FROM credentials WHERE id = ?").bind(id.0.to_string()).fetch_optional(&self.pool).await?;
        let sealed = sealed.ok_or_else(|| StoreError::NotFound(format!("credential {}", id.0)))?;
        String::from_utf8(vault.decrypt(&sealed)?).map_err(|_| StoreError::Crypto)
    }

    async fn list_credentials(&self, project: ProjectId) -> StoreResult<Vec<CredentialMeta>> {
        let rows = sqlx::query(&format!("SELECT {CREDENTIAL_COLUMNS} FROM credentials WHERE project_id = ? ORDER BY name"))
            .bind(project.0.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(credential_from).collect()
    }

    async fn delete_credential(&self, id: CredentialId) -> StoreResult<()> {
        let signing: Vec<String> = sqlx::query_scalar("SELECT id FROM webhooks WHERE secret_credential_id = ? ORDER BY created_at, path")
            .bind(id.0.to_string())
            .fetch_all(&self.pool)
            .await?;
        if !signing.is_empty() {
            return Err(signing_conflict(id, &signing));
        }
        let result = sqlx::query("DELETE FROM credentials WHERE id = ?").bind(id.0.to_string()).execute(&self.pool).await.map_err(protected_credential)?;
        ensure_found(result.rows_affected(), format!("credential {}", id.0))
    }

    async fn delete_webhook(&self, id: WebhookId) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM webhooks WHERE id = ?").bind(id.0.to_string()).execute(&self.pool).await?;
        ensure_found(result.rows_affected(), format!("webhook {}", id.0))
    }

    async fn clear_memories(&self, agent: AgentId) -> StoreResult<u64> {
        let result = sqlx::query("DELETE FROM memories WHERE agent_id = ?").bind(agent.0.to_string()).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    async fn append_chat(&self, input: NewChatMessage) -> StoreResult<ChatMessage> {
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
        sqlx::query("INSERT INTO chat_messages (id, agent_id, channel, role, sender, content, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(message.id.0.to_string())
            .bind(message.agent_id.0.to_string())
            .bind(&message.channel)
            .bind(role_name(message.role))
            .bind(&message.sender)
            .bind(&message.content)
            .bind(serde_json::to_string(&message.metadata)?)
            .bind(stamp(message.created_at))
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, "chat message"))?;
        Ok(message)
    }

    async fn chat_history(&self, agent: AgentId, channel: &str, limit: u32) -> StoreResult<Vec<ChatMessage>> {
        let rows = sqlx::query(
            "SELECT * FROM (SELECT seq, id, agent_id, channel, role, sender, content, metadata, created_at FROM chat_messages WHERE agent_id = ? AND channel = ? ORDER BY seq DESC LIMIT ?) ORDER BY seq ASC",
        )
        .bind(agent.0.to_string())
        .bind(channel)
        .bind(sql_limit(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(chat_from).collect()
    }

    async fn add_memory(&self, input: NewMemory) -> StoreResult<MemoryRecord> {
        require_text(&input.content, "memory content")?;
        let memory = MemoryRecord {
            id: MemoryId::new(),
            agent_id: input.agent_id,
            content: input.content,
            embedding: input.embedding,
            metadata: input.metadata,
            created_at: Utc::now(),
        };
        sqlx::query("INSERT INTO memories (id, agent_id, content, embedding, dimensions, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(memory.id.0.to_string())
            .bind(memory.agent_id.0.to_string())
            .bind(&memory.content)
            .bind(memory.embedding.as_deref().map(encode_vector))
            .bind(memory.embedding.as_deref().map(vector_dimensions))
            .bind(serde_json::to_string(&memory.metadata)?)
            .bind(stamp(memory.created_at))
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, "memory"))?;
        Ok(memory)
    }

    async fn search_memories(&self, agent: AgentId, embedding: &[f32], limit: u32) -> StoreResult<Vec<SearchHit<MemoryRecord>>> {
        if embedding.is_empty() {
            return Err(StoreError::Invalid("query embedding must not be empty".to_owned()));
        }
        let rows = sqlx::query(
            "SELECT id, agent_id, content, embedding, metadata, created_at FROM memories WHERE agent_id = ? AND embedding IS NOT NULL AND dimensions = ?",
        )
        .bind(agent.0.to_string())
        .bind(vector_dimensions(embedding))
        .fetch_all(&self.pool)
        .await?;
        let mut hits = rows
            .iter()
            .map(|row| {
                let memory = memory_from(row)?;
                let score = memory.embedding.as_deref().map_or(0.0, |stored| cosine_similarity(stored, embedding));
                Ok(SearchHit { item: memory, score })
            })
            .collect::<StoreResult<Vec<_>>>()?;
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(usize::try_from(limit.max(1)).unwrap_or(usize::MAX));
        Ok(hits)
    }

    async fn add_knowledge_doc(&self, input: NewKnowledgeDoc, chunks: Vec<KnowledgeChunk>) -> StoreResult<KnowledgeDoc> {
        require_text(&input.title, "document title")?;
        let doc = KnowledgeDoc {
            id: KnowledgeDocId::new(),
            agent_id: input.agent_id,
            title: input.title,
            source_uri: input.source_uri,
            mime: input.mime,
            content_hash: input.content_hash,
            chunk_count: u32::try_from(chunks.len()).map_err(|_| StoreError::Invalid("too many chunks".to_owned()))?,
            created_at: Utc::now(),
        };
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO knowledge_docs (id, agent_id, title, source_uri, mime, content_hash, chunk_count, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(doc.id.0.to_string())
        .bind(doc.agent_id.0.to_string())
        .bind(&doc.title)
        .bind(&doc.source_uri)
        .bind(&doc.mime)
        .bind(&doc.content_hash)
        .bind(i64::from(doc.chunk_count))
        .bind(stamp(doc.created_at))
        .execute(&mut *transaction)
        .await
        .map_err(|error| classify(error, &format!("document {}", doc.title)))?;
        for chunk in &chunks {
            sqlx::query("INSERT INTO knowledge_chunks (doc_id, agent_id, chunk_index, text, embedding, dimensions) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(doc.id.0.to_string())
                .bind(doc.agent_id.0.to_string())
                .bind(i64::from(chunk.index))
                .bind(&chunk.text)
                .bind(chunk.embedding.as_deref().map(encode_vector))
                .bind(chunk.embedding.as_deref().map(vector_dimensions))
                .execute(&mut *transaction)
                .await
                .map_err(|error| classify(error, &format!("chunk {} of {}", chunk.index, doc.title)))?;
        }
        transaction.commit().await?;
        Ok(doc)
    }

    async fn search_knowledge(&self, agent: AgentId, query: &str, embedding: &[f32], limit: u32) -> StoreResult<Vec<SearchHit<KnowledgeChunk>>> {
        let agent_key = agent.0.to_string();
        let mut keyword_strength: HashMap<i64, f32> = HashMap::new();
        if let Some(match_expression) = fts_query(query) {
            let ranked = sqlx::query(
                "SELECT c.id AS id, bm25(knowledge_chunks_fts) AS rank FROM knowledge_chunks_fts JOIN knowledge_chunks c ON c.id = knowledge_chunks_fts.rowid \
                 WHERE knowledge_chunks_fts MATCH ? AND c.agent_id = ? ORDER BY rank LIMIT ?",
            )
            .bind(match_expression)
            .bind(&agent_key)
            .bind(keyword_candidate_limit(limit))
            .fetch_all(&self.pool)
            .await?;
            for row in &ranked {
                let rank: f64 = row.try_get("rank")?;
                keyword_strength.insert(row.try_get("id")?, (-rank).max(0.0) as f32);
            }
        }
        let strongest = keyword_strength.values().copied().fold(0.0f32, f32::max);
        let use_vectors = !embedding.is_empty();
        if keyword_strength.is_empty() && !use_vectors {
            return Ok(Vec::new());
        }
        let mut candidates = QueryBuilder::<Sqlite>::new("SELECT id, doc_id, chunk_index, text, embedding FROM knowledge_chunks WHERE agent_id = ");
        candidates.push_bind(&agent_key);
        candidates.push(" AND (0");
        if use_vectors {
            candidates.push(" OR id IN (SELECT id FROM knowledge_chunks WHERE agent_id = ");
            candidates.push_bind(&agent_key);
            candidates.push(" ORDER BY id DESC LIMIT ");
            candidates.push_bind(VECTOR_CANDIDATE_LIMIT);
            candidates.push(")");
        }
        if !keyword_strength.is_empty() {
            candidates.push(" OR id IN (");
            let mut ids = candidates.separated(", ");
            for id in keyword_strength.keys() {
                ids.push_bind(*id);
            }
            candidates.push(")");
        }
        candidates.push(")");
        let rows = candidates.build().fetch_all(&self.pool).await?;
        let use_keywords = strongest > 0.0;
        let mut hits = Vec::new();
        for row in &rows {
            let row_id: i64 = row.try_get("id")?;
            let stored: Option<Vec<u8>> = row.try_get("embedding")?;
            let stored = stored.as_deref().map(decode_vector);
            let keyword = if use_keywords { keyword_strength.get(&row_id).copied().unwrap_or(0.0) / strongest } else { 0.0 };
            let vector = stored.as_deref().filter(|_| use_vectors).map_or(0.0, |stored| cosine_similarity(stored, embedding).max(0.0));
            let score = match (use_keywords, use_vectors) {
                (true, true) => KEYWORD_WEIGHT * keyword + VECTOR_WEIGHT * vector,
                (true, false) => keyword,
                (false, true) => vector,
                (false, false) => 0.0,
            };
            if score <= 0.0 {
                continue;
            }
            let chunk = KnowledgeChunk {
                doc_id: KnowledgeDocId(uuid_column(row, "doc_id")?),
                index: u32_column(row, "chunk_index")?,
                text: row.try_get("text")?,
                embedding: stored,
            };
            hits.push(SearchHit { item: chunk, score });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(usize::try_from(limit.max(1)).unwrap_or(usize::MAX));
        Ok(hits)
    }

    async fn create_webhook(&self, input: NewWebhook) -> StoreResult<WebhookRecord> {
        let webhook = WebhookRecord {
            id: WebhookId::new(),
            agent_id: input.agent_id,
            path: normalize_webhook_path(&input.path)?,
            secret_credential_id: input.secret_credential_id,
            enabled: true,
            created_at: Utc::now().trunc_subsecs(6),
        };
        sqlx::query(&format!("INSERT INTO webhooks ({WEBHOOK_COLUMNS}) VALUES (?, ?, ?, ?, 1, ?)"))
            .bind(webhook.id.0.to_string())
            .bind(webhook.agent_id.0.to_string())
            .bind(&webhook.path)
            .bind(webhook.secret_credential_id.map(|credential| credential.0.to_string()))
            .bind(stamp(webhook.created_at))
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, &format!("webhook {}", webhook.path)))?;
        Ok(webhook)
    }

    async fn find_webhook(&self, path: &str) -> StoreResult<WebhookRecord> {
        let path = normalize_webhook_path(path)?;
        let row = sqlx::query(&format!("SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE path = ?")).bind(&path).fetch_optional(&self.pool).await?;
        row.map(|row| webhook_from(&row)).ok_or_else(|| StoreError::NotFound(format!("webhook {path}")))?
    }

    async fn list_webhooks(&self, agent: Option<AgentId>) -> StoreResult<Vec<WebhookRecord>> {
        let rows = match agent {
            Some(agent) => {
                sqlx::query(&format!("SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE agent_id = ? ORDER BY created_at, path"))
                    .bind(agent.0.to_string())
                    .fetch_all(&self.pool)
                    .await?
            }
            None => sqlx::query(&format!("SELECT {WEBHOOK_COLUMNS} FROM webhooks ORDER BY created_at, path")).fetch_all(&self.pool).await?,
        };
        rows.iter().map(webhook_from).collect()
    }

    async fn record_webhook_delivery(&self, webhook: WebhookId, delivery_key: &str, received_at: i64, prune_before: i64) -> StoreResult<bool> {
        require_text(delivery_key, "webhook delivery id")?;
        sqlx::query("DELETE FROM webhook_deliveries WHERE received_at < ?").bind(prune_before).execute(&self.pool).await?;
        let inserted = sqlx::query("INSERT OR IGNORE INTO webhook_deliveries (webhook_id, delivery_key, received_at) VALUES (?, ?, ?)")
            .bind(webhook.0.to_string())
            .bind(delivery_key)
            .bind(received_at)
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, &format!("webhook {} delivery", webhook.0)))?;
        Ok(inserted.rows_affected() == 1)
    }

    async fn forget_webhook_delivery(&self, webhook: WebhookId, delivery_key: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM webhook_deliveries WHERE webhook_id = ? AND delivery_key = ?")
            .bind(webhook.0.to_string())
            .bind(delivery_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn record_run(&self, run: ExecutionRun) -> StoreResult<()> {
        sqlx::query(&format!(
            "INSERT INTO execution_runs ({RUN_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (run_id) DO UPDATE SET status = excluded.status, finished_at = excluded.finished_at, nodes_executed = excluded.nodes_executed, error = excluded.error, reply = excluded.reply, trace = excluded.trace"
        ))
        .bind(run.run_id.to_string())
        .bind(run.agent_id.map(|agent| agent.0.to_string()))
        .bind(run.spell_id.0.to_string())
        .bind(run.release_id.map(|release| release.0.to_string()))
        .bind(status_name(run.status)?)
        .bind(&run.trigger)
        .bind(stamp(run.started_at))
        .bind(run.finished_at.map(stamp))
        .bind(i64::try_from(run.nodes_executed).unwrap_or(i64::MAX))
        .bind(&run.error)
        .bind(&run.reply)
        .bind(serde_json::to_string(&run.trace)?)
        .execute(&self.pool)
        .await
        .map_err(|error| classify(error, &format!("run {}", run.run_id)))?;
        Ok(())
    }

    async fn list_runs(&self, agent: AgentId, limit: u32) -> StoreResult<Vec<ExecutionRun>> {
        let rows = sqlx::query(&format!("SELECT {RUN_COLUMNS} FROM execution_runs WHERE agent_id = ? ORDER BY started_at DESC LIMIT ?"))
            .bind(agent.0.to_string())
            .bind(sql_limit(limit))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(run_from).collect()
    }

    async fn find_run_by_correlation(&self, agent: AgentId, correlation_id: &str) -> StoreResult<Option<ExecutionRun>> {
        let row = sqlx::query(&format!(
            "SELECT {RUN_COLUMNS} FROM execution_runs WHERE agent_id = ? AND json_extract(trace, '$.correlation_id') = ? ORDER BY started_at DESC, rowid DESC LIMIT 1"
        ))
        .bind(agent.0.to_string())
        .bind(correlation_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(run_from).transpose()
    }

    async fn log_llm_request(&self, log: LlmRequestLog) -> StoreResult<()> {
        sqlx::query("INSERT INTO llm_requests (id, run_id, agent_id, provider, model, prompt_tokens, completion_tokens, latency_ms, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(log.id.to_string())
            .bind(log.run_id.map(|run| run.to_string()))
            .bind(log.agent_id.map(|agent| agent.0.to_string()))
            .bind(&log.provider)
            .bind(&log.model)
            .bind(i64::from(log.prompt_tokens))
            .bind(i64::from(log.completion_tokens))
            .bind(i64::try_from(log.latency_ms).unwrap_or(i64::MAX))
            .bind(&log.status)
            .bind(stamp(log.created_at))
            .execute(&self.pool)
            .await
            .map_err(|error| classify(error, &format!("llm request {}", log.id)))?;
        Ok(())
    }

    async fn get_plugin_state(&self, agent: AgentId, plugin: &str) -> StoreResult<Option<PluginStateRecord>> {
        let row = sqlx::query("SELECT id, agent_id, plugin, state, updated_at FROM plugin_state WHERE agent_id = ? AND plugin = ?")
            .bind(agent.0.to_string())
            .bind(plugin)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(plugin_state_from).transpose()
    }

    async fn put_plugin_state(&self, state: PluginStateRecord) -> StoreResult<()> {
        require_text(&state.plugin, "plugin name")?;
        sqlx::query(
            "INSERT INTO plugin_state (id, agent_id, plugin, state, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT (agent_id, plugin) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
        )
        .bind(state.id.0.to_string())
        .bind(state.agent_id.0.to_string())
        .bind(&state.plugin)
        .bind(serde_json::to_string(&state.state)?)
        .bind(stamp(state.updated_at))
        .execute(&self.pool)
        .await
        .map_err(|error| classify(error, &format!("plugin state {}", state.plugin)))?;
        Ok(())
    }
}
