import { test, expect } from 'e2e';
import { spawn, execFile } from 'node:child_process';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import type { AddressInfo } from 'node:net';

const repoRoot = resolve(import.meta.dirname, '..', '..');
const exampleBinary = join(repoRoot, 'target', 'debug', 'examples', 'run_spell');
const execFileAsync = promisify(execFile);

type TraceEvent = { event: string; node_id?: string; delta?: string; error?: string; name?: string; summary?: { status: string; outputs: Record<string, { type: string; value: unknown }> } };
type SpellRun = { code: number | null; events: TraceEvent[]; stdout: string };

async function buildRunSpell() {
  await execFileAsync('cargo', ['build', '-q', '-p', 'magicklala-nodes', '--example', 'run_spell'], { cwd: repoRoot, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
}

function runSpell(args: string[], env: Record<string, string>, onEvent: (event: TraceEvent) => void = () => {}): Promise<SpellRun> {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(exampleBinary, args, { cwd: repoRoot, env: { ...process.env, ...env } });
    const events: TraceEvent[] = [];
    let stdout = '';
    let pending = '';
    child.stdout.on('data', (chunk: Buffer) => {
      stdout += chunk.toString();
      pending += chunk.toString();
      const lines = pending.split('\n');
      pending = lines.pop() ?? '';
      for (const line of lines.filter((candidate) => candidate.trim().startsWith('{'))) {
        const event = JSON.parse(line) as TraceEvent;
        events.push(event);
        onEvent(event);
      }
    });
    child.stderr.on('data', (chunk: Buffer) => process.stderr.write(chunk));
    child.on('error', rejectRun);
    child.on('close', (code) => resolveRun({ code, events, stdout }));
  });
}

async function readBody(request: IncomingMessage) {
  let body = '';
  for await (const chunk of request) body += chunk;
  return JSON.parse(body) as { stream?: boolean; messages: { role: string; content: string }[] };
}

async function startLoopbackLlm(streamDeltas: string[], seenDelta: (delta: string) => Promise<void>) {
  const heldCompletions: { prompt: string; response: ServerResponse }[] = [];
  const completionPrompts: string[] = [];
  const streamPrompts: string[] = [];
  const server = createServer(async (request, response) => {
    if (request.method !== 'POST' || request.url !== '/v1/chat/completions') {
      response.writeHead(404).end();
      return;
    }
    const body = await readBody(request);
    const prompt = body.messages.filter((message) => message.role === 'user').map((message) => message.content).join('\n');
    if (body.stream) {
      streamPrompts.push(prompt);
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      for (const delta of streamDeltas) {
        response.write(`data: ${JSON.stringify({ choices: [{ delta: { content: delta } }] })}\n\n`);
        await seenDelta(delta);
      }
      response.end('data: [DONE]\n\n');
      return;
    }
    completionPrompts.push(prompt);
    heldCompletions.push({ prompt, response });
    if (heldCompletions.length < 2) return;
    for (const held of heldCompletions.splice(0)) {
      held.response.writeHead(200, { 'content-type': 'application/json' });
      held.response.end(JSON.stringify({ model: 'loopback', choices: [{ message: { content: `finding about ${held.prompt}` }, finish_reason: 'stop' }], usage: { prompt_tokens: 3, completion_tokens: 4 } }));
    }
  });
  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const { port } = server.address() as AddressInfo;
  return { baseUrl: `http://127.0.0.1:${port}`, completionPrompts, streamPrompts, close: () => new Promise<void>((done) => server.close(() => done())) };
}

function eventIndex(events: TraceEvent[], kind: string, nodeId: string) {
  return events.findIndex((event) => event.event === kind && event.node_id === nodeId);
}

async function readSpell(name: string) {
  return JSON.parse(await readFile(join(repoRoot, 'examples', name), 'utf8')) as { nodes: { id: string; kind: string; config: Record<string, unknown> }[]; wires: Record<string, string>[] };
}

test.beforeAll(async () => {
  await buildRunSpell();
});

