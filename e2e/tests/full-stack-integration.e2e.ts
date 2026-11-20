import { test, expect } from 'e2e';
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer as createNetServer, type AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
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

type Scripted = { prompt: string; authorization: string | undefined; response: ServerResponse };

async function startScriptedLlm(answer: (call: Scripted, calls: Scripted[]) => Promise<void>) {
  const calls: Scripted[] = [];
  const server = createHttpServer(async (request, response) => {
    if (request.method !== 'POST' || request.url !== '/v1/chat/completions') {
      response.writeHead(404).end();
      return;
    }
    const chat = await readChat(request);
    const call = { prompt: userPrompt(chat), authorization: request.headers.authorization, response };
    calls.push(call);
    try {
      await answer(call, calls);
    } catch (error) {
      if (!response.headersSent) response.writeHead(500, { 'content-type': 'application/json' }).end(JSON.stringify({ error: { message: String(error) } }));
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const { port } = server.address() as AddressInfo;
  return {
    baseUrl: `http://127.0.0.1:${port}`,
    calls,
    close: async () => {
      server.closeAllConnections();
      server.close();
      await once(server, 'close');
    },
  };
}

function refuse(response: ServerResponse, message: string) {
  response.writeHead(500, { 'content-type': 'application/json' });
  response.end(JSON.stringify({ error: { message } }));
}

function keylessEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env };
  for (const key of ['MAGICKLALA_MASTER_KEY', 'MAGICKLALA_VAULT_KEY', 'MAGICKLALA_LLM_API_KEY', 'OPENAI_API_KEY', 'MAGICKLALA_API_TOKEN']) delete env[key];
  return env;
}

async function launchStack(llmBaseUrl: string, extraEnv: Record<string, string> = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-stack-'));
  const dbPath = join(dir, 'stack.db');
  const port = await freePort();
  const child = spawn(binary, ['serve', '--bind', `127.0.0.1:${port}`, '--database-url', `sqlite://${dbPath}`], {
    cwd: dir,
    env: { ...keylessEnv(), MAGICKLALA_MASTER_KEY: masterKey, MAGICKLALA_LLM_BASE_URL: llmBaseUrl, ...extraEnv },
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
    await waitHealthy(baseUrl);
  } catch (error) {
    await stop();
    throw error;
  }
  return { baseUrl, wsUrl: `ws://127.0.0.1:${port}/ws`, dbPath, api: client(baseUrl), stop };
}

async function helloLlmGraph(configure: (graph: any) => void = () => {}) {
  const graph = JSON.parse(await readFile(join(repoRoot, 'examples', 'hello_llm.json'), 'utf8'));
  graph.id = randomUUID();
  configure(graph);
  return graph;
}

async function runningLlmAgent(api: ReturnType<typeof client>, projectId: string, label: string, graph: unknown) {
  const spell = await api('/api/v1/spells', { project_id: projectId, name: `${label} spell`, graph });
  expect(spell.status).toBe(201);
  const agent = await api('/api/v1/agents', { project_id: projectId, name: `${label} agent` });
  expect(agent.status).toBe(201);
  expect((await api(`/api/v1/agents/${agent.body.id}/releases`, { spell_id: spell.body.id })).status).toBe(201);
  expect((await api(`/api/v1/agents/${agent.body.id}/start`, {})).body.status.state).toBe('running');
  return { spell: spell.body, agent: agent.body };
}

function storedRun(dbPath: string, runId: string) {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  try {
    return db.prepare('SELECT status, error, spell_id, agent_id FROM execution_runs WHERE run_id = ?').get(runId) as Record<string, string | null> | undefined;
  } finally {
    db.close();
  }
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
test('Overlapping chats on two channels each get their own correlated outcome: the failing run answers 502 with its error while the sibling answers 200', async () => {
  const bothArrived = Promise.withResolvers<void>();
  const failingSettled = Promise.withResolvers<void>();
  const llm = await startScriptedLlm(async (call, calls) => {
    if (calls.length === 2) bothArrived.resolve();
    await bothArrived.promise;
    if (call.prompt.includes('fail')) return refuse(call.response, 'loopback refused this prompt');
    await failingSettled.promise;
    completion(call.response, `loopback answer about ${call.prompt}`);
  });
  const stack = await launchStack(llm.baseUrl);
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Crosstalk' });
    const { agent } = await runningLlmAgent(stack.api, project.body.id, 'crosstalk', await helloLlmGraph());
    const failing = stack.api(`/api/v1/agents/${agent.id}/chat`, { content: 'please fail', channel: 'alpha' }).finally(() => failingSettled.resolve());
    const succeeding = stack.api(`/api/v1/agents/${agent.id}/chat`, { content: 'please succeed', channel: 'beta' });
    const [failed, succeeded] = await Promise.all([failing, succeeding]);
    expect(failed.status).toBe(502);
    expect(failed.body.code).toBe('run_failed');
    expect(failed.body.error).toContain('loopback refused this prompt');
    expect(succeeded.status).toBe(200);
    expect(succeeded.body.reply.content).toBe('loopback answer about please succeed');
    await expect.poll(async () => (await stack.api(`/api/v1/agents/${agent.id}/runs`)).body.length, { timeout: 20_000 }).toBe(2);
    const runs = (await stack.api(`/api/v1/agents/${agent.id}/runs`)).body as { status: string; trace: { channel: string; correlation_id: string } }[];
    expect(Object.fromEntries(runs.map((item) => [item.trace.channel, item.status]))).toEqual({ alpha: 'failed', beta: 'succeeded' });
    expect(runs.find((item) => item.trace.channel === 'beta')?.trace.correlation_id).toBe(succeeded.body.reply.metadata.correlation_id);
  } finally {
    await stack.stop();
    await llm.close();
  }
});

test('An LLM node with credential_id authorizes with its own project\'s sealed secret in spell runs and agent chats and refuses another project\'s credential', async () => {
  const llm = await startScriptedLlm(async (call) => completion(call.response, `loopback answer about ${call.prompt}`));
  const stack = await launchStack(llm.baseUrl);
  try {
    const home = await stack.api('/api/v1/projects', { name: 'Vault home' });
    const away = await stack.api('/api/v1/projects', { name: 'Vault away' });
    const homeSecret = `sk-home-${randomUUID()}`;
    const awaySecret = `sk-away-${randomUUID()}`;
    const homeKey = await stack.api('/api/v1/credentials', { project_id: home.body.id, name: 'llm', provider: 'openai', secret: homeSecret });
    const awayKey = await stack.api('/api/v1/credentials', { project_id: away.body.id, name: 'llm', provider: 'openai', secret: awaySecret });
    expect([homeKey.status, awayKey.status]).toEqual([201, 201]);
    const withCredential = (credentialId: string) =>
      helloLlmGraph((graph) => {
        graph.nodes.find((node: { id: string }) => node.id === 'reply').config.credential_id = credentialId;
        graph.nodes.reverse();
      });
    const homeSpell = await stack.api('/api/v1/spells', { project_id: home.body.id, name: 'Vault spell', graph: await withCredential(homeKey.body.id) });
    const ran = await stack.api(`/api/v1/spells/${homeSpell.body.id}/run`, { input: 'vault spell question' });
    expect(ran.status).toBe(200);
    expect(ran.body.summary.status).toBe('succeeded');
    expect(llm.calls.map((call) => [call.prompt, call.authorization])).toEqual([['vault spell question', `Bearer ${homeSecret}`]]);
    const { agent } = await runningLlmAgent(stack.api, home.body.id, 'vault', await withCredential(homeKey.body.id));
    const chat = await stack.api(`/api/v1/agents/${agent.id}/chat`, { content: 'vault agent question' });
    expect(chat.status).toBe(200);
    expect(chat.body.reply.content).toBe('loopback answer about vault agent question');
    expect(llm.calls.at(-1)?.authorization).toBe(`Bearer ${homeSecret}`);
    const thief = await stack.api('/api/v1/spells', { project_id: home.body.id, name: 'Thief spell', graph: await withCredential(awayKey.body.id) });
    const stolen = await stack.api(`/api/v1/spells/${thief.body.id}/run`, { input: 'cross project question' });
    expect(stolen.status).toBe(422);
    expect(stolen.body.summary.status).toBe('failed');
    expect(stolen.body.error).toContain(`credential ${awayKey.body.id}`);
    expect(llm.calls.map((call) => call.prompt)).not.toContain('cross project question');
    expect(llm.calls.map((call) => call.authorization)).not.toContain(`Bearer ${awaySecret}`);
  } finally {
    await stack.stop();
    await llm.close();
  }
});

test('A failing POST /api/v1/spells/{id}/run answers 422 with the failed summary and records the failed run in SQLite', async () => {
  const llm = await startScriptedLlm(async (call) => refuse(call.response, 'loopback is down for maintenance'));
  const stack = await launchStack(llm.baseUrl);
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Failing runs' });
    const spell = await stack.api('/api/v1/spells', { project_id: project.body.id, name: 'Doomed', graph: await helloLlmGraph() });
    const failed = await stack.api(`/api/v1/spells/${spell.body.id}/run`, { input: 'will this work?' });
    expect(failed.status).toBe(422);
    expect(failed.body.code).toBe('spell_failed');
    expect(failed.body.summary.status).toBe('failed');
    expect(failed.body.error).toContain('loopback is down for maintenance');
    expect(failed.body.trace.at(-1).event).toBe('run_finished');
    expect(storedRun(stack.dbPath, failed.body.summary.run_id)).toEqual({ status: 'failed', error: failed.body.summary.error, spell_id: spell.body.id, agent_id: null });
  } finally {
    await stack.stop();
    await llm.close();
  }
});

test('A spell run that outlives MAGICKLALA_SPELL_RUN_TIMEOUT_MS answers 504 and records a timed_out run in SQLite', async () => {
  const released = Promise.withResolvers<void>();
  const llm = await startScriptedLlm(async (call) => {
    await released.promise;
    completion(call.response, 'too late');
  });
  const stack = await launchStack(llm.baseUrl, { MAGICKLALA_SPELL_RUN_TIMEOUT_MS: '1500' });
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Slow runs' });
    const spell = await stack.api('/api/v1/spells', { project_id: project.body.id, name: 'Slow', graph: await helloLlmGraph() });
    const timedOut = await stack.api(`/api/v1/spells/${spell.body.id}/run`, { input: 'take your time' });
    expect(timedOut.status).toBe(504);
    expect(timedOut.body.code).toBe('run_timeout');
    expect(storedRun(stack.dbPath, timedOut.body.run_id)).toMatchObject({ status: 'timed_out', spell_id: spell.body.id, agent_id: null });
  } finally {
    released.resolve();
    await stack.stop();
    await llm.close();
  }
});

