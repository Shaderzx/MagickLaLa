import { test, expect } from 'e2e';
import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { createServer, type IncomingMessage, type Server } from 'node:http';
import { mkdir, mkdtemp, readFile, readdir, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { DatabaseSync } from 'node:sqlite';
import { deflateSync } from 'node:zlib';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');

type ProbeOutcome = { ok: true; json: any } | { ok: false; stderr: string };

async function probe(command: string, spec: object, masterKey: string): Promise<ProbeOutcome> {
  const env = { ...process.env, MAGICKLALA_MASTER_KEY: masterKey };
  try {
    const { stdout } = await run('cargo', ['run', '-q', '-p', 'magicklala-store', '--example', 'store_probe', '--', command, JSON.stringify(spec)], { cwd: repoRoot, env, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
    return { ok: true, json: JSON.parse(stdout) };
  } catch (error: any) {
    return { ok: false, stderr: String(error.stderr ?? error.message) };
  }
}

async function probeJson(command: string, spec: object, masterKey: string): Promise<any> {
  const outcome = await probe(command, spec, masterKey);
  if (!outcome.ok) throw new Error(`store_probe ${command} failed: ${outcome.stderr}`);
  return outcome.json;
}

async function withDatabase<T>(body: (db: string, dir: string) => Promise<T>): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-store-'));
  try {
    return await body(join(dir, 'magicklala.db'), dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function queryDatabase<T>(db: string, sql: string, ...params: Array<string | number>): T[] {
  const database = new DatabaseSync(db, { readOnly: true });
  try {
    return database.prepare(sql).all(...params) as T[];
  } finally {
    database.close();
  }
}

async function rawDatabaseBytes(dir: string): Promise<Buffer> {
  const names = await readdir(dir);
  const parts = await Promise.all(names.map((name) => readFile(join(dir, name))));
  return Buffer.concat(parts);
}

const vocabulary = ['basalt', 'lava', 'eruption', 'volcano', 'magma', 'sourdough', 'starter', 'fermentation', 'bread', 'flour', 'tide', 'moon', 'ocean'];

function loopbackEmbedding(text: string): number[] {
  const words = text.toLowerCase().split(/[^a-z]+/);
  const vector = vocabulary.map((term) => words.filter((word) => word.startsWith(term)).length);
  const norm = Math.hypot(...vector) || 1;
  return vector.map((value) => value / norm);
}

async function startEmbeddingsServer(): Promise<{ server: Server; url: string; requests: Array<{ path: string; model: string; inputs: string[]; authorization: string }> }> {
  const requests: Array<{ path: string; model: string; inputs: string[]; authorization: string }> = [];
  const server = createServer(async (req: IncomingMessage, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    const inputs: string[] = Array.isArray(body.input) ? body.input : [body.input];
    requests.push({ path: req.url ?? '', model: body.model, inputs, authorization: String(req.headers.authorization ?? '') });
    if (req.url !== '/v1/embeddings') {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ object: 'list', model: body.model, data: inputs.map((input, index) => ({ object: 'embedding', index, embedding: loopbackEmbedding(input) })) }));
  });
  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('embeddings server has no port');
  return { server, url: `http://127.0.0.1:${address.port}`, requests };
}

const volcanoHtml = `<!doctype html><html><head><title>Volcanoes</title><style>.hidden { color: red; }</style><script>window.trackingPixel = 'collect';</script></head><body><nav>Home | About</nav><h1>How basalt lava forms</h1><p>During an eruption the volcano pushes magma to the surface, where it cools into basalt lava fields.</p><p>Fast cooling basalt lava creates glassy rock &amp; ropey pahoehoe.</p></body></html>`;
const sourdoughMarkdown = `# Sourdough starter\n\nA sourdough **starter** is flour and water kept alive by wild yeast.\n\n## Fermentation\n\nLong fermentation gives sourdough bread its sour taste and open crumb.\n`;
const tidesText = 'The moon pulls the ocean and makes the tide rise and fall twice a day.';

const plainPdfLine = 'Harbour tide tables list every high water';
const compressedPdfLine = 'Spring tides arrive after each full moon';
const compressedPdfStream = deflateSync(Buffer.from(`BT /F1 12 Tf 72 690 Td (${compressedPdfLine}) Tj ET`));
const harbourPdf = Buffer.concat([
  Buffer.from(`%PDF-1.4\n1 0 obj << /Length 64 >>\nstream\nBT /F1 12 Tf 72 712 Td (${plainPdfLine}) Tj ET\nendstream\nendobj\n`),
  Buffer.from(`2 0 obj << /Length ${compressedPdfStream.length} /Filter /FlateDecode >>\nstream\n`),
  compressedPdfStream,
  Buffer.from('\nendstream\nendobj\n%%EOF\n'),
]);

const knowledgeDocs = [
  { owner: 'primary', title: 'Harbour PDF', kind: 'pdf', base64: harbourPdf.toString('base64') },
  { owner: 'primary', title: 'Volcano field guide', kind: 'html', text: volcanoHtml },
  { owner: 'primary', title: 'Sourdough handbook', kind: 'markdown', text: sourdoughMarkdown },
  { owner: 'primary', title: 'Tide notes', kind: 'text', text: tidesText },
  { owner: 'other', title: 'Other agent volcano copy', kind: 'html', text: volcanoHtml },
];

test('store keeps projects, agents, spells and numbered releases after the process restarts', { timeout: 900_000 }, async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const names = { project: `project-${randomBytes(4).toString('hex')}`, agent: 'Librarian', spell: 'Answer questions' };
    const seeded = await probeJson('seed', { db, ...names, releaseNotes: ['first cut', 'second cut'] }, masterKey);

    const reopened = await probeJson('inspect', { db }, masterKey);

    expect(reopened.projects).toEqual([{ id: seeded.projectId, name: names.project }]);
    expect(reopened.agents).toEqual([{ id: seeded.agentId, name: names.agent, projectId: seeded.projectId, pinnedReleaseId: seeded.releases[1].id }]);
    expect(reopened.spells).toEqual([{ id: seeded.spellId, name: names.spell, releases: [{ id: seeded.releases[0].id, version: 1, notes: 'first cut' }, { id: seeded.releases[1].id, version: 2, notes: 'second cut' }] }]);
    expect(reopened.journalMode).toBe('wal');
    expect(reopened.tables).toEqual(expect.arrayContaining(['projects', 'agents', 'spells', 'spell_releases', 'credentials', 'chat_messages', 'memories', 'knowledge_docs', 'knowledge_chunks', 'knowledge_chunks_fts', 'webhooks', 'execution_runs', 'llm_requests', 'plugin_state']));
  });
});