test('parallel_research runs both research branches at the same time, joins them, and streams the summary chunk by chunk', async () => {
  const streamDeltas = ['The two ', 'findings ', 'agree.'];
  const waiters = new Map<string, () => void>();
  const seen = new Set<string>();
  const seenDelta = (delta: string) => new Promise<void>((done) => {
    if (seen.has(delta)) done();
    else waiters.set(delta, done);
  });
  const llm = await startLoopbackLlm(streamDeltas, seenDelta);
  try {
    const spell = await readSpell('parallel_research.json');
    const researchNodes = spell.nodes.filter((node) => node.kind === 'ai/generate_text').map((node) => node.id);
    const joinNode = spell.nodes.find((node) => node.kind === 'flow/wait_for_all')?.id ?? '';
    const sendNode = spell.nodes.find((node) => node.kind === 'io/send_message')?.id ?? '';
    const run = await runSpell([join(repoRoot, 'examples', 'parallel_research.json'), '--payload', JSON.stringify({ content: 'rust async runtimes' })], { MAGICKLALA_LLM_BASE_URL: llm.baseUrl }, (event) => {
      if (event.event !== 'stream_chunk' || !event.delta) return;
      seen.add(event.delta);
      waiters.get(event.delta)?.();
    });

    expect(run.code).toBe(0);
    expect(researchNodes).toHaveLength(2);
    const lastStart = Math.max(...researchNodes.map((id) => eventIndex(run.events, 'node_started', id)));
    const firstFinish = Math.min(...researchNodes.map((id) => eventIndex(run.events, 'node_finished', id)));
    expect(lastStart).toBeGreaterThan(-1);
    expect(lastStart).toBeLessThan(firstFinish);
    const joinFinishes = run.events.filter((event) => event.event === 'node_finished' && event.node_id === joinNode);
    expect(joinFinishes).toHaveLength(1);
    expect(run.events.indexOf(joinFinishes[0])).toBeGreaterThan(Math.max(...researchNodes.map((id) => eventIndex(run.events, 'node_finished', id))));
    expect(run.events.filter((event) => event.event === 'stream_chunk').map((event) => event.delta)).toEqual(streamDeltas);
    expect(llm.completionPrompts).toHaveLength(2);
    for (const prompt of llm.completionPrompts) expect(prompt).toContain('rust async runtimes');
    expect(llm.streamPrompts).toHaveLength(1);
    for (const prompt of llm.completionPrompts) expect(llm.streamPrompts[0]).toContain(`finding about ${prompt}`);
    const finished = run.events.find((event) => event.event === 'run_finished');
    expect(finished?.summary?.status).toBe('succeeded');
    expect(finished?.summary?.outputs[`${sendNode}.sent`]).toEqual({ type: 'string', value: streamDeltas.join('') });
  } finally {
    await llm.close();
  }
});

test('webhook_router routes each webhook through the Rhai script to the matching switch case', async () => {
  const spell = await readSpell('webhook_router.json');
  const switchNode = spell.nodes.find((node) => node.kind === 'flow/switch');
  const cases = (switchNode?.config.cases ?? []) as string[];
  expect(cases).toContain('orders');
  const routed = await runSpell([join(repoRoot, 'examples', 'webhook_router.json'), '--payload', JSON.stringify({ method: 'POST', path: '/hooks/shop', headers: {}, body: { type: 'order', id: 42 } })], {});
  expect(routed.code).toBe(0);
  const ordersWire = spell.wires.find((wire) => wire.from_node === switchNode?.id && wire.from_socket === 'orders');
  const defaultWire = spell.wires.find((wire) => wire.from_node === switchNode?.id && wire.from_socket === 'default');
  expect(eventIndex(routed.events, 'node_finished', ordersWire?.to_node ?? '')).toBeGreaterThan(-1);
  expect(eventIndex(routed.events, 'node_started', defaultWire?.to_node ?? '')).toBe(-1);
  const routedSummary = routed.events.find((event) => event.event === 'run_finished')?.summary;
  expect(routedSummary?.outputs[`${ordersWire?.to_node}.sent`]).toEqual({ type: 'string', value: 'Order 42 received' });

  const unrouted = await runSpell([join(repoRoot, 'examples', 'webhook_router.json'), '--payload', JSON.stringify({ method: 'POST', path: '/hooks/shop', headers: {}, body: { type: 'ping', id: 7 } })], {});
  expect(unrouted.code).toBe(0);
  expect(eventIndex(unrouted.events, 'node_started', ordersWire?.to_node ?? '')).toBe(-1);
  expect(eventIndex(unrouted.events, 'node_finished', defaultWire?.to_node ?? '')).toBeGreaterThan(-1);
});

