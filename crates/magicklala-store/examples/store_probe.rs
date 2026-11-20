use std::collections::BTreeMap;
use std::error::Error;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use magicklala_embedder::{ChunkConfig, DocumentInput, DocumentKind, Embedder, HttpEmbedder, KnowledgePipeline, LocalHashEmbedder, extract_text};
use magicklala_graph::{RunStatus, SpellGraph};
use magicklala_store::{
    AesGcmVault, AgentId, CredentialId, CryptoVault, ExecutionRun, KnowledgeChunk, KnowledgeDocId, NewAgent, NewCredential, NewKnowledgeDoc, NewMemory,
    NewProject, NewSpell, NewSpellRelease, NewWebhook, ProjectId, Repository, SpellRecordId, SqliteStore, UpdateAgent, WebhookId, is_in_memory_database_url,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type ProbeResult<T> = Result<T, Box<dyn Error>>;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match probe().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn probe() -> ProbeResult<()> {
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
        "credential-guard" => credential_guard(&spec).await?,
        "credential-release" => credential_release(&spec).await?,
        "clear-memories" => clear_memories(&spec).await?,
        "run-correlation" => run_correlation(&spec).await?,
        "vault-keys" => vault_keys(&spec)?,
        "memory-urls" => memory_urls(&spec).await?,
        "connect-urls" => connect_urls(&spec).await?,
        "webhook-deliveries" => webhook_deliveries(&spec).await?,
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

async fn credential_guard(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let vault = AesGcmVault::from_env_or_key(None)?;
    let (project, agents) = project_with_agents(&store, "guarded").await?;
    let credential = store
        .put_credential(
            NewCredential { project_id: project, name: "webhook-signing".to_owned(), provider: "webhook".to_owned(), secret: text(spec, "secret")?.to_owned() },
            &vault,
        )
        .await?;
    let webhook = store
        .create_webhook(NewWebhook {
            agent_id: agent_for(&agents, "primary")?,
            path: text(spec, "path")?.to_owned(),
            secret_credential_id: Some(credential.id),
        })
        .await?;
    let blocked = store.delete_credential(credential.id).await.err().map(|error| error.to_string());
    let signed = store.find_webhook(text(spec, "path")?).await?.secret_credential_id;
    Ok(json!({ "webhookId": webhook.id, "credentialId": credential.id, "blockedError": blocked, "signedWebhookSecret": signed }))
}

fn whole(spec: &Value, key: &str) -> ProbeResult<i64> {
    spec.get(key).and_then(Value::as_i64).ok_or_else(|| format!("spec is missing integer field {key}").into())
}

async fn webhook_deliveries(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let now = whole(spec, "now")?;
    let window = whole(spec, "window")?;
    let concurrent = whole(spec, "concurrent")?;
    let (_, agents) = project_with_agents(&store, "deliveries").await?;
    let hook = |owner: &str, path: &str| NewWebhook { agent_id: agents[owner], path: path.to_owned(), secret_credential_id: None };
    let primary = store.create_webhook(hook("primary", "deliveries/primary")).await?;
    let secondary = store.create_webhook(hook("other", "deliveries/secondary")).await?;
    let mut racers = tokio::task::JoinSet::new();
    for _ in 0..concurrent {
        let racer = store.clone();
        racers.spawn(async move { racer.record_webhook_delivery(primary.id, "delivery-fresh", now, now - window).await });
    }
    let mut fresh = 0_u32;
    let mut duplicates = 0_u32;
    while let Some(joined) = racers.join_next().await {
        if joined?? {
            fresh += 1;
        } else {
            duplicates += 1;
        }
    }
    let replayed = store.record_webhook_delivery(primary.id, "delivery-fresh", now, now - window).await?;
    let same_key_other_webhook = store.record_webhook_delivery(secondary.id, "delivery-fresh", now, now - window).await?;
    store.record_webhook_delivery(primary.id, "delivery-stale", now - window - 1, 0).await?;
    let stale_before_prune = store.record_webhook_delivery(primary.id, "delivery-stale", now, 0).await?;
    let stale_after_prune = store.record_webhook_delivery(primary.id, "delivery-stale", now + 1000, now - window).await?;
    let ids = |webhooks: Vec<magicklala_store::WebhookRecord>| webhooks.into_iter().map(|webhook| webhook.id.0.to_string()).collect::<Vec<_>>();
    let listed_for_primary = ids(store.list_webhooks(Some(agents["primary"])).await?);
    let listed_for_secondary = ids(store.list_webhooks(Some(agents["other"])).await?);
    let listed_all = ids(store.list_webhooks(None).await?);
    store.delete_webhook(secondary.id).await?;
    Ok(json!({
        "primaryWebhookId": primary.id,
        "secondaryWebhookId": secondary.id,
        "concurrentFresh": fresh,
        "concurrentDuplicates": duplicates,
        "replayed": replayed,
        "sameKeyOtherWebhook": same_key_other_webhook,
        "staleBeforePrune": stale_before_prune,
        "staleAfterPrune": stale_after_prune,
        "listedForPrimary": listed_for_primary,
        "listedForSecondary": listed_for_secondary,
        "listedAll": listed_all,
    }))
}

async fn credential_release(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let webhook = WebhookId(Uuid::parse_str(text(spec, "webhookId")?)?);
    let credential = CredentialId(Uuid::parse_str(text(spec, "credentialId")?)?);
    store.delete_webhook(webhook).await?;
    store.delete_credential(credential).await?;
    let missing = store.delete_webhook(webhook).await.err().map(|error| error.to_string());
    Ok(json!({ "webhookDeleted": true, "credentialDeleted": true, "missingWebhookError": missing }))
}

async fn recall(store: &SqliteStore, agent: AgentId, query: &[f32]) -> ProbeResult<Vec<String>> {
    Ok(store.search_memories(agent, query, 10).await?.into_iter().map(|hit| hit.item.content).collect())
}

async fn clear_memories(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let (_, agents) = project_with_agents(&store, "forgetful").await?;
    let embedder = LocalHashEmbedder::new(256);
    for memory in list(spec, "memories")? {
        let content = text(memory, "content")?;
        store
            .add_memory(NewMemory {
                agent_id: agent_for(&agents, text(memory, "owner")?)?,
                content: content.to_owned(),
                embedding: Some(embedder.embed_one(content)),
                metadata: json!({}),
            })
            .await?;
    }
    let query = embedder.embed_one(text(spec, "query")?);
    let primary = agent_for(&agents, "primary")?;
    let recall_before = recall(&store, primary, &query).await?;
    let removed = store.clear_memories(primary).await?;
    let recall_after = recall(&store, primary, &query).await?;
    let removed_again = store.clear_memories(primary).await?;
    let other_after = recall(&store, agent_for(&agents, "other")?, &query).await?;
    Ok(
        json!({ "recallBefore": recall_before, "removed": removed, "recallAfter": recall_after, "removedAgain": removed_again, "otherRecallAfter": other_after }),
    )
}

fn correlated_run(agent: AgentId, spell: SpellRecordId, correlation: &str, age_seconds: i64) -> ExecutionRun {
    let started_at = chrono::Utc::now() - chrono::Duration::seconds(age_seconds);
    ExecutionRun {
        run_id: Uuid::now_v7(),
        agent_id: Some(agent),
        spell_id: spell,
        release_id: None,
        status: RunStatus::Succeeded,
        trigger: "event/on_message".to_owned(),
        started_at,
        finished_at: Some(started_at),
        nodes_executed: 1,
        error: None,
        reply: None,
        trace: json!({ "correlation_id": correlation, "channel": "probe" }),
    }
}

async fn found_run(store: &SqliteStore, agent: AgentId, correlation: &str) -> ProbeResult<Value> {
    Ok(match store.find_run_by_correlation(agent, correlation).await? {
        Some(run) => json!({ "runId": run.run_id, "correlation": run.trace["correlation_id"] }),
        None => Value::Null,
    })
}

async fn run_correlation(spec: &Value) -> ProbeResult<Value> {
    let store = open(spec).await?;
    let (project, agents) = project_with_agents(&store, "correlated").await?;
    let spell = store.create_spell(NewSpell { project_id: project, name: "correlated".to_owned(), graph: SpellGraph::new("correlated") }).await?;
    let (primary, other) = (agent_for(&agents, "primary")?, agent_for(&agents, "other")?);
    let (correlation, other_correlation) = (text(spec, "correlation")?, text(spec, "otherCorrelation")?);
    let runs = [
        ("primaryOlder", correlated_run(primary, spell.id, correlation, 120)),
        ("primaryNewer", correlated_run(primary, spell.id, correlation, 5)),
        ("primaryOther", correlated_run(primary, spell.id, other_correlation, 1)),
        ("otherAgent", correlated_run(other, spell.id, correlation, 0)),
    ];
    let mut ids = serde_json::Map::new();
    for (label, run) in runs {
        ids.insert(label.to_owned(), json!(run.run_id));
        store.record_run(run).await?;
    }
    Ok(json!({
        "runs": ids,
        "found": {
            "primary": found_run(&store, primary, correlation).await?,
            "primaryOther": found_run(&store, primary, other_correlation).await?,
            "otherAgent": found_run(&store, other, correlation).await?,
            "unknown": found_run(&store, primary, text(spec, "unknown")?).await?,
        },
    }))
}

fn sealed_round_trip(vault: &AesGcmVault, plaintext: &str) -> ProbeResult<String> {
    Ok(String::from_utf8(vault.decrypt(&vault.encrypt(plaintext.as_bytes())?)?)?)
}

fn vault_keys(spec: &Value) -> ProbeResult<Value> {
    let plaintext = text(spec, "plaintext")?;
    let mut keys = Vec::new();
    for key in list(spec, "keys")? {
        let entry = match AesGcmVault::from_key_material(text(key, "material")?) {
            Ok(vault) => json!({ "label": text(key, "label")?, "accepted": true, "error": null, "opened": sealed_round_trip(&vault, plaintext)? }),
            Err(error) => json!({ "label": text(key, "label")?, "accepted": false, "error": error.to_string(), "opened": null }),
        };
        keys.push(entry);
    }
    Ok(json!({ "keys": keys }))
}

async fn project_names(store: &SqliteStore) -> ProbeResult<Vec<String>> {
    let mut names: Vec<String> = store.list_projects().await?.into_iter().map(|project| project.name).collect();
    names.sort();
    Ok(names)
}

async fn memory_urls(spec: &Value) -> ProbeResult<Value> {
    let classified: Vec<Value> = strings(spec, "urls")?.into_iter().map(|url| json!({ "url": url, "inMemory": is_in_memory_database_url(url) })).collect();
    let shared = SqliteStore::connect("sqlite::memory:").await?;
    shared.migrate().await?;
    let errors = concurrent_writers(&shared, count(spec, "writers")?).await;
    let concurrent = project_names(&shared).await?;
    let on_disk = SqliteStore::connect(text(spec, "fileUrl")?).await?;
    on_disk.migrate().await?;
    on_disk.create_project(NewProject { name: "on-disk".to_owned(), description: String::new() }).await?;
    let file_projects = project_names(&on_disk).await?;
    on_disk.pool().close().await;
    Ok(json!({ "classified": classified, "concurrent": { "errors": errors, "projects": concurrent }, "file": { "projects": file_projects } }))
}

async fn concurrent_writers(shared: &SqliteStore, writers: usize) -> Vec<String> {
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..writers {
        let store = shared.clone();
        tasks.spawn(async move { store.create_project(NewProject { name: format!("writer-{index:02}"), description: String::new() }).await });
    }
    let mut errors = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => errors.push(error.to_string()),
            Err(error) => errors.push(error.to_string()),
        }
    }
    errors
}

async fn connect_urls(spec: &Value) -> ProbeResult<Value> {
    let writers = count(spec, "writers")?;
    let mut stores = Vec::new();
    for url in strings(spec, "urls")? {
        let store = SqliteStore::connect(url).await?;
        store.migrate().await?;
        let on_disk = store.is_on_disk().await?;
        let errors = concurrent_writers(&store, writers).await;
        let projects = project_names(&store).await?;
        store.pool().close().await;
        stores.push(json!({ "url": url, "inMemory": is_in_memory_database_url(url), "onDisk": on_disk, "errors": errors, "projects": projects }));
    }
    Ok(json!({ "stores": stores }))
}