test('A WebSocket chat waiting on a slow run never blocks pings on the same socket and still delivers its reply', async () => {
  const frames: { type: string; reply?: { content: string } }[] = [];
  const promptArrived = Promise.withResolvers<string>();
  const pongSeen = Promise.withResolvers<void>();
  const llm = await startScriptedLlm(async (call) => {
    promptArrived.resolve(call.prompt);
    await pongSeen.promise;
    completion(call.response, `loopback answer about ${call.prompt}`);
  });
  const stack = await launchStack(llm.baseUrl);
  let socket: WebSocket | undefined;
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Socket chat' });
    const { agent } = await runningLlmAgent(stack.api, project.body.id, 'socket', await helloLlmGraph());
    socket = new WebSocket(stack.wsUrl);
    socket.addEventListener('message', (message) => {
      const frame = JSON.parse(String(message.data));
      frames.push(frame);
      if (frame.type === 'pong') pongSeen.resolve();
    });
    await once(socket as unknown as EventTarget, 'open');
    socket.send(JSON.stringify({ type: 'chat', agent_id: agent.id, content: 'slow socket question' }));
    expect(await promptArrived.promise).toBe('slow socket question');
    socket.send(JSON.stringify({ type: 'ping' }));
    await expect.poll(() => frames.some((frame) => frame.type === 'pong'), { timeout: 10_000 }).toBe(true);
    await expect.poll(() => frames.find((frame) => frame.type === 'chat_accepted')?.reply?.content, { timeout: 20_000 }).toBe('loopback answer about slow socket question');
  } finally {
    pongSeen.resolve();
    socket?.close();
    await stack.stop();
    await llm.close();
  }
});
function llmRequestRows(dbPath: string, agentId: string | null) {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  try {
    const rows = agentId === null ? db.prepare('SELECT run_id, agent_id, status FROM llm_requests WHERE agent_id IS NULL').all() : db.prepare('SELECT run_id, agent_id, status FROM llm_requests WHERE agent_id = ?').all(agentId);
    return rows as { run_id: string | null; agent_id: string | null; status: string }[];
  } finally {
    db.close();
  }
}

