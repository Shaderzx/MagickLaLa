use std::collections::BTreeMap;
use std::error::Error;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use magicklala_embedder::{ChunkConfig, DocumentInput, DocumentKind, Embedder, HttpEmbedder, KnowledgePipeline, LocalHashEmbedder, extract_text};
use magicklala_graph::SpellGraph;
use magicklala_store::{
    AesGcmVault, AgentId, CredentialId, KnowledgeChunk, KnowledgeDocId, NewAgent, NewCredential, NewKnowledgeDoc, NewMemory, NewProject, NewSpell,
    NewSpellRelease, NewWebhook, ProjectId, Repository, SqliteStore, UpdateAgent,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type ProbeResult<T> = Result<T, Box<dyn Error>>;

#[tokio::main]
async fn main() -> ProbeResult<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("usage: store_probe <command> <json-spec>")?;
    let spec: Value = serde_json::from_str(&args.next().ok_or("missing json spec argument")?)?;
    let output = match command.as_str() {
        "seed" => seed(&spec).await?,
        "inspect" => inspect(&spec).await?,
        "credential" => store_credential(&spec).await?,
        "reveal" => reveal(&spec).await?,
        "knowledge" => knowledge(&spec).await?,
        "memories" => memories(&spec).await?,
        "extract" => extract(&spec)?,
        "webhooks" => webhooks(&spec).await?,
        "concurrent-update" => concurrent_update(&spec).await?,
        "pinned-release" => pinned_release(&spec).await?,
        other => return Err(format!("unknown command {other}").into()),
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn text<'a>(spec: &'a Value, key: &str) -> ProbeResult<&'a str> {
    spec.get(key).and_then(Value::as_str).ok_or_else(|| format!("spec is missing string field {key}").into())
}

fn list<'a>(spec: &'a Value, key: &str) -> ProbeResult<&'a Vec<Value>> {
    spec.get(key).and_then(Value::as_array).ok_or_else(|| format!("spec is missing list field {key}").into())
}

fn limit(spec: &Value) -> u32 {
    spec.get("limit").and_then(Value::as_u64).and_then(|value| u32::try_from(value).ok()).unwrap_or(5)
}

async fn open(spec: &Value) -> ProbeResult<SqliteStore> {
    let store = SqliteStore::connect(&format!("sqlite://{}", text(spec, "db")?)).await?;
    store.migrate().await?;
    Ok(store)
}

async fn project_with_agents(store: &SqliteStore, name: &str) -> ProbeResult<(ProjectId, BTreeMap<String, AgentId>)> {
    let project = store.create_project(NewProject { name: name.to_owned(), description: String::new() }).await?;
    let mut agents = BTreeMap::new();
    for owner in ["primary", "other"] {
        let agent = store.create_agent(NewAgent { project_id: project.id, name: owner.to_owned(), description: String::new(), settings: json!({}) }).await?;
        agents.insert(owner.to_owned(), agent.id);
    }
    Ok((project.id, agents))
}

fn agent_for(agents: &BTreeMap<String, AgentId>, owner: &str) -> ProbeResult<AgentId> {
    agents.get(owner).copied().ok_or_else(|| format!("unknown owner {owner}").into())
}