test('a stored credential is ciphertext on disk, hidden from metadata and revealed only with the right master key', async () => {
  await withDatabase(async (db, dir) => {
    const masterKey = randomBytes(32).toString('hex');
    const secret = `sk-live-${randomBytes(12).toString('hex')}`;

    const stored = await probeJson('credential', { db, name: 'openai', provider: 'openai', secret }, masterKey);
    const raw = await rawDatabaseBytes(dir);
    const revealed = await probeJson('reveal', { db, id: stored.id }, masterKey);
    const wrongKey = await probe('reveal', { db, id: stored.id }, randomBytes(32).toString('hex'));

    expect(raw.includes(Buffer.from(secret))).toBe(false);
    expect(stored.ciphertextHex.includes(Buffer.from(secret).toString('hex'))).toBe(false);
    expect(stored.ciphertextHex.length).toBeGreaterThan(Buffer.from(secret).toString('hex').length);
    expect(JSON.stringify(stored.meta)).not.toContain(secret);
    expect(stored.metaDebug).not.toContain(secret);
    expect(stored.meta.hint).toBe(`…${secret.slice(-4)}`);
    expect(revealed.secret).toBe(secret);
    expect(wrongKey.ok).toBe(false);
  });
});

test('hybrid knowledge search ranks the matching HTML and Markdown documents first for the agent with the local and the HTTP embedder', async () => {
  const embeddings = await startEmbeddingsServer();
  try {
    for (const embedder of ['local', 'http']) {
      const masterKey = randomBytes(32).toString('hex');
      const searchFreshStore = (query: string) =>
        withDatabase((db) => probeJson('knowledge', { db, embedder, embeddingsUrl: `${embeddings.url}/v1`, model: 'loopback-embed', docs: knowledgeDocs, limit: 5, query }, masterKey));

      const volcano = await searchFreshStore('how does basalt lava form in an eruption');
      const sourdough = await searchFreshStore('sourdough starter fermentation');
      {

        expect(volcano.hits[0].title).toBe('Volcano field guide');
        expect(sourdough.hits[0].title).toBe('Sourdough handbook');
        expect(volcano.hits.map((hit: any) => hit.owner)).not.toContain('other');
        expect(sourdough.hits.map((hit: any) => hit.owner)).not.toContain('other');
        expect(volcano.hits[0].score).toBeGreaterThan(volcano.hits[volcano.hits.length - 1].score);
        const volcanoText = volcano.chunks['Volcano field guide'].join(' ');
        expect(volcanoText).toContain('During an eruption the volcano pushes magma to the surface');
        expect(volcanoText).toContain('glassy rock & ropey pahoehoe');
        expect(volcanoText).not.toContain('trackingPixel');
        expect(volcanoText).not.toContain('color: red');
        expect(volcanoText).not.toContain('<p>');
        const sourdoughText = sourdough.chunks['Sourdough handbook'].join(' ');
        expect(sourdoughText).toContain('A sourdough starter is flour and water kept alive by wild yeast.');
        expect(sourdoughText).not.toContain('**');
        expect(sourdoughText).not.toContain('# ');
        const harbourText = volcano.chunks['Harbour PDF'].join(' ');
        expect(harbourText).toContain(plainPdfLine);
        expect(harbourText).toContain(compressedPdfLine);
      }
    }
    const embeddingRequests = embeddings.requests.filter((request) => request.path === '/v1/embeddings');
    expect(embeddingRequests.length).toBe(embeddings.requests.length);
    expect(embeddingRequests.length).toBeGreaterThan(0);
    expect(new Set(embeddingRequests.map((request) => request.model))).toEqual(new Set(['loopback-embed']));
    expect(Math.max(...embeddingRequests.map((request) => request.inputs.length))).toBeGreaterThan(1);
  } finally {
    await new Promise((closed) => embeddings.server.close(closed));
  }
});