async function startEmbeddingServer(vector: number[]) {
  const calls: { authorization: string | undefined; body: { model: string; input: string[] } }[] = [];
  const server = createHttpServer(async (request, response) => {
    let body = '';
    for await (const chunk of request) body += chunk;
    if (request.method !== 'POST' || request.url !== '/v1/embeddings') {
      response.writeHead(404).end();
      return;
    }
    const parsed = JSON.parse(body);
    calls.push({ authorization: request.headers.authorization, body: parsed });
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ data: parsed.input.map((_: string, index: number) => ({ index, embedding: vector })), usage: { prompt_tokens: 2 } }));
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const { port } = server.address() as AddressInfo;
  return {
    baseUrl: `http://127.0.0.1:${port}`,
    calls,
    close: async () => {
      server.closeAllConnections();
      server.close();
      await once(server, 'close');
    },
  };
}

test('A spell whose flow loops forever answers 422 with the step limit error and the server stays healthy', async () => {
  const llm = await startScriptedLlm(async (call) => completion(call.response, 'unused'));
  const stack = await launchStack(llm.baseUrl);
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Loops' });
    const graph = {
      schema_version: 1,
      id: randomUUID(),
      name: 'Ping pong',
      description: 'flow loop',
      variables: [],
      nodes: [
        { id: 'start', kind: 'event/on_start', position: [0, 0], config: {} },
        { id: 'always', kind: 'logic/regex_match', position: [0, 200], config: { text: 'go', pattern: 'go' } },
        { id: 'a', kind: 'flow/branch', position: [280, 0], config: {} },
        { id: 'b', kind: 'flow/branch', position: [560, 0], config: {} },
      ],
      wires: [
        { id: 'w_start', from_node: 'start', from_socket: 'out', to_node: 'a', to_socket: 'in' },
        { id: 'w_a_cond', from_node: 'always', from_socket: 'matched', to_node: 'a', to_socket: 'condition' },
        { id: 'w_b_cond', from_node: 'always', from_socket: 'matched', to_node: 'b', to_socket: 'condition' },
        { id: 'w_ab', from_node: 'a', from_socket: 'true', to_node: 'b', to_socket: 'in' },
        { id: 'w_ba', from_node: 'b', from_socket: 'true', to_node: 'a', to_socket: 'in' },
      ],
      subgraphs: [],
    };
    const spell = await stack.api('/api/v1/spells', { project_id: project.body.id, name: 'Ping pong', graph });
    expect(spell.status).toBe(201);
    const looped = await stack.api(`/api/v1/spells/${spell.body.id}/run`, {});
    expect(looped.status).toBe(422);
    expect(looped.body.error).toContain('step limit 10000 exceeded');
    expect(looped.body.trace.at(-1).event).toBe('run_finished');
    expect(looped.body.trace_truncated).toBe(true);
    expect(storedRun(stack.dbPath, looped.body.run_id)).toMatchObject({ status: 'failed', spell_id: spell.body.id });
    expect((await fetch(`${stack.baseUrl}/health`)).status).toBe(200);
    expect((await stack.api('/api/v1/health')).body).toMatchObject({ status: 'ok' });
  } finally {
    await stack.stop();
    await llm.close();
  }
});