async fn seed(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let project = store.create_project(NewProject { name: text(spec, "project")?.to_owned(), description: "seeded by store_probe".to_owned() }).await?;
    let agent = store
        .create_agent(NewAgent {
            project_id: project.id,
            name: text(spec, "agent")?.to_owned(),
            description: String::new(),
            settings: json!({ "temperature": 0.2 }),
        })
        .await?;
    let mut graph = SpellGraph::new(text(spec, "spell")?);
    let spell = store.create_spell(NewSpell { project_id: project.id, name: text(spec, "spell")?.to_owned(), graph: graph.clone() }).await?;
    let mut releases = Vec::new();
    for (round, notes) in list(spec, "releaseNotes")?.iter().enumerate() {
        graph.description = format!("revision {round}");
        store.save_spell_graph(spell.id, graph.clone()).await?;
        let release = store.create_release(NewSpellRelease { spell_id: spell.id, notes: notes.as_str().unwrap_or_default().to_owned() }).await?;
        releases.push(release);
    }
    let pinned = releases.last().map(|release| release.id);
    store.update_agent(agent.id, UpdateAgent { default_spell_id: Some(Some(spell.id)), pinned_release_id: Some(pinned), ..UpdateAgent::default() }).await?;
    Ok(json!({
        "projectId": project.id,
        "agentId": agent.id,
        "spellId": spell.id,
        "releases": releases.iter().map(|release| json!({ "id": release.id, "version": release.version })).collect::<Vec<_>>(),
    }))
}

async fn inspect(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let mut projects = Vec::new();
    let mut agents = Vec::new();
    let mut spells = Vec::new();
    for project in store.list_projects().await? {
        projects.push(json!({ "id": project.id, "name": project.name }));
        for agent in store.list_agents(project.id).await? {
            agents.push(json!({ "id": agent.id, "name": agent.name, "projectId": agent.project_id, "pinnedReleaseId": agent.pinned_release_id }));
        }
        for spell in store.list_spells(project.id).await? {
            let releases = store.list_releases(spell.id).await?;
            spells.push(json!({
                "id": spell.id,
                "name": spell.name,
                "releases": releases.iter().map(|release| json!({ "id": release.id, "version": release.version, "notes": release.notes })).collect::<Vec<_>>(),
            }));
        }
    }
    Ok(json!({ "projects": projects, "agents": agents, "spells": spells, "journalMode": store.journal_mode().await?, "tables": store.table_names().await? }))
}

async fn store_credential(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let vault = AesGcmVault::from_env_or_key(None)?;
    let project = store.create_project(NewProject { name: "vault".to_owned(), description: String::new() }).await?;
    let input = NewCredential {
        project_id: project.id,
        name: text(spec, "name")?.to_owned(),
        provider: text(spec, "provider")?.to_owned(),
        secret: text(spec, "secret")?.to_owned(),
    };
    let meta = store.put_credential(input, &vault).await?;
    let ciphertext: Vec<u8> = sqlx::query_scalar("SELECT ciphertext FROM credentials WHERE id = ?").bind(meta.id.0.to_string()).fetch_one(store.pool()).await?;
    Ok(json!({ "id": meta.id, "meta": meta, "metaDebug": format!("{meta:?}"), "ciphertextHex": hex::encode(ciphertext) }))
}

async fn reveal(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let vault = AesGcmVault::from_env_or_key(None)?;
    let id = CredentialId(Uuid::parse_str(text(spec, "id")?)?);
    Ok(json!({ "secret": store.reveal_credential(id, &vault).await? }))
}

fn document_kind(raw: &str) -> ProbeResult<DocumentKind> {
    Ok(serde_json::from_value(Value::String(raw.to_owned()))?)
}

fn document_bytes(doc: &Value) -> ProbeResult<Vec<u8>> {
    match doc.get("base64").and_then(Value::as_str) {
        Some(encoded) => Ok(STANDARD.decode(encoded)?),
        None => Ok(text(doc, "text")?.as_bytes().to_vec()),
    }
}

async fn knowledge(spec: &Value) -> ProbeResult<Value> {
    let config = ChunkConfig { max_chars: 160, overlap_chars: 40 };
    match text(spec, "embedder")? {
        "local" => knowledge_with(spec, KnowledgePipeline::new(LocalHashEmbedder::new(256), config)?).await,
        "http" => {
            knowledge_with(spec, KnowledgePipeline::new(HttpEmbedder::new(text(spec, "embeddingsUrl")?, text(spec, "model")?, 0).with_batch_size(16), config)?)
                .await
        }
        other => Err(format!("unknown embedder {other}").into()),
    }
}