test('PDF extraction rejects bytes that are not a PDF and a PDF whose stream never ends', async () => {
  const masterKey = randomBytes(32).toString('hex');
  const notPdf = await probe('extract', { kind: 'pdf', base64: randomBytes(256).toString('base64') }, masterKey);
  const truncated = await probe('extract', { kind: 'pdf', base64: Buffer.from(`%PDF-1.4\n1 0 obj << /Length 64 >>\nstream\nBT (${plainPdfLine}) Tj`).toString('base64') }, masterKey);
  const readable = await probeJson('extract', { kind: 'pdf', base64: harbourPdf.toString('base64') }, masterKey);

  expect(notPdf.ok).toBe(false);
  expect(notPdf.ok ? '' : notPdf.stderr).toContain('missing %PDF- header');
  expect(truncated.ok).toBe(false);
  expect(truncated.ok ? '' : truncated.stderr).toContain('stream without endstream');
  expect(readable.text).toBe(`${plainPdfLine}\n\n${compressedPdfLine}`);
});

test('memory search returns the agent memory closest to the query first and never another agent memory', async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const memories = [
      { owner: 'primary', content: 'The user prefers green tea with honey in the morning.' },
      { owner: 'primary', content: 'The user is allergic to peanuts and tree nuts.' },
      { owner: 'primary', content: 'The user drives a blue bicycle to the office.' },
      { owner: 'other', content: 'The user prefers green tea with honey in the morning.' },
    ];

    const found = await probeJson('memories', { db, memories, query: 'what tea does the user like in the morning', limit: 10 }, masterKey);

    expect(found.hits[0].content).toBe(memories[0].content);
    expect(found.hits[0].owner).toBe('primary');
    expect(found.hits.map((hit: any) => hit.owner)).not.toContain('other');
    expect(found.hits).toHaveLength(3);
    const scores = found.hits.map((hit: any) => hit.score);
    expect(scores).toEqual([...scores].sort((a: number, b: number) => b - a));
  });
});

