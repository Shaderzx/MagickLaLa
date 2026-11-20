import { test as base, expect } from 'e2e';
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { createHmac, randomBytes, randomUUID } from 'node:crypto';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { request as httpRequest } from 'node:http';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const binary = join(repoRoot, 'target', 'debug', 'magicklala');
const buildBudget = { timeout: 900_000 };
const masterKey = 'b2'.repeat(32);

type Reply = { status: number; body: any; text: string; headers: Headers };
type Api = (path: string, init?: { method?: string; body?: unknown; headers?: Record<string, string> }) => Promise<Reply>;
type Server = { baseUrl: string; wsUrl: string; api: Api };
type Launched = { server: Server; stop: () => Promise<void> };

async function freePort(): Promise<number> {
  const probe = createServer();
  probe.listen(0, '127.0.0.1');
  await once(probe, 'listening');
  const address = probe.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  probe.close();
  await once(probe, 'close');
  return port;
}

async function buildCli(): Promise<void> {
  await run('cargo', ['build', '-q', '-p', 'magicklala-cli'], { cwd: repoRoot, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
}

async function stopProcess(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit');
  child.kill('SIGTERM');
  await exited;
}

function parseBody(text: string) {
  const trimmed = text.trim();
  return trimmed.startsWith('{') || trimmed.startsWith('[') ? JSON.parse(trimmed) : text;
}

function client(baseUrl: string, defaultHeaders: Record<string, string>): Api {
  return async (path, init = {}) => {
    const response = await fetch(`${baseUrl}${path}`, {
      method: init.method ?? (init.body === undefined ? 'GET' : 'POST'),
      headers: { ...defaultHeaders, ...(init.body === undefined ? {} : { 'content-type': 'application/json' }), ...init.headers },
      body: init.body === undefined ? undefined : JSON.stringify(init.body),
    });
    const text = await response.text();
    return { status: response.status, text, body: parseBody(text), headers: response.headers };
  };
}

function keylessEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env };
  delete env.MAGICKLALA_MASTER_KEY;
  delete env.MAGICKLALA_VAULT_KEY;
  return env;
}