async fn knowledge_with<E: Embedder>(spec: &Value, pipeline: KnowledgePipeline<E>) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let (_, agents) = project_with_agents(&store, "knowledge").await?;
    let mut owners: BTreeMap<KnowledgeDocId, (String, String)> = BTreeMap::new();
    let mut chunk_texts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for doc in list(spec, "docs")? {
        let owner = text(doc, "owner")?;
        let title = text(doc, "title")?;
        let kind = document_kind(text(doc, "kind")?)?;
        let input = DocumentInput { title: title.to_owned(), kind, source_uri: None, bytes: document_bytes(doc)? };
        let ingested = pipeline.ingest(&input).await?;
        let chunks = ingested
            .iter()
            .map(|chunk| KnowledgeChunk {
                doc_id: KnowledgeDocId::new(),
                index: chunk.index,
                text: chunk.text.clone(),
                embedding: Some(chunk.embedding.clone()),
            })
            .collect();
        let stored = store
            .add_knowledge_doc(
                NewKnowledgeDoc {
                    agent_id: agent_for(&agents, owner)?,
                    title: title.to_owned(),
                    source_uri: None,
                    mime: text(doc, "kind")?.to_owned(),
                    content_hash: hex::encode(Sha256::digest(&input.bytes)),
                },
                chunks,
            )
            .await?;
        owners.insert(stored.id, (title.to_owned(), owner.to_owned()));
        chunk_texts.insert(title.to_owned(), ingested.into_iter().map(|chunk| chunk.text).collect());
    }
    let query = text(spec, "query")?;
    let query_vector = pipeline.embedder().embed(&[query.to_owned()]).await?.into_iter().next().ok_or("embedder returned no query vector")?;
    let hits = store.search_knowledge(agent_for(&agents, "primary")?, query, &query_vector, limit(spec)).await?;
    let hits: Vec<Value> = hits
        .iter()
        .map(|hit| {
            let (title, owner) = owners.get(&hit.item.doc_id).cloned().unwrap_or_default();
            json!({ "title": title, "owner": owner, "score": hit.score, "text": hit.item.text })
        })
        .collect();
    Ok(json!({ "hits": hits, "chunks": chunk_texts }))
}

async fn memories(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let (_, agents) = project_with_agents(&store, "memories").await?;
    let embedder = LocalHashEmbedder::new(256);
    let mut owners = BTreeMap::new();
    for memory in list(spec, "memories")? {
        let content = text(memory, "content")?;
        let owner = text(memory, "owner")?;
        let stored = store
            .add_memory(NewMemory {
                agent_id: agent_for(&agents, owner)?,
                content: content.to_owned(),
                embedding: Some(embedder.embed_one(content)),
                metadata: json!({}),
            })
            .await?;
        owners.insert(stored.id, owner.to_owned());
    }
    let query_vector = embedder.embed_one(text(spec, "query")?);
    let hits = store.search_memories(agent_for(&agents, "primary")?, &query_vector, limit(spec)).await?;
    let hits: Vec<Value> = hits.iter().map(|hit| json!({ "content": hit.item.content, "owner": owners.get(&hit.item.id), "score": hit.score })).collect();
    Ok(json!({ "hits": hits }))
}

fn extract(spec: &Value) -> ProbeResult<Value> {
    let kind = document_kind(text(spec, "kind")?)?;
    Ok(json!({ "text": extract_text(kind, &document_bytes(spec)?)? }))
}

fn strings<'a>(spec: &'a Value, key: &str) -> ProbeResult<Vec<&'a str>> {
    list(spec, key)?.iter().map(|value| value.as_str().ok_or_else(|| format!("{key} must hold strings").into())).collect()
}

fn count(spec: &Value, key: &str) -> ProbeResult<usize> {
    let raw = spec.get(key).and_then(Value::as_u64).ok_or_else(|| format!("spec is missing number field {key}"))?;
    Ok(usize::try_from(raw)?)
}