test('An ai/embedding node with a vault credential_id embeds through the metered gateway in a direct spell run and an agent chat', async () => {
  const vector = [0.25, -0.5, 0.75];
  const embeddings = await startEmbeddingServer(vector);
  const stack = await launchStack(embeddings.baseUrl);
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Embeddings' });
    const secret = `sk-embed-${randomUUID()}`;
    const key = await stack.api('/api/v1/credentials', { project_id: project.body.id, name: 'embedder', provider: 'openai', secret });
    expect(key.status).toBe(201);
    const graph = (trigger: string) => ({
      schema_version: 1,
      id: randomUUID(),
      name: `Embed on ${trigger}`,
      description: 'embedding',
      variables: [],
      nodes: [
        { id: 'trigger', kind: trigger, position: [0, 0], config: {} },
        { id: 'embed', kind: 'ai/embedding', position: [280, 0], config: { provider: 'openai', model: 'text-embedding-3-small', credential_id: key.body.id } },
        { id: 'show', kind: 'json/stringify', position: [560, 120], config: {} },
        { id: 'send', kind: 'io/send_message', position: [840, 0], config: {} },
      ],
      wires: [
        { id: 'w_flow', from_node: 'trigger', from_socket: 'out', to_node: 'embed', to_socket: 'in' },
        { id: 'w_text', from_node: 'trigger', from_socket: trigger === 'event/on_start' ? 'payload' : 'content', to_node: 'embed', to_socket: 'text' },
        { id: 'w_send', from_node: 'embed', from_socket: 'out', to_node: 'send', to_socket: 'in' },
        { id: 'w_vector', from_node: 'embed', from_socket: 'embedding', to_node: 'show', to_socket: 'value' },
        { id: 'w_content', from_node: 'show', from_socket: 'text', to_node: 'send', to_socket: 'content' },
      ],
      subgraphs: [],
    });
    const spell = await stack.api('/api/v1/spells', { project_id: project.body.id, name: 'Embed spell', graph: graph('event/on_start') });
    const ran = await stack.api(`/api/v1/spells/${spell.body.id}/run`, { input: 'embed this sentence' });
    expect(ran.status).toBe(200);
    expect(ran.body.summary.outputs['embed.embedding']).toEqual({ type: 'embedding', value: vector });
    expect(embeddings.calls.map((call) => [call.body.input, call.authorization])).toEqual([[['embed this sentence'], `Bearer ${secret}`]]);
    expect(llmRequestRows(stack.dbPath, null).map((row) => row.status)).toEqual(['ok']);
    const { agent } = await runningLlmAgent(stack.api, project.body.id, 'embedder', graph('event/on_message'));
    const chat = await stack.api(`/api/v1/agents/${agent.id}/chat`, { content: 'embed my chat' });
    expect(chat.status).toBe(200);
    expect(JSON.parse(chat.body.reply.content)).toEqual(vector);
    expect(embeddings.calls.at(-1)).toEqual({ authorization: `Bearer ${secret}`, body: { model: 'text-embedding-3-small', input: ['embed my chat'] } });
    const runs = (await stack.api(`/api/v1/agents/${agent.id}/runs`)).body;
    expect(llmRequestRows(stack.dbPath, agent.id)).toEqual([{ run_id: runs[0].run_id, agent_id: agent.id, status: 'ok' }]);
  } finally {
    await stack.stop();
    await embeddings.close();
  }
});

