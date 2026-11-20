PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS spells (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    graph TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS spell_releases (
    id TEXT PRIMARY KEY NOT NULL,
    spell_id TEXT NOT NULL REFERENCES spells(id) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK (version > 0),
    notes TEXT NOT NULL DEFAULT '',
    graph TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (spell_id, version)
);

CREATE TRIGGER IF NOT EXISTS spell_releases_are_immutable
BEFORE UPDATE ON spell_releases
BEGIN
    SELECT RAISE(ABORT, 'spell releases are immutable');
END;

CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    default_spell_id TEXT REFERENCES spells(id) ON DELETE SET NULL,
    pinned_release_id TEXT REFERENCES spell_releases(id) ON DELETE SET NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    settings TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS credentials (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    hint TEXT NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at TEXT NOT NULL,
    rotated_at TEXT,
    UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS chat_messages (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    channel TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    sender TEXT NOT NULL,
    content TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS chat_messages_by_channel ON chat_messages (agent_id, channel, seq);

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    embedding BLOB,
    dimensions INTEGER,
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS memories_by_agent ON memories (agent_id, created_at);

CREATE TABLE IF NOT EXISTS knowledge_docs (
    id TEXT PRIMARY KEY NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    source_uri TEXT,
    mime TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    chunk_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    UNIQUE (agent_id, content_hash)
);

CREATE TABLE IF NOT EXISTS knowledge_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id TEXT NOT NULL REFERENCES knowledge_docs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    text TEXT NOT NULL,
    embedding BLOB,
    dimensions INTEGER,
    UNIQUE (doc_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS knowledge_chunks_by_agent ON knowledge_chunks (agent_id);

CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_chunks_fts USING fts5(
    text,
    content = 'knowledge_chunks',
    content_rowid = 'id',
    tokenize = 'porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS knowledge_chunks_fts_insert AFTER INSERT ON knowledge_chunks
BEGIN
    INSERT INTO knowledge_chunks_fts (rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS knowledge_chunks_fts_delete AFTER DELETE ON knowledge_chunks
BEGIN
    INSERT INTO knowledge_chunks_fts (knowledge_chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER IF NOT EXISTS knowledge_chunks_fts_update AFTER UPDATE ON knowledge_chunks
BEGIN
    INSERT INTO knowledge_chunks_fts (knowledge_chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO knowledge_chunks_fts (rowid, text) VALUES (new.id, new.text);
END;

CREATE TABLE IF NOT EXISTS webhooks (
    id TEXT PRIMARY KEY NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    path TEXT NOT NULL UNIQUE,
    secret_credential_id TEXT REFERENCES credentials(id) ON DELETE SET NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS execution_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL,
    spell_id TEXT NOT NULL REFERENCES spells(id) ON DELETE CASCADE,
    release_id TEXT REFERENCES spell_releases(id) ON DELETE SET NULL,
    status TEXT NOT NULL,
    trigger_kind TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    nodes_executed INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    trace TEXT NOT NULL DEFAULT 'null'
);

CREATE INDEX IF NOT EXISTS execution_runs_by_agent ON execution_runs (agent_id, started_at);

CREATE TABLE IF NOT EXISTS llm_requests (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT,
    agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    latency_ms INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS llm_requests_by_run ON llm_requests (run_id);

CREATE TABLE IF NOT EXISTS plugin_state (
    id TEXT PRIMARY KEY NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    plugin TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'null',
    updated_at TEXT NOT NULL,
    UNIQUE (agent_id, plugin)
);