test('webhook paths are stored trimmed of slashes, found with or without them, and empty or traversal paths are refused', async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const lookups = ['/hooks/shop', 'hooks/shop', '/hooks/shop/', 'hooks/shop/'];
    const refused = ['/', '   ', '//', 'hooks/../admin'];

    const outcome = await probeJson('webhooks', { db, create: '/hooks/shop', lookups, refused }, masterKey);

    expect(outcome.created.path).toBe('hooks/shop');
    expect(outcome.found.map((hit: any) => hit.lookup)).toEqual(lookups);
    expect(outcome.found.map((hit: any) => hit.id)).toEqual(lookups.map(() => outcome.created.id));
    expect(outcome.refused.map((miss: any) => miss.path)).toEqual(refused);
    expect(outcome.refused.slice(0, 3).map((miss: any) => miss.error)).toEqual(refused.slice(0, 3).map(() => 'invalid input: webhook path cannot be empty'));
    expect(outcome.refused[3].error).toBe('invalid input: webhook path cannot contain ".." segments');
  });
});

function flatePdf(streams: Buffer[]): Buffer {
  const objects = streams.map((stream, index) => Buffer.concat([Buffer.from(`${index + 1} 0 obj << /Length ${stream.length} /Filter /FlateDecode >>\nstream\n`), stream, Buffer.from('\nendstream\nendobj\n')]));
  return Buffer.concat([Buffer.from('%PDF-1.4\n'), ...objects, Buffer.from('%%EOF\n')]);
}

test('PDF extraction refuses a FlateDecode stream that inflates past 4 MiB and a document whose streams inflate past 16 MiB in total', async () => {
  const masterKey = randomBytes(32).toString('hex');
  const mebibyte = 1024 * 1024;
  const readableStream = deflateSync(Buffer.from(`BT /F1 12 Tf 72 690 Td (${compressedPdfLine}) Tj ET`));
  const singleBombStream = deflateSync(Buffer.alloc(5 * mebibyte));
  const nearLimitStream = deflateSync(Buffer.alloc(3.5 * mebibyte));
  const singleBomb = flatePdf([readableStream, singleBombStream]);
  const spreadBomb = flatePdf([readableStream, ...Array.from({ length: 5 }, () => nearLimitStream)]);
  expect(singleBomb.length).toBeLessThan(64 * 1024);
  expect(spreadBomb.length).toBeLessThan(64 * 1024);

  const single = await probe('extract', { kind: 'pdf', base64: singleBomb.toString('base64') }, masterKey);
  const spread = await probe('extract', { kind: 'pdf', base64: spreadBomb.toString('base64') }, masterKey);
  const underLimit = await probeJson('extract', { kind: 'pdf', base64: flatePdf([readableStream, nearLimitStream]).toString('base64') }, masterKey);

  expect(single.ok).toBe(false);
  expect(single.ok ? '' : single.stderr).toContain('pdf stream exceeds decompression limit');
  expect(spread.ok).toBe(false);
  expect(spread.ok ? '' : spread.stderr).toContain('pdf stream exceeds decompression limit');
  expect(underLimit.text).toBe(compressedPdfLine);
});

test('two concurrent agent patches to different fields both land, the rename and the disable', async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const rounds = 24;

    const outcome = await probeJson('concurrent-update', { db, rounds, renamePrefix: 'renamed-agent' }, masterKey);

    expect(outcome.agents).toHaveLength(rounds);
    expect(outcome.agents.map((agent: any) => agent.name)).toEqual(Array.from({ length: rounds }, (_, index) => `renamed-agent-${index}`));
    expect(outcome.agents.map((agent: any) => agent.enabled)).toEqual(Array.from({ length: rounds }, () => false));
    expect(outcome.agents.map((agent: any) => agent.description)).toEqual(Array.from({ length: rounds }, () => 'original description'));
  });
});