test('MAGICKLALA_SPELL_RUN_TIMEOUT_MS also bounds agent chats: a stuck LLM answers 504 within the configured timeout and records a timed_out run', async () => {
  const released = Promise.withResolvers<void>();
  const llm = await startScriptedLlm(async (call) => {
    await released.promise;
    completion(call.response, 'too late');
  });
  const stack = await launchStack(llm.baseUrl, { MAGICKLALA_SPELL_RUN_TIMEOUT_MS: '1500' });
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Slow chats' });
    const { agent } = await runningLlmAgent(stack.api, project.body.id, 'slow chat', await helloLlmGraph());
    const response = await fetch(`${stack.baseUrl}/api/v1/agents/${agent.id}/chat`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: 'are you stuck?' }),
      signal: AbortSignal.timeout(20_000),
    });
    expect(response.status).toBe(504);
    const body = await response.json();
    expect(body.code).toBe('run_timeout');
    expect(body.error).toContain('1500 ms');
    const runs = (await stack.api(`/api/v1/agents/${agent.id}/runs`)).body;
    expect(runs.map((item: { status: string }) => item.status)).toEqual(['timed_out']);
  } finally {
    released.resolve();
    await stack.stop();
    await llm.close();
  }
});

test('Restarting magicklala serve on the same SQLite database boots every enabled agent with a pinned release so chats work without calling start', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-boot-'));
  const databaseUrl = `sqlite://${join(dir, 'boot.db')}`;
  const llm = await startScriptedLlm(async (call) => completion(call.response, `loopback answer about ${call.prompt}`));
  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const api = client(baseUrl);
  let serve = startServe(port, databaseUrl, llm.baseUrl, dir);
  try {
    await waitHealthy(baseUrl);
    const project = await api('/api/v1/projects', { name: 'Boot' });
    const { agent } = await runningLlmAgent(api, project.body.id, 'booted', await helloLlmGraph());
    const idle = await api('/api/v1/agents', { project_id: project.body.id, name: 'never released' });
    expect((await api(`/api/v1/agents/${agent.id}/chat`, { content: 'before restart' })).status).toBe(200);
    await stopProcess(serve.child);
    serve = startServe(port, databaseUrl, llm.baseUrl, dir);
    await waitHealthy(baseUrl);
    await expect.poll(async () => (await api(`/api/v1/agents/${agent.id}`)).body.status, { timeout: 20_000 }).toEqual({ state: 'running' });
    expect((await api(`/api/v1/agents/${idle.body.id}`)).body.status).toEqual({ state: 'stopped' });
    const chat = await api(`/api/v1/agents/${agent.id}/chat`, { content: 'after restart' });
    expect(chat.status).toBe(200);
    expect(chat.body.reply.content).toBe('loopback answer about after restart');
  } finally {
    await stopProcess(serve.child);
    await llm.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('One run id links the live WebSocket trace, GET /api/v1/agents/{id}/runs, the saved trace events and the metered LLM request of an agent chat', async () => {
  const llm = await startScriptedLlm(async (call) => completion(call.response, `loopback answer about ${call.prompt}`));
  const stack = await launchStack(llm.baseUrl);
  let socket: WebSocket | undefined;
  try {
    const project = await stack.api('/api/v1/projects', { name: 'Run ids' });
    const { agent } = await runningLlmAgent(stack.api, project.body.id, 'run ids', await helloLlmGraph());
    const frames: { type: string; agent_id?: string; run_id?: string; event?: { event: string; run_id: string } }[] = [];
    socket = new WebSocket(`${stack.wsUrl}?agent_id=${agent.id}`);
    socket.addEventListener('message', (message) => frames.push(JSON.parse(String(message.data))));
    await once(socket as unknown as EventTarget, 'open');
    const chat = await stack.api(`/api/v1/agents/${agent.id}/chat`, { content: 'which run is this?' });
    expect(chat.status).toBe(200);
    await expect.poll(() => frames.some((frame) => frame.type === 'run_completed'), { timeout: 20_000 }).toBe(true);
    const [run] = (await stack.api(`/api/v1/agents/${agent.id}/runs`)).body;
    expect(run.trace.correlation_id).toBe(chat.body.reply.metadata.correlation_id);
    expect(run.trace.truncated).toBe(false);
    const liveTrace = frames.filter((frame) => frame.type === 'trace').map((frame) => frame.event!);
    expect(new Set(liveTrace.map((event) => event.run_id))).toEqual(new Set([run.run_id]));
    expect(frames.find((frame) => frame.type === 'run_completed')?.run_id).toBe(run.run_id);
    expect(run.trace.events.map((event: { event: string }) => event.event)).toEqual(liveTrace.map((event) => event.event));
    expect(new Set(run.trace.events.map((event: { run_id: string }) => event.run_id))).toEqual(new Set([run.run_id]));
    expect(llmRequestRows(stack.dbPath, agent.id)).toEqual([{ run_id: run.run_id, agent_id: agent.id, status: 'ok' }]);
  } finally {
    socket?.close();
    await stack.stop();
    await llm.close();
  }
});for (const [width, height] of [[960, 640], [1280, 800]]) {
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