test('a Rhai script stuck in an infinite loop is stopped by the operation limit and fails the run', async () => {
  const spell = await readSpell('webhook_router.json');
  const scriptNode = spell.nodes.find((node) => node.kind === 'code/rhai_script');
  expect(scriptNode).toBeDefined();
  if (scriptNode) scriptNode.config.script = 'let spins = 0; loop { spins += 1; }';
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-rhai-'));
  try {
    const path = join(dir, 'looping.json');
    await writeFile(path, JSON.stringify(spell));
    const run = await runSpell([path, '--payload', JSON.stringify({ method: 'POST', path: '/hooks/shop', headers: {}, body: { type: 'order', id: 1 } })], {});
    expect(run.code).not.toBe(0);
    const failure = run.events.find((event) => event.event === 'node_failed' && event.node_id === scriptNode?.id);
    expect(failure?.error).toContain('Too many operations');
    expect(run.events.find((event) => event.event === 'run_finished')?.summary?.status).toBe('failed');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('the validator rejects a type-mismatched wire and a value cycle with the exact wire, types and cycle nodes', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-validate-'));
  const node = (id: string, kind: string, config: Record<string, unknown> = {}) => ({ id, kind, position: [0, 0], config });
  const wire = (id: string, from_node: string, from_socket: string, to_node: string, to_socket: string) => ({ id, from_node, from_socket, to_node, to_socket });
  const spell = (wires: ReturnType<typeof wire>[], nodes: ReturnType<typeof node>[]) => ({ schema_version: 1, id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b', name: 'invalid', nodes, wires });
  try {
    const mismatchPath = join(dir, 'mismatch.json');
    await writeFile(mismatchPath, JSON.stringify(spell(
      [wire('w_flow', 'start', 'out', 'gate', 'in'), wire('w_mismatch', 'words', 'text', 'gate', 'condition')],
      [node('start', 'event/on_start'), node('words', 'text/template', { template: 'yes' }), node('gate', 'flow/branch')],
    )));
    const cyclePath = join(dir, 'cycle.json');
    await writeFile(cyclePath, JSON.stringify(spell(
      [wire('w_ab', 'a', 'text', 'b', 'text'), wire('w_ba', 'b', 'text', 'a', 'text')],
      [node('start', 'event/on_start'), node('a', 'text/replace'), node('b', 'text/replace')],
    )));
    const mismatch = await runSpell([mismatchPath, '--validate'], {});
    expect(mismatch.code).toBe(2);
    expect(JSON.parse(mismatch.stdout)).toEqual({ valid: false, errors: [{ kind: 'type_mismatch', detail: { wire: 'w_mismatch', from: 'string', to: 'bool' } }] });
    const cycle = await runSpell([cyclePath, '--validate'], {});
    expect(cycle.code).toBe(2);
    expect(JSON.parse(cycle.stdout)).toEqual({ valid: false, errors: [{ kind: 'value_cycle', detail: ['a', 'b'] }] });
    const valid = await runSpell([join(repoRoot, 'examples', 'hello_llm.json'), '--validate'], {});
    expect(JSON.parse(valid.stdout)).toEqual({ valid: true, errors: [] });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

type SpellNode = { id: string; kind: string; position: number[]; config: Record<string, unknown> };
type SpellWire = { id: string; from_node: string; from_socket: string; to_node: string; to_socket: string };

const spellNode = (id: string, kind: string, config: Record<string, unknown> = {}): SpellNode => ({ id, kind, position: [0, 0], config });
const spellWire = (id: string, from_node: string, from_socket: string, to_node: string, to_socket: string): SpellWire => ({ id, from_node, from_socket, to_node, to_socket });

async function withSpellFile<T>(nodes: SpellNode[], wires: SpellWire[], use: (path: string) => Promise<T>) {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-hardening-'));
  try {
    const path = join(dir, 'spell.json');
    await writeFile(path, JSON.stringify({ schema_version: 1, id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c', name: 'hardening', nodes, wires }));
    return await use(path);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

async function startRecordingServer(respond: (response: ServerResponse) => void) {
  const requests: string[] = [];
  const server = createServer((request, response) => {
    requests.push(`${request.method} ${request.url}`);
    request.resume();
    request.on('end', () => respond(response));
  });
  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const { port } = server.address() as AddressInfo;
  return { baseUrl: `http://127.0.0.1:${port}`, requests, close: () => new Promise<void>((done) => server.close(() => done())) };
}

function failureOf(run: SpellRun, nodeId: string) {
  return run.events.find((event) => event.event === 'node_failed' && event.node_id === nodeId)?.error ?? '';
}

function finalStatus(run: SpellRun) {
  return run.events.find((event) => event.event === 'run_finished')?.summary?.status;
}

test('wiring both outcomes of one branch into wait_for_all is rejected because the join could never complete', async () => {
  const nodes = [spellNode('start', 'event/on_start'), spellNode('check', 'logic/regex_match', { text: 'go', pattern: 'go' }), spellNode('gate', 'flow/branch'), spellNode('join', 'flow/wait_for_all')];
  const wires = [
    spellWire('w_start', 'start', 'out', 'gate', 'in'),
    spellWire('w_condition', 'check', 'matched', 'gate', 'condition'),
    spellWire('w_true', 'gate', 'true', 'join', 'in'),
    spellWire('w_false', 'gate', 'false', 'join', 'in'),
  ];
  await withSpellFile(nodes, wires, async (path) => {
    const validation = await runSpell([path, '--validate'], {});
    expect(validation.code).toBe(2);
    expect(JSON.parse(validation.stdout)).toEqual({ valid: false, errors: [{ kind: 'exclusive_join', detail: { node: 'join', source: 'gate', wires: ['w_true', 'w_false'] } }] });
    const run = await runSpell([path], {});
    expect(run.code).not.toBe(0);
    expect(run.events.some((event) => event.event === 'run_finished' && event.summary?.status === 'succeeded')).toBe(false);
  });
});

test('a wait_for_all that only some branches reach fails the run as an incomplete join instead of succeeding', async () => {
  const nodes = [
    spellNode('start', 'event/on_start'),
    spellNode('fork', 'flow/split'),
    spellNode('never', 'logic/regex_match', { text: 'stop', pattern: 'go' }),
    spellNode('gate', 'flow/branch'),
    spellNode('join', 'flow/wait_for_all'),
    spellNode('after', 'io/send_message', { content: 'joined' }),
  ];
  const wires = [
    spellWire('w_start', 'start', 'out', 'fork', 'in'),
    spellWire('w_direct', 'fork', 'out_0', 'join', 'in'),
    spellWire('w_gate', 'fork', 'out_1', 'gate', 'in'),
    spellWire('w_condition', 'never', 'matched', 'gate', 'condition'),
    spellWire('w_gated', 'gate', 'true', 'join', 'in'),
    spellWire('w_after', 'join', 'out', 'after', 'in'),
  ];
  await withSpellFile(nodes, wires, async (path) => {
    const run = await runSpell([path], {});
    expect(run.code).toBe(1);
    expect(eventIndex(run.events, 'node_started', 'after')).toBe(-1);
    expect(finalStatus(run)).toBe('failed');
    expect(run.events.find((event) => event.event === 'run_finished')?.summary?.error).toContain('join incomplete: node "join" received 1 of 2 branches');
  });
});

test('a while_loop stops at its own max_iterations and fails the run with the configured limit', async () => {
  const nodes = [
    spellNode('start', 'event/on_start'),
    spellNode('always', 'logic/regex_match', { text: 'go', pattern: 'go' }),
    spellNode('loop', 'flow/while_loop', { max_iterations: 3 }),
    spellNode('tick', 'var/set', { name: 'ticks', value: 1 }),
  ];
  const wires = [spellWire('w_start', 'start', 'out', 'loop', 'in'), spellWire('w_condition', 'always', 'matched', 'loop', 'condition'), spellWire('w_body', 'loop', 'body', 'tick', 'in')];
  await withSpellFile(nodes, wires, async (path) => {
    const run = await runSpell([path], {});
    expect(run.code).toBe(1);
    expect(run.events.filter((event) => event.event === 'node_finished' && event.node_id === 'tick').length).toBe(3);
    expect(failureOf(run, 'loop')).toContain('while loop exceeded max_iterations 3');
    expect(finalStatus(run)).toBe('failed');
  });
});

test('a stream that closes before the [DONE] marker fails the run instead of returning a truncated answer', async () => {
  const truncated = await startRecordingServer((response) => {
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    response.end(`data: ${JSON.stringify({ choices: [{ delta: { content: 'half an ans' } }] })}\n\n`);
  });
  try {
    const nodes = [spellNode('start', 'event/on_start'), spellNode('writer', 'ai/stream_text', { prompt: 'write a poem' })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'writer', 'in')], async (path) => {
      const run = await runSpell([path], { MAGICKLALA_LLM_BASE_URL: truncated.baseUrl });
      expect(truncated.requests).toEqual(['POST /v1/chat/completions']);
      expect(run.code).toBe(1);
      expect(failureOf(run, 'writer')).toContain('upstream stream closed before completion marker');
      expect(finalStatus(run)).toBe('failed');
      expect(run.events.find((event) => event.event === 'run_finished')?.summary?.outputs['writer.text']).toBeUndefined();
    });
  } finally {
    await truncated.close();
  }
});

test('net/http_fetch refuses loopback and cloud metadata addresses unless private HTTP is explicitly allowed', async () => {
  const internal = await startRecordingServer((response) => response.writeHead(200, { 'content-type': 'text/plain' }).end('internal secret'));
  try {
    const fetchSpell = (url: string) => [[spellNode('start', 'event/on_start'), spellNode('fetch', 'net/http_fetch', { url, timeout_ms: 5000 })], [spellWire('w_start', 'start', 'out', 'fetch', 'in')]] as const;
    for (const url of [`${internal.baseUrl}/admin`, 'http://169.254.169.254/latest/meta-data/']) {
      const [nodes, wires] = fetchSpell(url);
      await withSpellFile([...nodes], [...wires], async (path) => {
        const run = await runSpell([path], { MAGICKLALA_ALLOW_PRIVATE_HTTP: '0' });
        expect(run.code).toBe(1);
        expect(failureOf(run, 'fetch')).toContain('refuses to connect to private address');
        expect(finalStatus(run)).toBe('failed');
      });
    }
    expect(internal.requests).toEqual([]);
    const [nodes, wires] = fetchSpell(`${internal.baseUrl}/admin`);
    await withSpellFile([...nodes], [...wires], async (path) => {
      const allowed = await runSpell([path], { MAGICKLALA_ALLOW_PRIVATE_HTTP: '1' });
      expect(allowed.code).toBe(0);
      expect(internal.requests).toEqual(['GET /admin']);
      expect(allowed.events.find((event) => event.event === 'run_finished')?.summary?.outputs['fetch.body']).toEqual({ type: 'string', value: 'internal secret' });
    });
  } finally {
    await internal.close();
  }
});

test('an LLM call that names a vault credential fails as invalid config when no resolver is wired, and never sends the env key', async () => {
  const provider = await startRecordingServer((response) => response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ choices: [{ message: { content: 'leaked' } }] })));
  try {
    const nodes = [spellNode('start', 'event/on_start'), spellNode('ask', 'ai/generate_text', { prompt: 'hello', credential_id: 'vault-credential-7' })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'ask', 'in')], async (path) => {
      const run = await runSpell([path], { MAGICKLALA_LLM_BASE_URL: provider.baseUrl, MAGICKLALA_LLM_API_KEY: 'env-key-that-must-not-be-used' });
      expect(run.code).toBe(1);
      expect(failureOf(run, 'ask')).toBe('invalid config: credential vault-credential-7 cannot be resolved: no credential resolver is configured');
      expect(provider.requests).toEqual([]);
      expect(finalStatus(run)).toBe('failed');
    });
  } finally {
    await provider.close();
  }
});