async function launchServe(env: Record<string, string> = {}, databaseUrl?: (dir: string) => string): Promise<Launched> {
  const port = await freePort();
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-serve-'));
  const url = databaseUrl ? databaseUrl(dir) : `sqlite://${join(dir, 'magicklala.db')}`;
  const child = spawn(binary, ['serve', '--bind', `127.0.0.1:${port}`, '--database-url', url], {
    cwd: dir,
    env: { ...keylessEnv(), MAGICKLALA_MASTER_KEY: masterKey, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout?.resume();
  child.stderr?.resume();
  const baseUrl = `http://127.0.0.1:${port}`;
  const stop = async () => {
    await stopProcess(child);
    await rm(dir, { recursive: true, force: true });
  };
  try {
    await expect.poll(() => fetch(`${baseUrl}/health`).then((response) => response.status, () => 0), { timeout: 30_000 }).toBe(200);
  } catch (error) {
    await stop();
    throw error;
  }
  const token = env.MAGICKLALA_API_TOKEN;
  const api = client(baseUrl, token ? { authorization: `Bearer ${token}` } : {});
  return { server: { baseUrl, wsUrl: `ws://127.0.0.1:${port}/ws`, api }, stop };
}

const test = base.extend<{ server: Server }>({
  server: async ({}, use) => {
    await buildCli();
    const { server, stop } = await launchServe();
    try {
      await use(server);
    } finally {
      await stop();
    }
  },
});

function rawRequest(url: string, init: { method?: string; headers?: Record<string, string>; body?: string | Buffer } = {}) {
  return new Promise<{ status: number; headers: Record<string, string | string[] | undefined> }>((resolvePromise, reject) => {
    const outgoing = httpRequest(url, { method: init.method ?? 'GET', headers: init.headers });
    outgoing.on('response', (response) => {
      response.resume();
      resolvePromise({ status: response.statusCode ?? 0, headers: response.headers });
    });
    outgoing.on('upgrade', (response, socket) => {
      socket.destroy();
      resolvePromise({ status: response.statusCode ?? 0, headers: response.headers });
    });
    outgoing.on('error', reject);
    outgoing.end(init.body);
  });
}

function upgradeHeaders(extra: Record<string, string> = {}) {
  return { connection: 'Upgrade', upgrade: 'websocket', 'sec-websocket-version': '13', 'sec-websocket-key': randomBytes(16).toString('base64'), ...extra };
}

function emptySpell(name: string) {
  return {
    schema_version: 1,
    id: randomUUID(),
    name,
    description: 'loopback spell',
    variables: [],
    nodes: [{ id: 'message', kind: 'event/on_message', position: [0, 0], config: {} }],
    wires: [],
    subgraphs: [],
  };
}

function echoSpell(name: string) {
  return {
    ...emptySpell(name),
    nodes: [
      { id: 'message', kind: 'event/on_message', position: [0, 0], config: {} },
      { id: 'send', kind: 'io/send_message', position: [280, 0], config: {} },
    ],
    wires: [
      { id: 'w_flow', from_node: 'message', from_socket: 'out', to_node: 'send', to_socket: 'in' },
      { id: 'w_content', from_node: 'message', from_socket: 'content', to_node: 'send', to_socket: 'content' },
    ],
  };
}

function fixedReplySpell(id: string, name: string, reply: string) {
  return {
    ...emptySpell(name),
    id,
    nodes: [
      { id: 'message', kind: 'event/on_message', position: [0, 0], config: {} },
      { id: 'text', kind: 'text/template', position: [280, 120], config: { template: reply } },
      { id: 'send', kind: 'io/send_message', position: [560, 0], config: {} },
    ],
    wires: [
      { id: 'w_flow', from_node: 'message', from_socket: 'out', to_node: 'send', to_socket: 'in' },
      { id: 'w_text', from_node: 'text', from_socket: 'text', to_node: 'send', to_socket: 'content' },
    ],
  };
}

function webhookSpell(name: string) {
  return { ...emptySpell(name), nodes: [{ id: 'hook', kind: 'event/on_webhook', position: [0, 0], config: {} }] };
}

async function createAgentWithRelease(server: Server, label: string, graph: object = emptySpell(`${label} spell`)) {
  const project = await server.api('/api/v1/projects', { body: { name: `${label} project`, description: 'e2e' } });
  expect(project.status).toBe(201);
  const spell = await server.api('/api/v1/spells', { body: { project_id: project.body.id, name: `${label} spell`, graph } });
  expect(spell.status).toBe(201);
  const agent = await server.api('/api/v1/agents', { body: { project_id: project.body.id, name: `${label} agent`, description: 'e2e agent', settings: {} } });
  expect(agent.status).toBe(201);
  const release = await server.api(`/api/v1/agents/${agent.body.id}/releases`, { body: { spell_id: spell.body.id, notes: 'first release' } });
  expect(release.status).toBe(201);
  return { project: project.body, spell: spell.body, agent: agent.body, release: release.body };
}

async function startedAgent(server: Server, label: string, graph: object) {
  const created = await createAgentWithRelease(server, label, graph);
  const started = await server.api(`/api/v1/agents/${created.agent.id}/start`, { body: {} });
  expect(started.status).toBe(200);
  return created;
}

function sign(secret: string, body: string, timestamp = Math.floor(Date.now() / 1000)) {
  return { 'X-MagickLaLa-Timestamp': String(timestamp), 'X-MagickLaLa-Signature': `sha256=${createHmac('sha256', secret).update(`${timestamp}.${body}`).digest('hex')}` };
}

async function launchToken(token: string, extra: Record<string, string> = {}) {
  return launchServe({ MAGICKLALA_API_TOKEN: token, ...extra });
}

function runCli(args: string[], env: NodeJS.ProcessEnv, timeout = 60_000) {
  return run(binary, args, { env, timeout }).then(
    () => ({ code: 0 as number | null, stderr: '' }),
    (error: { code: number | null; stderr: string }) => ({ code: error.code, stderr: error.stderr }),
  );
}
test('GET /api/v1/openapi.json serves an OpenAPI 3.1 document listing every control plane path, its error statuses and its auth schemes', buildBudget, async ({ server }) => {
  const doc = await server.api('/api/v1/openapi.json');
  expect(doc.status).toBe(200);
  expect(String(doc.body.openapi)).toMatch(/^3\.1/);
  const paths = Object.keys(doc.body.paths);
  expect(paths).toEqual(
    expect.arrayContaining([
      '/api/v1/health',
      '/api/v1/node-specs',
      '/api/v1/projects',
      '/api/v1/spells',
      '/api/v1/spells/{id}',
      '/api/v1/spells/{id}/graph',
      '/api/v1/spells/{id}/validate',
      '/api/v1/spells/{id}/run',
      '/api/v1/agents',
      '/api/v1/agents/{id}',
      '/api/v1/agents/{id}/releases',
      '/api/v1/agents/{id}/start',
      '/api/v1/agents/{id}/stop',
      '/api/v1/agents/{id}/chat',
      '/api/v1/agents/{id}/knowledge',
      '/api/v1/agents/{id}/knowledge/search',
      '/api/v1/agents/{id}/memories',
      '/api/v1/agents/{id}/memories/search',
      '/api/v1/agents/{id}/runs',
      '/api/v1/credentials',
      '/api/v1/credentials/{id}',
      '/api/v1/webhooks',
      '/api/v1/webhooks/{path}',
      '/api/v1/webhooks/{id}',
      '/api/v1/ws-ticket',
      '/api/v1/copilot/suggest',
    ]),
  );
  expect(Object.keys(doc.body.paths['/api/v1/agents/{id}/chat'].post.responses).sort()).toEqual(['200', '400', '401', '404', '409', '421', '502', '503', '504']);
  expect(Object.keys(doc.body.paths['/api/v1/webhooks/{id}'].delete.responses)).toEqual(expect.arrayContaining(['204', '401', '404']));
  expect(Object.keys(doc.body.paths['/api/v1/ws-ticket'].post.responses)).toEqual(expect.arrayContaining(['201', '401']));
  expect(doc.body.paths['/ws'].get.parameters.map((parameter: { name: string }) => parameter.name)).toEqual(expect.arrayContaining(['agent_id', 'ticket']));
  expect(doc.body.paths['/ws'].get.parameters.map((parameter: { name: string }) => parameter.name)).not.toContain('token');
  expect(Object.keys(doc.body.paths['/api/v1/spells/{id}/run'].post.responses)).toEqual(expect.arrayContaining(['200', '400', '401', '404', '422', '504']));
  const webhook = doc.body.paths['/api/v1/webhooks/{path}'].post;
  expect(Object.keys(webhook.responses)).toEqual(expect.arrayContaining(['202', '400', '401', '404', '409', '413']));
  expect(webhook.parameters.map((parameter: { name: string; in: string }) => [parameter.in, parameter.name])).toEqual(
    expect.arrayContaining([['path', 'path'], ['header', 'X-MagickLaLa-Signature'], ['header', 'X-MagickLaLa-Timestamp']]),
  );
  expect(webhook.security).toEqual([]);
  expect(doc.body.components.securitySchemes.bearer_token).toMatchObject({ type: 'http', scheme: 'bearer' });
  expect(doc.body.security).toEqual([{ bearer_token: [] }]);
  const legacy = await server.api('/api/nodes');
  expect(legacy.status).toBe(200);
  expect(legacy.body).toEqual((await server.api('/api/v1/node-specs')).body);
});
test('An agent published with a validated spell release moves from stopped to running and back to stopped', buildBudget, async ({ server }) => {
  const { project, spell, agent, release } = await createAgentWithRelease(server, 'lifecycle');
  const validation = await server.api(`/api/v1/spells/${spell.id}/validate`, { body: {} });
  expect(validation.status).toBe(200);
  expect(validation.body).toEqual({ valid: true, errors: [] });
  expect(release.version).toBe(1);
  expect(release.spell_id).toBe(spell.id);
  const listed = await server.api(`/api/v1/agents?project_id=${project.id}`);
  expect(listed.body.map((item: { id: string }) => item.id)).toEqual([agent.id]);
  expect((await server.api(`/api/v1/agents/${agent.id}`)).body).toMatchObject({ id: agent.id, pinned_release_id: release.id, status: { state: 'stopped' } });
  const started = await server.api(`/api/v1/agents/${agent.id}/start`, { body: {} });
  expect(started.status).toBe(200);
  expect((await server.api(`/api/v1/agents/${agent.id}`)).body.status).toEqual({ state: 'running' });
  const startedTwice = await server.api(`/api/v1/agents/${agent.id}/start`, { body: {} });
  expect(startedTwice.status).toBe(409);
  expect(startedTwice.body).toMatchObject({ code: 'conflict' });
  const stopped = await server.api(`/api/v1/agents/${agent.id}/stop`, { body: {} });
  expect(stopped.status).toBe(200);
  expect((await server.api(`/api/v1/agents/${agent.id}`)).body.status).toEqual({ state: 'stopped' });
  const missing = await server.api(`/api/v1/agents/${randomUUID()}`);
  expect(missing.status).toBe(404);
  expect(missing.body).toMatchObject({ code: 'not_found' });
});

test('Stored credentials are listed as metadata only and never echo the plaintext secret', buildBudget, async ({ server }) => {
  const project = await server.api('/api/v1/projects', { body: { name: 'vault project', description: 'e2e' } });
  const secret = `sk-live-${randomUUID()}`;
  const created = await server.api('/api/v1/credentials', { body: { project_id: project.body.id, name: 'openai key', provider: 'openai', secret } });
  expect(created.status).toBe(201);
  expect(created.text).not.toContain(secret);
  const listed = await server.api(`/api/v1/credentials?project_id=${project.body.id}`);
  expect(listed.status).toBe(200);
  expect(listed.text).not.toContain(secret);
  expect(listed.body).toEqual([expect.objectContaining({ id: created.body.id, name: 'openai key', provider: 'openai', hint: `…${secret.slice(-4)}` })]);
  expect(Object.keys(listed.body[0]).sort()).toEqual(['created_at', 'hint', 'id', 'name', 'project_id', 'provider', 'rotated_at']);
  const deleted = await server.api(`/api/v1/credentials/${created.body.id}`, { method: 'DELETE' });
  expect(deleted.status).toBe(204);
  expect((await server.api(`/api/v1/credentials?project_id=${project.body.id}`)).body).toEqual([]);
});

test('A WebSocket client on /ws receives the live agent_status_changed event when an agent starts', buildBudget, async ({ server }) => {
  const { agent } = await createAgentWithRelease(server, 'websocket');
  const socket = new WebSocket(server.wsUrl);
  const frames: any[] = [];
  socket.addEventListener('message', (event) => frames.push(JSON.parse(String(event.data))));
  try {
    await once(socket, 'open');
    socket.send(JSON.stringify({ type: 'ping' }));
    await expect.poll(() => frames.some((frame) => frame.type === 'pong'), { timeout: 10_000 }).toBe(true);
    const started = await server.api(`/api/v1/agents/${agent.id}/start`, { body: {} });
    expect(started.status).toBe(200);
    await expect
      .poll(() => frames.find((frame) => frame.type === 'agent_status_changed' && frame.agent_id === agent.id), { timeout: 10_000 })
      .toEqual({ type: 'agent_status_changed', agent_id: agent.id, status: { state: 'running' } });
  } finally {
    socket.close();
  }
});

test('magicklala spell specs prints the node catalog and spell validate accepts a valid spell and rejects a broken one', buildBudget, async ({ server }) => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-spell-'));
  try {
    const specs = await run(binary, ['spell', 'specs']);
    expect(JSON.parse(specs.stdout)).toEqual((await server.api('/api/v1/node-specs')).body);
    const validFile = join(dir, 'valid.json');
    await writeFile(validFile, JSON.stringify(emptySpell('cli valid spell')));
    const valid = await run(binary, ['spell', 'validate', validFile]);
    expect(valid.stdout.trim()).toBe('valid: cli valid spell');
    const brokenFile = join(dir, 'broken.json');
    await writeFile(brokenFile, JSON.stringify({ ...emptySpell('cli broken spell'), schema_version: 99 }));
    const broken = await run(binary, ['spell', 'validate', brokenFile]).then(
      () => ({ code: 0, stderr: '' }),
      (error: { code: number; stderr: string }) => ({ code: error.code, stderr: error.stderr }),
    );
    expect(broken.code).toBe(1);
    expect(broken.stderr).toContain('unsupported schema version 99, expected 1');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('A WebSocket opened with ?agent_id only receives live runtime events about that agent', buildBudget, async ({ server }) => {
  const watched = await createAgentWithRelease(server, 'watched');
  const other = await createAgentWithRelease(server, 'unwatched');
  const socket = new WebSocket(`${server.wsUrl}?agent_id=${watched.agent.id}`);
  const frames: any[] = [];
  socket.addEventListener('message', (event) => frames.push(JSON.parse(String(event.data))));
  try {
    await once(socket, 'open');
    expect((await server.api(`/api/v1/agents/${other.agent.id}/start`, { body: {} })).status).toBe(200);
    expect((await server.api(`/api/v1/agents/${watched.agent.id}/start`, { body: {} })).status).toBe(200);
    await expect
      .poll(() => frames.find((frame) => frame.type === 'agent_status_changed'), { timeout: 10_000 })
      .toEqual({ type: 'agent_status_changed', agent_id: watched.agent.id, status: { state: 'running' } });
    expect(frames.filter((frame) => frame.agent_id === other.agent.id)).toEqual([]);
  } finally {
    socket.close();
  }
});

test('magicklala serve refuses a file-backed database without MAGICKLALA_MASTER_KEY or MAGICKLALA_VAULT_KEY and starts once a key is set', buildBudget, async () => {
  await buildCli();
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-keyless-'));
  try {
    const port = await freePort();
    const refused = await run(binary, ['serve', '--bind', `127.0.0.1:${port}`, '--database-url', `sqlite://${join(dir, 'keyless.db')}`], { cwd: dir, env: keylessEnv(), timeout: 60_000 }).then(
      () => ({ code: 0 as number | null, stderr: '' }),
      (error: { code: number | null; stderr: string }) => ({ code: error.code, stderr: error.stderr }),
    );
    expect(refused.code).toBe(1);
    expect(refused.stderr).toContain('MAGICKLALA_MASTER_KEY');
    expect(refused.stderr).toContain('MAGICKLALA_VAULT_KEY');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
  const withVaultKey = await launchServe({ MAGICKLALA_VAULT_KEY: 'c3'.repeat(32), MAGICKLALA_MASTER_KEY: '' });
  try {
    expect((await withVaultKey.server.api('/api/v1/health')).body).toMatchObject({ status: 'ok' });
  } finally {
    await withVaultKey.stop();
  }
  const inMemory = await launchServe({ MAGICKLALA_MASTER_KEY: '' }, () => 'sqlite::memory:');
  try {
    expect((await inMemory.server.api('/api/v1/projects')).body).toEqual([]);
  } finally {
    await inMemory.stop();
  }
});

test('MAGICKLALA_API_TOKEN requires the bearer token on /api/v1 and /ws, never accepts it in the /ws URL, and /ws takes single-use tickets instead', buildBudget, async () => {
  await buildCli();
  const token = `tok-${randomUUID()}`;
  const { server, stop } = await launchServe({ MAGICKLALA_API_TOKEN: token });
  try {
    expect((await fetch(`${server.baseUrl}/health`)).status).toBe(200);
    const anonymous = await server.api('/api/v1/projects', { headers: { authorization: '' } });
    expect(anonymous.status).toBe(401);
    expect(anonymous.body).toMatchObject({ code: 'unauthorized' });
    expect((await server.api('/api/v1/projects', { headers: { authorization: `Bearer wrong-${token}` } })).status).toBe(401);
    expect((await server.api('/api/v1/projects', { headers: { authorization: `Basic ${token}` } })).status).toBe(401);
    const authorized = await server.api('/api/v1/projects');
    expect(authorized.status).toBe(200);
    expect(authorized.body).toEqual([]);
    expect((await server.api(`/api/v1/webhooks/${randomUUID()}`, { body: {}, headers: { authorization: '' } })).status).toBe(404);
    const wsHttp = server.baseUrl + '/ws';
    expect((await rawRequest(wsHttp, { headers: upgradeHeaders() })).status).toBe(401);
    expect((await rawRequest(`${wsHttp}?token=wrong-${token}`, { headers: upgradeHeaders() })).status).toBe(401);
    expect((await rawRequest(`${wsHttp}?token=${token}`, { headers: upgradeHeaders() })).status).toBe(401);
    expect((await rawRequest(wsHttp, { headers: upgradeHeaders({ authorization: `Bearer ${token}` }) })).status).toBe(101);
    expect((await server.api('/api/v1/ws-ticket', { body: {}, headers: { authorization: '' } })).status).toBe(401);
    const ticket = await server.api('/api/v1/ws-ticket', { body: {} });
    expect(ticket.status).toBe(201);
    expect(ticket.body.ticket).toMatch(/^[0-9a-f]{64}$/);
    expect(ticket.body.expires_in_secs).toBe(30);
    expect(ticket.text).not.toContain(token);
    expect((await rawRequest(`${wsHttp}?ticket=${ticket.body.ticket}`, { headers: upgradeHeaders() })).status).toBe(101);
    expect((await rawRequest(`${wsHttp}?ticket=${ticket.body.ticket}`, { headers: upgradeHeaders() })).status).toBe(401);
    expect((await rawRequest(`${wsHttp}?ticket=${'0'.repeat(64)}`, { headers: upgradeHeaders() })).status).toBe(401);
  } finally {
    await stop();
  }
});

test('CORS answers only allowlisted Studio origins and never grants a foreign Origin', buildBudget, async ({ server }) => {
  const preflight = (origin: string, url: string) =>
    rawRequest(url, { method: 'OPTIONS', headers: { origin, 'access-control-request-method': 'POST', 'access-control-request-headers': 'content-type,authorization' } });
  const projects = `${server.baseUrl}/api/v1/projects`;
  const foreign = await preflight('https://evil.example', projects);
  expect(foreign.headers['access-control-allow-origin']).toBeUndefined();
  expect((await rawRequest(projects, { headers: { origin: 'https://evil.example' } })).headers['access-control-allow-origin']).toBeUndefined();
  const studio = await preflight('http://127.0.0.1:4242', projects);
  expect(studio.headers['access-control-allow-origin']).toBe('http://127.0.0.1:4242');
  expect(String(studio.headers['access-control-allow-methods'])).toContain('POST');
  expect((await rawRequest(projects, { headers: { origin: 'http://localhost:3000' } })).headers['access-control-allow-origin']).toBe('http://localhost:3000');
  await buildCli();
  const custom = await launchServe({ MAGICKLALA_CORS_ORIGINS: 'https://studio.example, http://127.0.0.1:5173' });
  try {
    const customProjects = `${custom.server.baseUrl}/api/v1/projects`;
    expect((await preflight('https://studio.example', customProjects)).headers['access-control-allow-origin']).toBe('https://studio.example');
    expect((await rawRequest(customProjects, { headers: { origin: 'http://127.0.0.1:5173' } })).headers['access-control-allow-origin']).toBe('http://127.0.0.1:5173');
    expect((await preflight('http://127.0.0.1:4242', customProjects)).headers['access-control-allow-origin']).toBeUndefined();
    expect((await preflight('https://evil.example', customProjects)).headers['access-control-allow-origin']).toBeUndefined();
  } finally {
    await custom.stop();
  }
});

test('A webhook with a secret credential runs the agent only for fresh timestamped X-MagickLaLa-Signature payloads and rejects bodies over 1 MiB', buildBudget, async ({ server }) => {
  const { project, spell, agent, release } = await startedAgent(server, 'signed webhook', webhookSpell('signed webhook spell'));
  const secret = `whsec-${randomUUID()}`;
  const credential = await server.api('/api/v1/credentials', { body: { project_id: project.id, name: 'webhook secret', provider: 'webhook', secret } });
  expect(credential.status).toBe(201);
  const hook = await server.api('/api/v1/webhooks', { body: { agent_id: agent.id, path: '/orders/new/', secret_credential_id: credential.body.id } });
  expect(hook.status).toBe(201);
  expect(hook.body.path).toBe('orders/new');
  const payload = JSON.stringify({ type: 'order', id: 42 });
  const deliver = (body: string, signature: Record<string, string> = {}) =>
    fetch(`${server.baseUrl}/api/v1/webhooks/orders/new`, { method: 'POST', headers: { 'content-type': 'application/json', ...signature }, body });
  const unsigned = await deliver(payload);
  expect(unsigned.status).toBe(401);
  expect(await unsigned.json()).toMatchObject({ code: 'invalid_signature' });
  expect((await deliver(payload, sign(`not-${secret}`, payload))).status).toBe(401);
  expect((await deliver(payload, sign(secret, `${payload} `))).status).toBe(401);
  const { 'X-MagickLaLa-Signature': signatureOnly } = sign(secret, payload);
  const missingTimestamp = await deliver(payload, { 'X-MagickLaLa-Signature': signatureOnly });
  expect(missingTimestamp.status).toBe(401);
  expect((await missingTimestamp.json()).error).toContain('X-MagickLaLa-Timestamp');
  const stale = await deliver(payload, sign(secret, payload, Math.floor(Date.now() / 1000) - 600));
  expect(stale.status).toBe(401);
  expect((await stale.json()).error).toContain('timestamp');
  expect((await deliver(payload, { ...sign(secret, payload), 'X-MagickLaLa-Timestamp': 'yesterday' })).status).toBe(401);
  const bodyOnly = `sha256=${createHmac('sha256', secret).update(payload).digest('hex')}`;
  expect((await deliver(payload, { 'X-MagickLaLa-Timestamp': String(Math.floor(Date.now() / 1000)), 'X-MagickLaLa-Signature': bodyOnly })).status).toBe(401);
  const oversized = 'x'.repeat(1024 * 1024 + 1);
  expect((await deliver(oversized, sign(secret, oversized))).status).toBe(413);
  const malformed = '{"type":';
  expect((await deliver(malformed, sign(secret, malformed))).status).toBe(400);
  const signed = await deliver(payload, sign(secret, payload));
  expect(signed.status).toBe(202);
  expect(await signed.json()).toEqual({ accepted: true, webhook_id: hook.body.id, agent_id: agent.id });
  await expect.poll(async () => (await server.api(`/api/v1/agents/${agent.id}/runs`)).body.length, { timeout: 20_000 }).toBe(1);
  const runs = (await server.api(`/api/v1/agents/${agent.id}/runs`)).body;
  expect(runs).toEqual([expect.objectContaining({ agent_id: agent.id, spell_id: spell.id, release_id: release.id, status: 'succeeded', trigger: 'webhook', error: null })]);
  const otherProject = await server.api('/api/v1/projects', { body: { name: 'foreign secret project' } });
  const foreignSecret = await server.api('/api/v1/credentials', { body: { project_id: otherProject.body.id, name: 'foreign', provider: 'webhook', secret: `x-${secret}` } });
  const foreignHook = await server.api('/api/v1/webhooks', { body: { agent_id: agent.id, path: 'orders/foreign', secret_credential_id: foreignSecret.body.id } });
  expect(foreignHook.status).toBe(400);
});

test('Every agent chat is persisted as a run that GET /api/v1/agents/{id}/runs lists with its correlation id', buildBudget, async ({ server }) => {
  const { agent, spell, release } = await startedAgent(server, 'recorded chat', echoSpell('recorded chat spell'));
  const first = await server.api(`/api/v1/agents/${agent.id}/chat`, { body: { content: 'first hello' } });
  expect(first.status).toBe(200);
  expect(first.body.reply.content).toBe('first hello');
  const second = await server.api(`/api/v1/agents/${agent.id}/chat`, { body: { content: 'second hello' } });
  expect(second.status).toBe(200);
  const correlations = [first, second].map((chat) => chat.body.reply.metadata.correlation_id);
  await expect.poll(async () => (await server.api(`/api/v1/agents/${agent.id}/runs`)).body.length, { timeout: 20_000 }).toBe(2);
  const runs = (await server.api(`/api/v1/agents/${agent.id}/runs`)).body;
  expect(runs.map((item: { trace: { correlation_id: string } }) => item.trace.correlation_id).sort()).toEqual([...correlations].sort());
  for (const item of runs) expect(item).toMatchObject({ agent_id: agent.id, spell_id: spell.id, release_id: release.id, status: 'succeeded', trigger: 'rest' });
});

test('A chat with an agent whose spell sends no reply fails fast with 502 instead of waiting for the run timeout', buildBudget, async ({ server }) => {
  const { agent } = await startedAgent(server, 'silent', emptySpell('silent spell'));
  const response = await fetch(`${server.baseUrl}/api/v1/agents/${agent.id}/chat`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ content: 'anyone there?' }),
    signal: AbortSignal.timeout(20_000),
  });
  expect(response.status).toBe(502);
  expect(await response.json()).toEqual({ code: 'no_reply', error: 'agent produced no reply' });
});

test('PATCH /api/v1/agents/{id} with pinned_release_id switches a running agent onto that release and rejects a foreign release', buildBudget, async ({ server }) => {
  const graphId = randomUUID();
  const { project, spell, agent, release: firstRelease } = await startedAgent(server, 'pinning', fixedReplySpell(graphId, 'pinning spell', 'reply from release one'));
  expect((await server.api(`/api/v1/spells/${spell.id}/graph`, { method: 'PUT', body: fixedReplySpell(graphId, 'pinning spell', 'reply from release two') })).status).toBe(200);
  const secondRelease = await server.api(`/api/v1/agents/${agent.id}/releases`, { body: { notes: 'second release' } });
  expect(secondRelease.status).toBe(201);
  expect((await server.api(`/api/v1/agents/${agent.id}/chat`, { body: { content: 'which release?' } })).body.reply.content).toBe('reply from release two');
  const pinned = await server.api(`/api/v1/agents/${agent.id}`, { method: 'PATCH', body: { pinned_release_id: firstRelease.id } });
  expect(pinned.status).toBe(200);
  expect(pinned.body).toMatchObject({ id: agent.id, pinned_release_id: firstRelease.id, status: { state: 'running' } });
  expect((await server.api(`/api/v1/agents/${agent.id}/chat`, { body: { content: 'which release now?' } })).body.reply.content).toBe('reply from release one');
  const foreign = await createAgentWithRelease(server, 'foreign pin');
  const rejected = await server.api(`/api/v1/agents/${agent.id}`, { method: 'PATCH', body: { pinned_release_id: foreign.release.id } });
  expect(rejected.status).toBe(400);
  expect((await server.api(`/api/v1/agents/${agent.id}`)).body.pinned_release_id).toBe(firstRelease.id);
  expect(project.id).not.toBe(foreign.project.id);
});

test('Deleting an agent answers 404 for it afterwards with a message that names the agent once', buildBudget, async ({ server }) => {
  const { agent } = await startedAgent(server, 'deleted', echoSpell('deleted spell'));
  expect((await server.api(`/api/v1/agents/${agent.id}`, { method: 'DELETE' })).status).toBe(204);
  const gone = await server.api(`/api/v1/agents/${agent.id}`);
  expect(gone.status).toBe(404);
  expect(gone.body).toEqual({ code: 'not_found', error: `not found: agent ${agent.id}` });
  const chat = await server.api(`/api/v1/agents/${agent.id}/chat`, { body: { content: 'still there?' } });
  expect(chat.status).toBe(404);
});

test('A webhook signing credential cannot be deleted while a webhook uses it, and deleting the webhook first frees it', buildBudget, async ({ server }) => {
  const { project, agent } = await startedAgent(server, 'rotating secret', webhookSpell('rotating secret spell'));
  const secret = `whsec-${randomUUID()}`;
  const credential = await server.api('/api/v1/credentials', { body: { project_id: project.id, name: 'rotating secret', provider: 'webhook', secret } });
  const hook = await server.api('/api/v1/webhooks', { body: { agent_id: agent.id, path: 'rotating/hook', secret_credential_id: credential.body.id } });
  expect(hook.status).toBe(201);
  const refused = await server.api(`/api/v1/credentials/${credential.body.id}`, { method: 'DELETE' });
  expect(refused.status).toBe(409);
  expect(refused.body.code).toBe('conflict');
  const unsignedAfterRefusal = await fetch(`${server.baseUrl}/api/v1/webhooks/rotating/hook`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
  expect(unsignedAfterRefusal.status).toBe(401);
  expect((await server.api(`/api/v1/credentials?project_id=${project.id}`)).body.map((item: { id: string }) => item.id)).toEqual([credential.body.id]);
  expect((await server.api(`/api/v1/webhooks/${hook.body.id}`, { method: 'DELETE' })).status).toBe(204);
  expect((await server.api(`/api/v1/webhooks/${hook.body.id}`, { method: 'DELETE' })).status).toBe(404);
  const gone = await fetch(`${server.baseUrl}/api/v1/webhooks/rotating/hook`, { method: 'POST', headers: { 'content-type': 'application/json', ...sign(secret, '{}') }, body: '{}' });
  expect(gone.status).toBe(404);
  expect((await server.api(`/api/v1/credentials/${credential.body.id}`, { method: 'DELETE' })).status).toBe(204);
  expect((await server.api(`/api/v1/credentials?project_id=${project.id}`)).body).toEqual([]);
});

test('Creating a webhook requires a signing credential unless it is explicitly unsigned on a server without an API token', buildBudget, async ({ server }) => {
  const { agent } = await startedAgent(server, 'unsigned policy', webhookSpell('unsigned policy spell'));
  const implicit = await server.api('/api/v1/webhooks', { body: { agent_id: agent.id, path: 'implicit/open' } });
  expect(implicit.status).toBe(400);
  expect(implicit.body.error).toContain('secret_credential_id');
  const explicit = await server.api('/api/v1/webhooks', { body: { agent_id: agent.id, path: 'explicit/open', unsigned: true } });
  expect(explicit.status).toBe(201);
  expect(explicit.body.secret_credential_id).toBeNull();
  const delivered = await fetch(`${server.baseUrl}/api/v1/webhooks/explicit/open`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{"hello":"world"}' });
  expect(delivered.status).toBe(202);
});

test('With MAGICKLALA_API_TOKEN set, an unsigned webhook is refused and a timestamped signed delivery is accepted', buildBudget, async () => {
  await buildCli();
  const token = `tok-${randomUUID()}`;
  const { server, stop } = await launchToken(token);
  try {
    const { project, agent } = await startedAgent(server, 'token webhook', webhookSpell('token webhook spell'));
    const unsigned = await server.api('/api/v1/webhooks', { body: { agent_id: agent.id, path: 'token/open', unsigned: true } });
    expect(unsigned.status).toBe(400);
    expect(unsigned.body.error).toContain('MAGICKLALA_API_TOKEN');
    const secret = `whsec-${randomUUID()}`;
    const credential = await server.api('/api/v1/credentials', { body: { project_id: project.id, name: 'token hook secret', provider: 'webhook', secret } });
    const hook = await server.api('/api/v1/webhooks', { body: { agent_id: agent.id, path: 'token/signed', secret_credential_id: credential.body.id } });
    expect(hook.status).toBe(201);
    const payload = JSON.stringify({ paid: true });
    const accepted = await fetch(`${server.baseUrl}/api/v1/webhooks/token/signed`, { method: 'POST', headers: { 'content-type': 'application/json', ...sign(secret, payload) }, body: payload });
    expect(accepted.status).toBe(202);
    await expect.poll(async () => (await server.api(`/api/v1/agents/${agent.id}/runs`)).body.map((item: { status: string }) => item.status), { timeout: 20_000 }).toEqual(['succeeded']);
  } finally {
    await stop();
  }
});

test('The server rejects a foreign Origin on /ws with 403 and an unknown Host header with 421 while allowlisted hosts still work', buildBudget, async () => {
  await buildCli();
  const { server, stop } = await launchServe({ MAGICKLALA_ALLOWED_HOSTS: 'studio.internal, agents.example:8443' });
  try {
    const wsHttp = `${server.baseUrl}/ws`;
    expect((await rawRequest(wsHttp, { headers: upgradeHeaders({ origin: 'https://evil.example' }) })).status).toBe(403);
    expect((await rawRequest(wsHttp, { headers: upgradeHeaders({ origin: 'http://127.0.0.1:4242' }) })).status).toBe(101);
    expect((await rawRequest(wsHttp, { headers: upgradeHeaders() })).status).toBe(101);
    const health = `${server.baseUrl}/api/v1/health`;
    expect((await rawRequest(health, { headers: { host: 'evil.example' } })).status).toBe(421);
    expect((await rawRequest(`${server.baseUrl}/health`, { headers: { host: 'evil.example:7777' } })).status).toBe(421);
    expect((await rawRequest(health, { headers: { host: 'studio.internal' } })).status).toBe(200);
    expect((await rawRequest(health, { headers: { host: 'agents.example:8443' } })).status).toBe(200);
    expect((await rawRequest(health, { headers: { host: 'localhost:9' } })).status).toBe(200);
    expect((await rawRequest(health, { headers: { host: '[::1]:7777' } })).status).toBe(200);
  } finally {
    await stop();
  }
});

test('magicklala serve refuses a non-loopback bind without MAGICKLALA_API_TOKEN unless --insecure-no-auth is passed', buildBudget, async () => {
  await buildCli();
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-public-'));
  try {
    const port = await freePort();
    const env = { ...keylessEnv(), MAGICKLALA_MASTER_KEY: masterKey, MAGICKLALA_API_TOKEN: '' };
    const refused = await runCli(['serve', '--bind', `0.0.0.0:${port}`, '--database-url', `sqlite://${join(dir, 'public.db')}`], env);
    expect(refused.code).toBe(1);
    expect(refused.stderr).toContain('MAGICKLALA_API_TOKEN');
    expect(refused.stderr).toContain('--insecure-no-auth');
    const child = spawn(binary, ['serve', '--bind', `0.0.0.0:${port}`, '--database-url', `sqlite://${join(dir, 'public.db')}`, '--insecure-no-auth'], { env, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout?.resume();
    child.stderr?.resume();
    try {
      await expect.poll(() => fetch(`http://127.0.0.1:${port}/health`).then((response) => response.status, () => 0), { timeout: 30_000 }).toBe(200);
    } finally {
      await stopProcess(child);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

function memorySpell(name: string) {
  return {
    ...emptySpell(name),
    nodes: [
      { id: 'start', kind: 'event/on_start', position: [0, 0], config: {} },
      { id: 'remember', kind: 'memory/add', position: [280, 0], config: {} },
    ],
    wires: [
      { id: 'w_flow', from_node: 'start', from_socket: 'out', to_node: 'remember', to_socket: 'in' },
      { id: 'w_content', from_node: 'start', from_socket: 'payload', to_node: 'remember', to_socket: 'content' },
    ],
  };
}

function clearMemorySpell(name: string) {
  return {
    ...emptySpell(name),
    nodes: [
      { id: 'start', kind: 'event/on_start', position: [0, 0], config: {} },
      { id: 'forget', kind: 'memory/clear', position: [280, 0], config: {} },
    ],
    wires: [{ id: 'w_flow', from_node: 'start', from_socket: 'out', to_node: 'forget', to_socket: 'in' }],
  };
}

function knowledgeSearchSpell(name: string) {
  return {
    ...emptySpell(name),
    nodes: [
      { id: 'start', kind: 'event/on_start', position: [0, 0], config: {} },
      { id: 'lookup', kind: 'knowledge/search', position: [280, 0], config: {} },
    ],
    wires: [
      { id: 'w_flow', from_node: 'start', from_socket: 'out', to_node: 'lookup', to_socket: 'in' },
      { id: 'w_query', from_node: 'start', from_socket: 'payload', to_node: 'lookup', to_socket: 'query' },
    ],
  };
}

test('A direct spell run reaches memory and knowledge only through an agent of the spell\'s own project, and memory/clear reports how many it removed', buildBudget, async ({ server }) => {
  const home = await server.api('/api/v1/projects', { body: { name: 'memory home' } });
  const away = await server.api('/api/v1/projects', { body: { name: 'memory away' } });
  const homeAgent = await server.api('/api/v1/agents', { body: { project_id: home.body.id, name: 'home agent' } });
  const awayAgent = await server.api('/api/v1/agents', { body: { project_id: away.body.id, name: 'away agent' } });
  expect((await server.api(`/api/v1/agents/${awayAgent.body.id}/memories`, { body: { content: 'away secret plan' } })).status).toBe(201);
  expect((await server.api(`/api/v1/agents/${awayAgent.body.id}/knowledge`, { body: { title: 'Away doc', content: 'away confidential roadmap' } })).status).toBe(201);
  const writer = await server.api('/api/v1/spells', { body: { project_id: home.body.id, name: 'memory writer', graph: memorySpell('memory writer') } });
  const clearer = await server.api('/api/v1/spells', { body: { project_id: home.body.id, name: 'memory clearer', graph: clearMemorySpell('memory clearer') } });
  const reader = await server.api('/api/v1/spells', { body: { project_id: home.body.id, name: 'knowledge reader', graph: knowledgeSearchSpell('knowledge reader') } });

  const unbound = await server.api(`/api/v1/spells/${writer.body.id}/run`, { body: { input: 'unbound note' } });
  expect(unbound.status).toBe(422);
  expect(unbound.body.error).toContain('direct spell run requires agent_id in the same project to access memory or knowledge');
  const foreign = await server.api(`/api/v1/spells/${writer.body.id}/run`, { body: { input: 'foreign note', agent_id: awayAgent.body.id } });
  expect(foreign.status).toBe(403);
  expect(foreign.body.code).toBe('agent_not_in_project');
  const unknown = await server.api(`/api/v1/spells/${writer.body.id}/run`, { body: { input: 'ghost note', agent_id: randomUUID() } });
  expect(unknown.status).toBe(404);
  const foreignRead = await server.api(`/api/v1/spells/${reader.body.id}/run`, { body: { input: 'roadmap', agent_id: awayAgent.body.id } });
  expect(foreignRead.status).toBe(403);
  expect(foreignRead.text).not.toContain('away confidential roadmap');
  const unboundRead = await server.api(`/api/v1/spells/${reader.body.id}/run`, { body: { input: 'roadmap' } });
  expect(unboundRead.status).toBe(422);
  expect(unboundRead.text).not.toContain('away confidential roadmap');

  for (const note of ['home note one', 'home note two']) {
    const bound = await server.api(`/api/v1/spells/${writer.body.id}/run`, { body: { input: note, agent_id: homeAgent.body.id } });
    expect(bound.status).toBe(200);
  }
  const homeMemories = await server.api(`/api/v1/agents/${homeAgent.body.id}/memories/search?q=home%20note&limit=10`);
  expect(homeMemories.body.map((hit: { item: { content: string } }) => hit.item.content).sort()).toEqual(['home note one', 'home note two']);
  const cleared = await server.api(`/api/v1/spells/${clearer.body.id}/run`, { body: { agent_id: homeAgent.body.id } });
  expect(cleared.status).toBe(200);
  expect(cleared.body.summary.outputs['forget.removed']).toEqual({ type: 'int', value: homeMemories.body.length });
  expect((await server.api(`/api/v1/agents/${homeAgent.body.id}/memories/search?q=home%20note`)).body).toEqual([]);
  const awayMemories = await server.api(`/api/v1/agents/${awayAgent.body.id}/memories/search?q=away%20secret`);
  expect(awayMemories.body.map((hit: { item: { content: string } }) => hit.item.content)).toEqual(['away secret plan']);
});

test('PATCH /api/v1/agents/{id} rejects a default spell from another project and clears a stale pinned release when the spell changes', buildBudget, async ({ server }) => {
  const { project, agent, release } = await createAgentWithRelease(server, 'respell', echoSpell('respell first'));
  const foreign = await createAgentWithRelease(server, 'respell foreign');
  const rejected = await server.api(`/api/v1/agents/${agent.id}`, { method: 'PATCH', body: { default_spell_id: foreign.spell.id } });
  expect(rejected.status).toBe(400);
  expect(rejected.body.error).toContain('another project');
  expect((await server.api(`/api/v1/agents/${agent.id}`)).body).toMatchObject({ default_spell_id: expect.any(String), pinned_release_id: release.id });
  const second = await server.api('/api/v1/spells', { body: { project_id: project.id, name: 'respell second', graph: echoSpell('respell second') } });
  const switched = await server.api(`/api/v1/agents/${agent.id}`, { method: 'PATCH', body: { default_spell_id: second.body.id } });
  expect(switched.status).toBe(200);
  expect(switched.body).toMatchObject({ default_spell_id: second.body.id, pinned_release_id: null });
  const start = await server.api(`/api/v1/agents/${agent.id}/start`, { body: {} });
  expect(start.status).toBe(409);
  expect(start.body.code).toBe('no_release');
});

test('Publishing a release rejects a spell whose subgraph reference is missing, from another project or cyclic, and accepts one in the same project', buildBudget, async ({ server }) => {
  const { project, agent } = await createAgentWithRelease(server, 'subgraphs', echoSpell('subgraph parent seed'));
  const foreign = await createAgentWithRelease(server, 'subgraphs foreign', echoSpell('foreign child'));
  const child = echoSpell('subgraph child');
  const childSpell = await server.api('/api/v1/spells', { body: { project_id: project.id, name: 'subgraph child', graph: child } });
  expect(childSpell.status).toBe(201);
  const withSubgraph = (name: string, spellId: string) => ({ ...echoSpell(name), subgraphs: [{ node_id: 'send', spell_id: spellId }] });
  const publish = async (name: string, graph: object) => {
    const spell = await server.api('/api/v1/spells', { body: { project_id: project.id, name, graph } });
    return server.api(`/api/v1/agents/${agent.id}/releases`, { body: { spell_id: spell.body.id } });
  };
  const missing = await publish('missing child', withSubgraph('missing child', randomUUID()));
  expect(missing.status).toBe(400);
  expect(missing.body.error).toContain('cannot be resolved');
  const crossProject = await publish('foreign child ref', withSubgraph('foreign child ref', foreign.spell.graph.id));
  expect(crossProject.status).toBe(400);
  expect(crossProject.body.error).toContain('cannot be resolved');
  const ownId = randomUUID();
  const cyclic = await publish('cyclic', { ...withSubgraph('cyclic', ownId), id: ownId });
  expect(cyclic.status).toBe(400);
  const accepted = await publish('good parent', withSubgraph('good parent', child.id));
  expect(accepted.status).toBe(201);
  expect((await server.api(`/api/v1/agents/${agent.id}`)).body.pinned_release_id).toBe(accepted.body.id);
});