async fn webhooks(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let (_, agents) = project_with_agents(&store, "webhooks").await?;
    let agent_id = agent_for(&agents, "primary")?;
    let created = store.create_webhook(NewWebhook { agent_id, path: text(spec, "create")?.to_owned(), secret_credential_id: None }).await?;
    let mut found = Vec::new();
    for lookup in strings(spec, "lookups")? {
        found.push(json!({ "lookup": lookup, "id": store.find_webhook(lookup).await?.id }));
    }
    let mut refused = Vec::new();
    for path in strings(spec, "refused")? {
        let error = match store.create_webhook(NewWebhook { agent_id, path: path.to_owned(), secret_credential_id: None }).await {
            Ok(webhook) => return Err(format!("webhook path {path:?} was accepted as {}", webhook.path).into()),
            Err(error) => error.to_string(),
        };
        refused.push(json!({ "path": path, "error": error }));
    }
    Ok(json!({ "created": { "id": created.id, "path": created.path }, "found": found, "refused": refused }))
}

async fn concurrent_update(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let project = store.create_project(NewProject { name: "concurrent".to_owned(), description: String::new() }).await?;
    let prefix = text(spec, "renamePrefix")?;
    let mut agents = Vec::new();
    for round in 0..count(spec, "rounds")? {
        let agent = store
            .create_agent(NewAgent {
                project_id: project.id,
                name: format!("agent-{round}"),
                description: "original description".to_owned(),
                settings: json!({}),
            })
            .await?;
        let rename = UpdateAgent { name: Some(format!("{prefix}-{round}")), ..UpdateAgent::default() };
        let disable = UpdateAgent { enabled: Some(false), ..UpdateAgent::default() };
        let (renamed, disabled) = tokio::join!(store.update_agent(agent.id, rename), store.update_agent(agent.id, disable));
        renamed?;
        disabled?;
        let stored = store.get_agent(agent.id).await?;
        agents.push(json!({ "name": stored.name, "enabled": stored.enabled, "description": stored.description }));
    }
    Ok(json!({ "agents": agents }))
}

async fn pinned_release(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let project = store.create_project(NewProject { name: "releases".to_owned(), description: String::new() }).await?;
    let mut draft = SpellGraph::new("pinned");
    draft.description = text(spec, "draft")?.to_owned();
    let spell = store.create_spell(NewSpell { project_id: project.id, name: "pinned".to_owned(), graph: draft.clone() }).await?;
    let mut snapshot_hashes = Vec::new();
    for round in 0..count(spec, "rounds")? {
        let mut snapshot = draft.clone();
        snapshot.description = format!("{} {round}", text(spec, "snapshot")?);
        let mut edited = draft.clone();
        edited.description = format!("{} {round}", text(spec, "edit")?);
        snapshot_hashes.push(hex::encode(Sha256::digest(serde_json::to_string(&snapshot)?.as_bytes())));
        let release = NewSpellRelease { spell_id: spell.id, notes: format!("round {round}") };
        let (saved, released) = tokio::join!(store.save_spell_graph(spell.id, edited), store.create_release_with_graph(release, Some(snapshot)));
        saved?;
        released?;
    }
    let releases: Vec<Value> = store
        .list_releases(spell.id)
        .await?
        .iter()
        .zip(&snapshot_hashes)
        .map(|(release, snapshot_hash)| {
            json!({ "version": release.version, "description": release.graph.description, "contentHash": release.content_hash, "snapshotHash": snapshot_hash })
        })
        .collect();
    let foreign = store
        .create_release_with_graph(NewSpellRelease { spell_id: spell.id, notes: "foreign".to_owned() }, Some(SpellGraph::new("someone else")))
        .await
        .err()
        .map(|error| error.to_string());
    Ok(json!({ "releases": releases, "draftDescription": store.get_spell(spell.id).await?.graph.description, "foreignGraphError": foreign }))
}