test('a release created from an explicit graph snapshot pins that snapshot even while the draft spell is edited at the same time', async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const spec = { db, draft: 'draft before release', snapshot: 'validated snapshot', edit: 'edited during release', rounds: 12 };

    const outcome = await probeJson('pinned-release', spec, masterKey);

    expect(outcome.releases).toHaveLength(spec.rounds);
    expect(outcome.releases.map((release: any) => release.description)).toEqual(Array.from({ length: spec.rounds }, (_, index) => `${spec.snapshot} ${index}`));
    expect(outcome.releases.map((release: any) => release.version)).toEqual(Array.from({ length: spec.rounds }, (_, index) => index + 1));
    expect(outcome.releases.map((release: any) => release.contentHash)).toEqual(outcome.releases.map((release: any) => release.snapshotHash));
    expect(outcome.draftDescription).toMatch(new RegExp(`^${spec.edit} \\d+$`));
    expect(outcome.foreignGraphError).toBe('invalid input: release graph belongs to a different spell');
  });
});

const weakKeyError = 'invalid input: vault master key must be 32 bytes encoded as 64 hex characters or base64';

test('deleting a credential that signs a webhook is refused as a conflict and the webhook stays signed until the webhook itself is deleted', async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const secret = `whsec-${randomBytes(12).toString('hex')}`;

    const guarded = await probeJson('credential-guard', { db, path: '/hooks/signed', secret }, masterKey);
    const webhookRows = queryDatabase<{ secret_credential_id: string | null }>(db, 'SELECT secret_credential_id FROM webhooks WHERE id = ?', guarded.webhookId);
    const credentialRows = queryDatabase<{ id: string }>(db, 'SELECT id FROM credentials WHERE id = ?', guarded.credentialId);
    const revealed = await probeJson('reveal', { db, id: guarded.credentialId }, masterKey);

    expect(guarded.blockedError).toBe('conflict: credential is referenced by a webhook');
    expect(guarded.signedWebhookSecret).toBe(guarded.credentialId);
    expect(webhookRows).toEqual([{ secret_credential_id: guarded.credentialId }]);
    expect(credentialRows).toEqual([{ id: guarded.credentialId }]);
    expect(revealed.secret).toBe(secret);

    const released = await probeJson('credential-release', { db, webhookId: guarded.webhookId, credentialId: guarded.credentialId }, masterKey);

    expect(released).toEqual({ webhookDeleted: true, credentialDeleted: true, missingWebhookError: `not found: webhook ${guarded.webhookId}` });
    expect(queryDatabase(db, 'SELECT id FROM webhooks WHERE id = ?', guarded.webhookId)).toEqual([]);
    expect(queryDatabase(db, 'SELECT id FROM credentials WHERE id = ?', guarded.credentialId)).toEqual([]);
  });
});

test('clearing an agent memories removes exactly that agent memories, reports how many went, and leaves its recall empty while another agent keeps its own', async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const memories = [
      { owner: 'primary', content: 'The user prefers green tea with honey in the morning.' },
      { owner: 'primary', content: 'The user is allergic to peanuts and tree nuts.' },
      { owner: 'primary', content: 'The user drives a blue bicycle to the office.' },
      { owner: 'other', content: 'The other user prefers black coffee in the morning.' },
    ];
    const primaryCount = memories.filter((memory) => memory.owner === 'primary').length;

    const outcome = await probeJson('clear-memories', { db, memories, query: 'what does the user drink in the morning' }, masterKey);
    const remaining = queryDatabase<{ content: string }>(db, 'SELECT content FROM memories ORDER BY content');

    expect(outcome.recallBefore).toHaveLength(primaryCount);
    expect(outcome.removed).toBe(primaryCount);
    expect(outcome.recallAfter).toEqual([]);
    expect(outcome.removedAgain).toBe(0);
    expect(outcome.otherRecallAfter).toEqual(memories.filter((memory) => memory.owner === 'other').map((memory) => memory.content));
    expect(remaining).toEqual(memories.filter((memory) => memory.owner === 'other').map((memory) => ({ content: memory.content })));
  });
});

