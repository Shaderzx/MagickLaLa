import { test as base, expect } from 'e2e';
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const binary = join(repoRoot, 'target', 'debug', 'magicklala');
const buildBudget = { timeout: 900_000 };

type Server = { baseUrl: string; wsUrl: string; api: (path: string, init?: { method?: string; body?: unknown }) => Promise<{ status: number; body: any; text: string }> };

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

function emptySpell(name: string) {
  return { schema_version: 1, id: randomUUID(), name, description: 'loopback spell', variables: [], nodes: [], wires: [], subgraphs: [] };
}

const test = base.extend<{ server: Server }>({
  server: async ({}, use) => {
    await buildCli();
    const port = await freePort();
    const dir = await mkdtemp(join(tmpdir(), 'magicklala-serve-'));
    const child = spawn(binary, ['serve', '--bind', `127.0.0.1:${port}`, '--database-url', `sqlite://${join(dir, 'magicklala.db')}`], { cwd: dir, stdio: ['ignore', 'pipe', 'pipe'] });
    const baseUrl = `http://127.0.0.1:${port}`;
    try {
      await expect
        .poll(() => fetch(`${baseUrl}/health`).then((response) => response.status, () => 0), { timeout: 30_000 })
        .toBe(200);
      const api: Server['api'] = async (path, init = {}) => {
        const response = await fetch(`${baseUrl}${path}`, {
          method: init.method ?? (init.body === undefined ? 'GET' : 'POST'),
          headers: init.body === undefined ? {} : { 'content-type': 'application/json' },
          body: init.body === undefined ? undefined : JSON.stringify(init.body),
        });
        const text = await response.text();
        return { status: response.status, text, body: text.length > 0 && text.trim().startsWith('{') || text.trim().startsWith('[') ? JSON.parse(text) : text };
      };
      await use({ baseUrl, wsUrl: `ws://127.0.0.1:${port}/ws`, api });
    } finally {
      await stopProcess(child);
      await rm(dir, { recursive: true, force: true });
    }
  },
});

async function createAgentWithRelease(server: Server, label: string) {
  const project = await server.api('/api/v1/projects', { body: { name: `${label} project`, description: 'e2e' } });
  expect(project.status).toBe(201);
  const spell = await server.api('/api/v1/spells', { body: { project_id: project.body.id, name: `${label} spell`, graph: emptySpell(`${label} spell`) } });
  expect(spell.status).toBe(201);
  const agent = await server.api('/api/v1/agents', { body: { project_id: project.body.id, name: `${label} agent`, description: 'e2e agent', settings: {} } });
  expect(agent.status).toBe(201);
  const release = await server.api(`/api/v1/agents/${agent.body.id}/releases`, { body: { spell_id: spell.body.id, notes: 'first release' } });
  expect(release.status).toBe(201);
  return { project: project.body, spell: spell.body, agent: agent.body, release: release.body };
}

test('GET /api/v1/openapi.json serves an OpenAPI 3.1 document listing every control plane path', buildBudget, async ({ server }) => {
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
      '/api/v1/copilot/suggest',
    ]),
  );
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