CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS projects (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS spells (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    graph JSONB NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS spell_releases (
    id UUID PRIMARY KEY,
    spell_id UUID NOT NULL REFERENCES spells(id) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK (version > 0),
    notes TEXT NOT NULL DEFAULT '',
    graph JSONB NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (spell_id, version)
);

CREATE OR REPLACE FUNCTION reject_spell_release_update() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'spell releases are immutable';
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS spell_releases_are_immutable ON spell_releases;
CREATE TRIGGER spell_releases_are_immutable BEFORE UPDATE ON spell_releases
    FOR EACH ROW EXECUTE FUNCTION reject_spell_release_update();

CREATE TABLE IF NOT EXISTS agents (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    default_spell_id UUID REFERENCES spells(id) ON DELETE SET NULL,
    pinned_release_id UUID REFERENCES spell_releases(id) ON DELETE SET NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    settings JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS credentials (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    hint TEXT NOT NULL,
    ciphertext BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    rotated_at TIMESTAMPTZ,
    UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS chat_messages (
    seq BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL UNIQUE,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    channel TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    sender TEXT NOT NULL,
    content TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS chat_messages_by_channel ON chat_messages (agent_id, channel, seq);

CREATE TABLE IF NOT EXISTS memories (
    id UUID PRIMARY KEY,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    embedding vector,
    dimensions INTEGER,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS memories_by_agent ON memories (agent_id, created_at);

CREATE TABLE IF NOT EXISTS knowledge_docs (
    id UUID PRIMARY KEY,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    source_uri TEXT,
    mime TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    chunk_count INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_id, content_hash)
);

CREATE TABLE IF NOT EXISTS knowledge_chunks (
    id BIGSERIAL PRIMARY KEY,
    doc_id UUID NOT NULL REFERENCES knowledge_docs(id) ON DELETE CASCADE,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    text TEXT NOT NULL,
    embedding vector,
    dimensions INTEGER,
    text_search tsvector GENERATED ALWAYS AS (to_tsvector('english', text)) STORED,
    UNIQUE (doc_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS knowledge_chunks_by_agent ON knowledge_chunks (agent_id);
CREATE INDEX IF NOT EXISTS knowledge_chunks_fts ON knowledge_chunks USING GIN (text_search);

CREATE TABLE IF NOT EXISTS webhooks (
    id UUID PRIMARY KEY,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    path TEXT NOT NULL UNIQUE,
    secret_credential_id UUID REFERENCES credentials(id) ON DELETE RESTRICT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS execution_runs (
    run_id UUID PRIMARY KEY,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    spell_id UUID NOT NULL REFERENCES spells(id) ON DELETE CASCADE,
    release_id UUID REFERENCES spell_releases(id) ON DELETE SET NULL,
    status TEXT NOT NULL,
    trigger_kind TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    nodes_executed BIGINT NOT NULL DEFAULT 0,
    error TEXT,
    trace JSONB NOT NULL DEFAULT 'null'::jsonb
);

CREATE INDEX IF NOT EXISTS execution_runs_by_agent ON execution_runs (agent_id, started_at);

CREATE TABLE IF NOT EXISTS llm_requests (
    id UUID PRIMARY KEY,
    run_id UUID,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    latency_ms BIGINT NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS llm_requests_by_run ON llm_requests (run_id);

CREATE TABLE IF NOT EXISTS plugin_state (
    id UUID PRIMARY KEY,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    plugin TEXT NOT NULL,
    state JSONB NOT NULL DEFAULT 'null'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_id, plugin)
);