test('a recorded run is found by its correlation id for its own agent only, the newest one wins, and an unknown id finds nothing', async () => {
  await withDatabase(async (db) => {
    const masterKey = randomBytes(32).toString('hex');
    const correlation = `corr-${randomBytes(6).toString('hex')}`;
    const otherCorrelation = `corr-${randomBytes(6).toString('hex')}`;

    const outcome = await probeJson('run-correlation', { db, correlation, otherCorrelation, unknown: 'corr-never-recorded' }, masterKey);

    expect(outcome.found.primary).toEqual({ runId: outcome.runs.primaryNewer, correlation });
    expect(outcome.found.primaryOther).toEqual({ runId: outcome.runs.primaryOther, correlation: otherCorrelation });
    expect(outcome.found.otherAgent).toEqual({ runId: outcome.runs.otherAgent, correlation });
    expect(outcome.found.unknown).toBeNull();
    expect(outcome.runs.primaryOlder).not.toBe(outcome.runs.primaryNewer);
  });
});

test('the vault accepts only a 32-byte master key written as 64 hex characters or base64 and refuses passphrases and wrong-length keys', async () => {
  const key = randomBytes(32);
  const accepted = [
    { label: 'hex', material: key.toString('hex') },
    { label: 'hex with spaces around', material: `  ${key.toString('hex').toUpperCase()}\n` },
    { label: 'base64', material: key.toString('base64') },
    { label: 'base64 unpadded', material: key.toString('base64').replace(/=+$/, '') },
    { label: 'base64url', material: key.toString('base64url') },
  ];
  const refused = [
    { label: 'passphrase', material: 'weak-passphrase' },
    { label: 'short hex', material: randomBytes(16).toString('hex') },
    { label: 'long hex', material: randomBytes(33).toString('hex') },
    { label: 'short base64', material: randomBytes(24).toString('base64') },
    { label: 'blank', material: '   ' },
  ];
  const plaintext = `sk-${randomBytes(10).toString('hex')}`;

  const outcome = await probeJson('vault-keys', { keys: [...accepted, ...refused], plaintext }, key.toString('hex'));
  const weakEnv = await withDatabase((db) => probe('credential', { db, name: 'openai', provider: 'openai', secret: plaintext }, 'weak-passphrase'));

  expect(outcome.keys).toEqual([
    ...accepted.map((entry) => ({ label: entry.label, accepted: true, error: null, opened: plaintext })),
    ...refused.map((entry) => ({ label: entry.label, accepted: false, error: weakKeyError, opened: null })),
  ]);
  expect(weakEnv.ok).toBe(false);
  expect(weakEnv.ok ? '' : weakEnv.stderr).toContain(weakKeyError);
});

test('only real in-memory SQLite URLs count as in-memory and share one database across concurrent queries, while file paths that merely mention memory stay on disk', async () => {
  await withDatabase(async (_db, dir) => {
    const masterKey = randomBytes(32).toString('hex');
    const trickyDir = join(dir, 'mode=memory');
    await mkdir(trickyDir);
    const trickyFile = join(trickyDir, 'app.db');
    const urls = [
      { url: ':memory:', inMemory: true },
      { url: 'sqlite::memory:', inMemory: true },
      { url: 'sqlite://:memory:', inMemory: true },
      { url: 'sqlite://:memory:?cache=shared', inMemory: true },
      { url: 'sqlite:file:studio?mode=memory&cache=shared', inMemory: true },
      { url: 'file:studio?cache=shared&mode=memory', inMemory: true },
      { url: `sqlite://${trickyFile}`, inMemory: false },
      { url: trickyFile, inMemory: false },
      { url: `sqlite://${join(dir, 'my:memory:.db')}`, inMemory: false },
      { url: `sqlite://${join(dir, 'app.db')}?mode=rwc`, inMemory: false },
    ];
    const writers = 24;

    const outcome = await probeJson('memory-urls', { urls: urls.map((entry) => entry.url), fileUrl: `sqlite://${trickyFile}`, writers }, masterKey);

    expect(outcome.classified).toEqual(urls);
    expect(outcome.concurrent.errors).toEqual([]);
    expect(outcome.concurrent.projects).toEqual(Array.from({ length: writers }, (_, index) => `writer-${String(index).padStart(2, '0')}`));
    expect(outcome.file.projects).toEqual(['on-disk']);
    expect((await stat(trickyFile)).size).toBeGreaterThan(0);
    expect(queryDatabase(trickyFile, 'SELECT name FROM projects')).toEqual([{ name: 'on-disk' }]);
  });
});
