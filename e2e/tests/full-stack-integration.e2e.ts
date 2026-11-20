import { test, expect } from 'e2e';
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer as createNetServer, type AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const binary = join(repoRoot, 'target', 'debug', 'magicklala');
const pngSignature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const masterKey = 'a1'.repeat(32);

type Chat = { stream?: boolean; messages: { role: string; content: string }[] };

async function readChat(request: IncomingMessage): Promise<Chat> {
  let body = '';
  for await (const chunk of request) body += chunk;
  return JSON.parse(body) as Chat;
}

function userPrompt(chat: Chat) {
  return chat.messages.filter((message) => message.role === 'user').map((message) => message.content).join('\n');
}

function completion(response: ServerResponse, content: string) {
  response.writeHead(200, { 'content-type': 'application/json' });
  response.end(JSON.stringify({ model: 'loopback', choices: [{ message: { content }, finish_reason: 'stop' }], usage: { prompt_tokens: 3, completion_tokens: 4 } }));
}

async function startLoopbackLlm(options: { holdUntil: number; streamDeltas: string[]; deltaSeen: (delta: string) => Promise<void>; beforeAnswer?: () => Promise<void> }) {
  const held: { prompt: string; response: ServerResponse }[] = [];
  const completionPrompts: string[] = [];
  const streamPrompts: string[] = [];
  let concurrentPeak = 0;
  const server = createHttpServer(async (request, response) => {
    if (request.method !== 'POST' || request.url !== '/v1/chat/completions') {
      response.writeHead(404).end();
      return;
    }
    const chat = await readChat(request);
    const prompt = userPrompt(chat);
    if (chat.stream) {
      streamPrompts.push(prompt);
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      for (const delta of options.streamDeltas) {
        response.write(`data: ${JSON.stringify({ choices: [{ delta: { content: delta } }] })}\n\n`);
        await options.deltaSeen(delta);
      }
      response.end('data: [DONE]\n\n');
      return;
    }
    completionPrompts.push(prompt);
    held.push({ prompt, response });
    concurrentPeak = Math.max(concurrentPeak, held.length);
    if (held.length < options.holdUntil) return;
    await options.beforeAnswer?.();
    for (const waiting of held.splice(0)) completion(waiting.response, `loopback answer about ${waiting.prompt}`);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const { port } = server.address() as AddressInfo;
  return {
    baseUrl: `http://127.0.0.1:${port}`,
    completionPrompts,
    streamPrompts,
    concurrentPeak: () => concurrentPeak,
    close: async () => {
      server.closeAllConnections();
      server.close();
      await once(server, 'close');
    },
  };
}

async function freePort(): Promise<number> {
  const probe = createNetServer();
  probe.listen(0, '127.0.0.1');
  await once(probe, 'listening');
  const { port } = probe.address() as AddressInfo;
  probe.close();
  await once(probe, 'close');
  return port;
}

async function stopProcess(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit');
  child.kill('SIGTERM');
  await exited;
}

function startServe(port: number, databaseUrl: string, llmBaseUrl: string, cwd: string) {
  const child = spawn(binary, ['serve', '--bind', `127.0.0.1:${port}`, '--database-url', databaseUrl], {
    cwd,
    env: { ...process.env, MAGICKLALA_MASTER_KEY: masterKey, MAGICKLALA_LLM_BASE_URL: llmBaseUrl },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr?.on('data', (chunk) => (stderr += chunk));
  child.stdout?.resume();
  return { child, stderr: () => stderr };
}

function client(baseUrl: string) {
  return async (path: string, body?: unknown) => {
    const response = await fetch(`${baseUrl}${path}`, {
      method: body === undefined ? 'GET' : 'POST',
      headers: body === undefined ? {} : { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    return { status: response.status, body: text ? JSON.parse(text) : null, text };
  };
}

async function waitHealthy(baseUrl: string) {
  await expect.poll(() => fetch(`${baseUrl}/health`).then((response) => response.status, () => 0), { timeout: 30_000 }).toBe(200);
}

test.beforeAll(async () => {
  await run('cargo', ['build', '-q', '-p', 'magicklala-cli'], { cwd: repoRoot, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
});

test('magicklala spell run streams parallel_research through the loopback LLM with both branches in flight at once and trace lines on stderr', async () => {
  const streamDeltas = ['Both ', 'angles ', 'agree.'];
  let stdout = '';
  const deltaSeen = (delta: string) => expect.poll(() => stdout.includes(delta), { timeout: 20_000 }).toBe(true);
  const llm = await startLoopbackLlm({ holdUntil: 2, streamDeltas, deltaSeen });
  try {
    const child = spawn(binary, ['spell', 'run', join(repoRoot, 'examples', 'parallel_research.json'), '--input', 'rust async runtimes', '--stream', '--trace', '--llm-base-url', llm.baseUrl, '--model', 'loopback-model'], {
      cwd: repoRoot,
      env: { ...process.env, MAGICKLALA_LLM_BASE_URL: '' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stderr = '';
    child.stdout?.on('data', (chunk) => (stdout += chunk));
    child.stderr?.on('data', (chunk) => (stderr += chunk));
    const [code] = await once(child, 'exit');
    expect(code).toBe(0);
    expect(llm.concurrentPeak()).toBe(2);
    expect(llm.completionPrompts).toHaveLength(2);
    for (const prompt of llm.completionPrompts) expect(prompt).toContain('rust async runtimes');
    expect(llm.streamPrompts).toHaveLength(1);
    expect(stdout).toContain(streamDeltas.join(''));
    const traceEvents = stderr.split('\n').filter((line) => line.startsWith('{"event"')).map((line) => JSON.parse(line) as { event: string; node_id?: string; delta?: string });
    expect(traceEvents.filter((event) => event.event === 'stream_chunk').map((event) => event.delta)).toEqual(streamDeltas);
    expect(traceEvents.filter((event) => event.event === 'node_started').map((event) => event.node_id)).toEqual(expect.arrayContaining(['research_performance', 'research_ecosystem', 'summarize']));
    expect(traceEvents.at(-1)?.event).toBe('run_finished');
  } finally {
    await llm.close();
  }
});

test('magicklala serve keeps a project, sealed credential, knowledge, spell release and live agent chat in SQLite across a restart', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-fullstack-'));
  const dbPath = join(dir, 'fullstack.db');
  const databaseUrl = `sqlite://${dbPath}`;
  const secret = `sk-loopback-${'z'.repeat(24)}`;
  const llm = await startLoopbackLlm({ holdUntil: 1, streamDeltas: [], deltaSeen: async () => {} });
  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const api = client(baseUrl);
  let serve = startServe(port, databaseUrl, llm.baseUrl, dir);
  let socket: WebSocket | undefined;
  try {
    await waitHealthy(baseUrl);
    expect(serve.stderr()).toContain('store: sqlite');
    const project = await api('/api/v1/projects', { name: 'Full stack' });
    expect(project.status).toBe(201);
    const credential = await api('/api/v1/credentials', { project_id: project.body.id, name: 'openai', provider: 'openai', secret });
    expect(credential.status).toBe(201);
    expect(credential.text).not.toContain(secret);
    expect((await readFile(dbPath)).includes(Buffer.from(secret))).toBe(false);
    const spellGraph = JSON.parse(await readFile(join(repoRoot, 'examples', 'hello_llm.json'), 'utf8'));
    const spell = await api('/api/v1/spells', { project_id: project.body.id, name: 'Hello LLM', graph: spellGraph });
    expect(spell.status).toBe(201);
    expect((await api(`/api/v1/spells/${spell.body.id}/validate`, {})).body).toEqual({ valid: true, errors: [] });
    const agent = await api('/api/v1/agents', { project_id: project.body.id, name: 'Greeter' });
    expect(agent.status).toBe(201);
    const knowledge = await api(`/api/v1/agents/${agent.body.id}/knowledge`, { title: 'Owls', content: 'Barn owls hunt silently at night using sharp hearing.' });
    expect(knowledge.status).toBe(201);
    const memory = await api(`/api/v1/agents/${agent.body.id}/memories`, { content: 'The user prefers short answers.' });
    expect(memory.status).toBe(201);
    const release = await api(`/api/v1/agents/${agent.body.id}/releases`, { spell_id: spell.body.id, notes: 'first' });
    expect(release.status).toBe(201);
    expect((await api(`/api/v1/agents/${agent.body.id}/start`, {})).body.status.state).toBe('running');

    const frames: { type: string; event?: { event: string; node_id?: string }; action?: { content: string } }[] = [];
    socket = new WebSocket(`ws://127.0.0.1:${port}/ws`);
    socket.addEventListener('message', (message) => frames.push(JSON.parse(String(message.data))));
    await once(socket as unknown as EventTarget, 'open');
    const chat = await api(`/api/v1/agents/${agent.body.id}/chat`, { content: 'tell me about owls' });
    expect(chat.status).toBe(200);
    expect(llm.completionPrompts).toEqual(['tell me about owls']);
    expect(chat.body.reply.content).toBe('loopback answer about tell me about owls');
    expect(chat.body.reply.role).toBe('assistant');
    await expect.poll(() => frames.filter((frame) => frame.type === 'trace').map((frame) => frame.event?.event), { timeout: 20_000 }).toEqual(expect.arrayContaining(['run_started', 'node_started', 'run_finished']));
    await expect.poll(() => frames.filter((frame) => frame.type === 'outbound').map((frame) => frame.action?.content), { timeout: 20_000 }).toEqual(['loopback answer about tell me about owls']);
    socket.close();

    await stopProcess(serve.child);
    serve = startServe(port, databaseUrl, llm.baseUrl, dir);
    await waitHealthy(baseUrl);

    expect((await api('/api/v1/projects')).body.map((item: { id: string }) => item.id)).toContain(project.body.id);
    const restored = await api(`/api/v1/agents/${agent.body.id}`);
    expect(restored.body.pinned_release_id).toBe(release.body.id);
    expect((await api(`/api/v1/spells/${spell.body.id}`)).body.graph.name).toBe(spellGraph.name);
    expect((await api(`/api/v1/agents/${agent.body.id}/releases`)).body.map((item: { id: string }) => item.id)).toEqual([release.body.id]);
    const hits = await api(`/api/v1/agents/${agent.body.id}/knowledge/search?q=owls%20hunt%20at%20night`);
    expect(hits.body[0].item.text).toContain('Barn owls hunt silently');
    const credentials = await api(`/api/v1/credentials?project_id=${project.body.id}`);
    expect(credentials.body.map((item: { name: string; provider: string }) => [item.name, item.provider])).toEqual([['openai', 'openai']]);
    expect(credentials.text).not.toContain(secret);
  } finally {
    socket?.close();
    await stopProcess(serve.child);
    await llm.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('two concurrent chats to one running agent on the same channel each get the reply to their own message, correlated end to end', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-concurrent-chat-'));
  const databaseUrl = `sqlite://${join(dir, 'concurrent.db')}`;
  type Frame = { type: string; event?: { content?: string; payload?: { correlation_id?: string } }; action?: { content: string; channel: string; payload?: { correlation_id?: string } } };
  const frames: Frame[] = [];
  const bothInboundSeen = () => expect.poll(() => frames.filter((frame) => frame.type === 'inbound').length, { timeout: 20_000 }).toBe(2);
  const llm = await startLoopbackLlm({ holdUntil: 1, streamDeltas: [], deltaSeen: async () => {}, beforeAnswer: bothInboundSeen });
  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const api = client(baseUrl);
  const serve = startServe(port, databaseUrl, llm.baseUrl, dir);
  let socket: WebSocket | undefined;
  try {
    await waitHealthy(baseUrl);
    const project = await api('/api/v1/projects', { name: 'Concurrent chat' });
    const spellGraph = JSON.parse(await readFile(join(repoRoot, 'examples', 'hello_llm.json'), 'utf8'));
    const spell = await api('/api/v1/spells', { project_id: project.body.id, name: 'Hello LLM', graph: spellGraph });
    const agent = await api('/api/v1/agents', { project_id: project.body.id, name: 'Twin' });
    expect((await api(`/api/v1/agents/${agent.body.id}/releases`, { spell_id: spell.body.id })).status).toBe(201);
    expect((await api(`/api/v1/agents/${agent.body.id}/start`, {})).body.status.state).toBe('running');
    socket = new WebSocket(`ws://127.0.0.1:${port}/ws`);
    socket.addEventListener('message', (message) => frames.push(JSON.parse(String(message.data))));
    await once(socket as unknown as EventTarget, 'open');

    const questions = ['tell me about owls', 'tell me about bats'];
    const answers = await Promise.all(questions.map((content) => api(`/api/v1/agents/${agent.body.id}/chat`, { content, channel: 'shared' })));

    expect(answers.map((answer) => answer.status)).toEqual([200, 200]);
    expect(answers.map((answer) => answer.body.message.content)).toEqual(questions);
    expect(answers.map((answer) => answer.body.reply.content)).toEqual(questions.map((question) => `loopback answer about ${question}`));
    expect(answers.map((answer) => answer.body.reply.metadata.in_reply_to)).toEqual(answers.map((answer) => answer.body.message.id));
    expect([...llm.completionPrompts].sort()).toEqual([...questions].sort());

    await expect.poll(() => frames.filter((frame) => frame.type === 'outbound').length, { timeout: 20_000 }).toBe(2);
    const inboundIds = new Map(frames.filter((frame) => frame.type === 'inbound').map((frame) => [frame.event?.content, frame.event?.payload?.correlation_id]));
    const outboundIds = new Map(frames.filter((frame) => frame.type === 'outbound').map((frame) => [frame.action?.content, frame.action?.payload?.correlation_id]));
    expect(new Set(inboundIds.values()).size).toBe(2);
    for (const question of questions) {
      expect(inboundIds.get(question)).toMatch(/^[0-9a-f-]{36}$/);
      expect(outboundIds.get(`loopback answer about ${question}`)).toBe(inboundIds.get(question));
    }
  } finally {
    socket?.close();
    await stopProcess(serve.child);
    await llm.close();
    await rm(dir, { recursive: true, force: true });
  }
});
for (const [width, height] of [[960, 640], [1280, 800]]) {
  test(`magicklala studio --snap renders a ${width}x${height} PNG with no accessibility warnings`, async () => {
    const dir = await mkdtemp(join(tmpdir(), 'magicklala-snap-size-'));
    const out = join(dir, 'studio.png');
    try {
      const env = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '' };
      const result = await run(binary, ['studio', '--snap', out, '--width', String(width), '--height', String(height)], { cwd: repoRoot, env, timeout: 300_000, maxBuffer: 64 * 1024 * 1024 });
      const bytes = await readFile(out);
      expect([...bytes.subarray(0, 8)]).toEqual(pngSignature);
      expect([bytes.readUInt32BE(16), bytes.readUInt32BE(20)]).toEqual([width, height]);
      expect(bytes.length).toBeGreaterThan(4096);
      expect(result.stderr.toLowerCase()).not.toMatch(/screen reader|accessib|a11y/);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
}