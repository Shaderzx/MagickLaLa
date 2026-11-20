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

type EmittedActivation = { node_id: string; outputs: Record<string, { type: string; value: unknown }> };
type TraceEvent = {
  event: string;
  run_id?: string;
  node_id?: string;
  delta?: string;
  error?: string;
  name?: string;
  summary?: RunSummary;
};
type RunSummary = {
  run_id: string;
  status: string;
  error?: string | null;
  nodes_executed?: number;
  outputs: Record<string, { type: string; value: unknown }>;
  activations?: EmittedActivation[];
  activations_truncated?: boolean;
  replies?: string[];
};
type SpellRun = { code: number | null; signal: NodeJS.Signals | null; events: TraceEvent[]; summary?: RunSummary; stdout: string; stderr: string };

async function buildRunSpell() {
  await execFileAsync('cargo', ['build', '-q', '-p', 'magicklala-nodes', '--example', 'run_spell'], { cwd: repoRoot, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
}

function runSpell(args: string[], env: Record<string, string>, onEvent: (event: TraceEvent) => void = () => {}): Promise<SpellRun> {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(exampleBinary, args, { cwd: repoRoot, env: { ...process.env, ...env } });
    const events: TraceEvent[] = [];
    let returned: RunSummary | undefined;
    let stdout = '';
    let stderr = '';
    let pending = '';
    child.stdout.on('data', (chunk: Buffer) => {
      stdout += chunk.toString();
      pending += chunk.toString();
      const lines = pending.split('\n');
      pending = lines.pop() ?? '';
      for (const line of lines.filter((candidate) => candidate.trim().startsWith('{'))) {
        const parsed = JSON.parse(line) as TraceEvent & { run_summary?: RunSummary };
        if (parsed.run_summary) {
          returned = parsed.run_summary;
          continue;
        }
        events.push(parsed);
        onEvent(parsed);
      }
    });
    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString();
      process.stderr.write(chunk);
    });
    child.on('error', rejectRun);
    child.on('close', (code, signal) => resolveRun({ code, signal, events, summary: returned, stdout, stderr }));
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
    expect(summaryOf(run)?.status).toBe('succeeded');
    expect(summaryOf(run)?.outputs[`${sendNode}.sent`]).toEqual({ type: 'string', value: streamDeltas.join('') });
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
  const routedSummary = summaryOf(routed);
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

async function withSpellFile<T>(nodes: SpellNode[], wires: SpellWire[], use: (path: string) => Promise<T>, extra: Record<string, unknown> = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-hardening-'));
  try {
    const path = join(dir, 'spell.json');
    await writeFile(path, JSON.stringify({ schema_version: 1, id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c', name: 'hardening', nodes, wires, ...extra }));
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
      expect(summaryOf(allowed)?.outputs['fetch.body']).toEqual({ type: 'string', value: 'internal secret' });
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

type RecordedRequest = { method: string; url: string; headers: Record<string, string | string[] | undefined>; body: string };

async function startScriptedServer(respond: (request: RecordedRequest, response: ServerResponse) => void) {
  const requests: RecordedRequest[] = [];
  const server = createServer(async (request, response) => {
    let body = '';
    for await (const chunk of request) body += chunk;
    const recorded = { method: request.method ?? '', url: request.url ?? '', headers: request.headers, body };
    requests.push(recorded);
    respond(recorded, response);
  });
  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const { port } = server.address() as AddressInfo;
  return { baseUrl: `http://127.0.0.1:${port}`, port, requests, close: () => new Promise<void>((done) => server.close(() => done())) };
}

function summaryOf(run: SpellRun) {
  return run.summary ?? run.events.find((event) => event.event === 'run_finished')?.summary;
}

test('a flow cycle between two branches runs until the step limit and fails cleanly instead of overflowing the stack', async () => {
  const nodes = [spellNode('start', 'event/on_start'), spellNode('a', 'flow/branch', { condition: true }), spellNode('b', 'flow/branch', { condition: true })];
  const wires = [spellWire('w_start', 'start', 'out', 'a', 'in'), spellWire('w_ab', 'a', 'true', 'b', 'in'), spellWire('w_ba', 'b', 'true', 'a', 'in')];
  await withSpellFile(nodes, wires, async (path) => {
    const validation = await runSpell([path, '--validate'], {});
    expect(JSON.parse(validation.stdout)).toEqual({ valid: true, errors: [] });
    const run = await runSpell([path], {});
    expect(run.signal).toBeNull();
    expect(run.code).toBe(1);
    expect(run.stderr).toContain('step limit 10000 exceeded');
    expect(finalStatus(run)).toBe('failed');
    expect(summaryOf(run)?.error).toBe('step limit 10000 exceeded');
    expect(run.events.filter((event) => event.event === 'node_finished').length).toBe(10_000);
  });
});

test('a parallel branch cannot read a sibling branch output until both meet at wait_for_all', async () => {
  let releaseHold: () => void = () => {};
  const siblingFinished = new Promise<void>((done) => {
    releaseHold = done;
  });
  const held = await startScriptedServer((_request, response) => {
    siblingFinished.then(() => response.writeHead(200, { 'content-type': 'text/plain' }).end('held'));
  });
  try {
    const nodes = [
      spellNode('start', 'event/on_start'),
      spellNode('fork', 'flow/split'),
      spellNode('setA', 'var/set', { name: 'a', value: 'from-a' }),
      spellNode('hold', 'net/http_fetch', { url: `${held.baseUrl}/hold`, timeout_ms: 20_000 }),
      spellNode('peek', 'json/stringify'),
      spellNode('sendB', 'io/send_message'),
      spellNode('join', 'flow/wait_for_all'),
      spellNode('both', 'text/template', { template: '{{a}}|{{b}}' }),
      spellNode('after', 'io/send_message'),
    ];
    const wires = [
      spellWire('w_start', 'start', 'out', 'fork', 'in'),
      spellWire('w_a', 'fork', 'out_0', 'setA', 'in'),
      spellWire('w_b', 'fork', 'out_1', 'hold', 'in'),
      spellWire('w_hold', 'hold', 'out', 'sendB', 'in'),
      spellWire('w_peek', 'setA', 'value', 'peek', 'value'),
      spellWire('w_peek_text', 'peek', 'text', 'sendB', 'content'),
      spellWire('w_join_a', 'setA', 'out', 'join', 'in'),
      spellWire('w_join_b', 'sendB', 'out', 'join', 'in'),
      spellWire('w_after', 'join', 'out', 'after', 'in'),
      spellWire('w_both_a', 'setA', 'value', 'both', 'a'),
      spellWire('w_both_b', 'hold', 'status', 'both', 'b'),
      spellWire('w_both_text', 'both', 'text', 'after', 'content'),
    ];
    await withSpellFile(nodes, wires, async (path) => {
      const run = await runSpell([path], { MAGICKLALA_ALLOW_PRIVATE_HTTP: '1' }, (event) => {
        if (event.event === 'node_finished' && event.node_id === 'setA') releaseHold();
      });
      expect(run.code).toBe(0);
      expect(held.requests.map((request) => `${request.method} ${request.url}`)).toEqual(['GET /hold']);
      expect(eventIndex(run.events, 'node_finished', 'setA')).toBeLessThan(eventIndex(run.events, 'node_finished', 'hold'));
      const outputs = summaryOf(run)?.outputs ?? {};
      expect(outputs['sendB.sent']).toEqual({ type: 'string', value: 'null' });
      expect(outputs['after.sent']).toEqual({ type: 'string', value: 'from-a|200' });
    });
  } finally {
    await held.close();
  }
});

test('a loop that sends a message three times keeps all three replies in order on the run summary', async () => {
  const nodes = [
    spellNode('start', 'event/on_start'),
    spellNode('count', 'var/get', { name: 'count' }),
    spellNode('under', 'logic/regex_match', { pattern: '^x{0,2}$' }),
    spellNode('loop', 'flow/while_loop', { max_iterations: 10 }),
    spellNode('grow', 'text/template', { template: '{{count}}x' }),
    spellNode('bump', 'var/set', { name: 'count' }),
    spellNode('send', 'io/send_message'),
  ];
  const wires = [
    spellWire('w_start', 'start', 'out', 'loop', 'in'),
    spellWire('w_count', 'count', 'value', 'under', 'text'),
    spellWire('w_condition', 'under', 'matched', 'loop', 'condition'),
    spellWire('w_body', 'loop', 'body', 'bump', 'in'),
    spellWire('w_grow', 'grow', 'text', 'bump', 'value'),
    spellWire('w_send', 'bump', 'out', 'send', 'in'),
    spellWire('w_content', 'bump', 'value', 'send', 'content'),
  ];
  await withSpellFile(nodes, wires, async (path) => {
    const run = await runSpell([path], {});
    expect(run.code).toBe(0);
    const replies = (summaryOf(run)?.activations ?? []).filter((activation) => activation.node_id === 'send').map((activation) => activation.outputs.sent);
    expect(replies).toEqual([
      { type: 'string', value: 'x' },
      { type: 'string', value: 'xx' },
      { type: 'string', value: 'xxx' },
    ]);
    expect(summaryOf(run)?.outputs['send.sent']).toEqual({ type: 'string', value: 'xxx' });
  }, { variables: [{ name: 'count', ty: 'string', default: { type: 'string', value: '' } }] });
});

test('a single streamed event larger than 1 MiB fails the run before it is decoded', async () => {
  const oversized = 'y'.repeat(1_048_576 + 64);
  const provider = await startScriptedServer((_request, response) => {
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    response.end(`data: ${JSON.stringify({ choices: [{ delta: { content: oversized } }] })}\n\ndata: [DONE]\n\n`);
  });
  try {
    const nodes = [spellNode('start', 'event/on_start'), spellNode('writer', 'ai/stream_text', { prompt: 'write a lot' })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'writer', 'in')], async (path) => {
      const run = await runSpell([path], { MAGICKLALA_LLM_BASE_URL: provider.baseUrl });
      expect(provider.requests.map((request) => request.url)).toEqual(['/v1/chat/completions']);
      expect(run.code).toBe(1);
      expect(failureOf(run, 'writer')).toContain('stream frame exceeds 1 MiB');
      expect(run.events.some((event) => event.event === 'stream_chunk')).toBe(false);
      expect(finalStatus(run)).toBe('failed');
    });
  } finally {
    await provider.close();
  }
});

test('a Gemini stream that ends without a finishReason fails, while one that carries it succeeds', async () => {
  let finishReason: string | undefined;
  const provider = await startScriptedServer((_request, response) => {
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    const candidate = { content: { parts: [{ text: 'half of it' }] }, ...(finishReason ? { finishReason } : {}) };
    response.end(`data: ${JSON.stringify({ candidates: [candidate] })}\n\n`);
  });
  try {
    const nodes = [spellNode('start', 'event/on_start'), spellNode('writer', 'ai/stream_text', { provider: 'gemini', model: 'gemini-2.0-flash', prompt: 'write' })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'writer', 'in')], async (path) => {
      const truncated = await runSpell([path], { MAGICKLALA_LLM_BASE_URL: provider.baseUrl });
      expect(provider.requests.map((request) => request.url)).toEqual(['/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse']);
      expect(truncated.code).toBe(1);
      expect(failureOf(truncated, 'writer')).toContain('gemini stream ended without a finishReason');
      expect(summaryOf(truncated)?.outputs['writer.text']).toBeUndefined();
      finishReason = 'STOP';
      const complete = await runSpell([path], { MAGICKLALA_LLM_BASE_URL: provider.baseUrl });
      expect(complete.code).toBe(0);
      expect(summaryOf(complete)?.outputs['writer.text']).toEqual({ type: 'string', value: 'half of it' });
    });
  } finally {
    await provider.close();
  }
});

test('net/http_fetch connects directly and never through an HTTP_PROXY from the environment', async () => {
  const proxy = await startScriptedServer((_request, response) => response.writeHead(200, { 'content-type': 'text/plain' }).end('answered by proxy'));
  try {
    const nodes = [spellNode('start', 'event/on_start'), spellNode('fetch', 'net/http_fetch', { url: 'http://1.1.1.1:9/proxy-probe', timeout_ms: 2000 })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'fetch', 'in')], async (path) => {
      const run = await runSpell([path], { HTTP_PROXY: proxy.baseUrl, http_proxy: proxy.baseUrl, HTTPS_PROXY: proxy.baseUrl, ALL_PROXY: proxy.baseUrl, NO_PROXY: '', no_proxy: '', MAGICKLALA_ALLOW_PRIVATE_HTTP: '0' });
      expect(proxy.requests).toEqual([]);
      expect(summaryOf(run)?.outputs['fetch.body']).toBeUndefined();
    });
  } finally {
    await proxy.close();
  }
});

test('net/http_fetch forwards only accept, accept-language, content-type and user-agent on a cross-origin redirect', async () => {
  const landing = await startScriptedServer((_request, response) => response.writeHead(200, { 'content-type': 'text/plain' }).end('landed'));
  const origin = await startScriptedServer((_request, response) => response.writeHead(302, { location: `${landing.baseUrl}/landing` }).end());
  try {
    const safe = { Accept: 'application/json', 'Accept-Language': 'nl-NL', 'User-Agent': 'magicklala-e2e' };
    const secret = {
      Authorization: 'Bearer top-secret',
      Cookie: 'session=abc',
      'X-Api-Key': 'key-123',
      'Proxy-Authorization': 'Basic cHJveHk=',
      'X-Auth-Token': 'auth-token-1',
      'Api-Key': 'api-key-2',
      'X-Goog-Api-Key': 'goog-key-3',
      'Private-Token': 'gitlab-token-4',
      'X-Trace': 'custom-header-5',
    };
    const headers = { ...safe, ...secret };
    const nodes = [spellNode('start', 'event/on_start'), spellNode('fetch', 'net/http_fetch', { url: `${origin.baseUrl}/start`, headers, timeout_ms: 5000 })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'fetch', 'in')], async (path) => {
      const run = await runSpell([path], { MAGICKLALA_ALLOW_PRIVATE_HTTP: '1' });
      expect(run.code).toBe(0);
      expect(origin.requests.map((request) => request.headers.authorization)).toEqual(['Bearer top-secret']);
      expect(landing.requests.map((request) => request.url)).toEqual(['/landing']);
      expect(origin.requests.map((request) => request.headers['x-goog-api-key'])).toEqual(['goog-key-3']);
      const forwarded = landing.requests[0]?.headers ?? {};
      expect(Object.keys(secret).map((name) => forwarded[name.toLowerCase()])).toEqual(Object.keys(secret).map(() => undefined));
      expect(Object.keys(safe).map((name) => forwarded[name.toLowerCase()])).toEqual(Object.values(safe));
      expect(summaryOf(run)?.outputs['fetch.body']).toEqual({ type: 'string', value: 'landed' });
    });
  } finally {
    await origin.close();
    await landing.close();
  }
});

test('net/http_fetch refuses to replay a request body across origins on a 307 but follows it on the same origin', async () => {
  const landing = await startScriptedServer((_request, response) => response.writeHead(200, { 'content-type': 'text/plain' }).end('landed elsewhere'));
  const origin = await startScriptedServer((request, response) => {
    if (request.url === '/cross') response.writeHead(307, { location: `${landing.baseUrl}/landing` }).end();
    else if (request.url === '/same') response.writeHead(307, { location: '/received' }).end();
    else response.writeHead(200, { 'content-type': 'text/plain' }).end(`received ${request.body}`);
  });
  try {
    const fetchNode = (path: string) => [spellNode('start', 'event/on_start'), spellNode('fetch', 'net/http_fetch', { url: `${origin.baseUrl}${path}`, method: 'POST', body: 'order=42', timeout_ms: 5000 })];
    await withSpellFile(fetchNode('/cross'), [spellWire('w_start', 'start', 'out', 'fetch', 'in')], async (path) => {
      const run = await runSpell([path], { MAGICKLALA_ALLOW_PRIVATE_HTTP: '1' });
      expect(run.code).toBe(1);
      expect(failureOf(run, 'fetch')).toContain('refused cross-origin redirect with request body');
      expect(landing.requests).toEqual([]);
    });
    await withSpellFile(fetchNode('/same'), [spellWire('w_start', 'start', 'out', 'fetch', 'in')], async (path) => {
      const run = await runSpell([path], { MAGICKLALA_ALLOW_PRIVATE_HTTP: '1' });
      expect(run.code).toBe(0);
      expect(summaryOf(run)?.outputs['fetch.body']).toEqual({ type: 'string', value: 'received order=42' });
    });
    expect(origin.requests.map((request) => `${request.method} ${request.url} ${request.body}`)).toEqual(['POST /cross order=42', 'POST /same order=42', 'POST /received order=42']);
  } finally {
    await origin.close();
    await landing.close();
  }
});

test('net/http_fetch refuses NAT64, site-local and IPv4-compatible IPv6 addresses', async () => {
  for (const url of ['http://[64:ff9b::7f00:1]/nat64', 'http://[64:ff9b:1::a00:1]/local-nat64', 'http://[fec0::1]/site-local', 'http://[::7f00:1]/compatible']) {
    const nodes = [spellNode('start', 'event/on_start'), spellNode('fetch', 'net/http_fetch', { url, timeout_ms: 2000 })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'fetch', 'in')], async (path) => {
      const run = await runSpell([path], { MAGICKLALA_ALLOW_PRIVATE_HTTP: '0' });
      expect(run.code).toBe(1);
      expect(failureOf(run, 'fetch')).toContain('refuses to connect to private address');
    });
  }
});

test('a spell that calls a subgraph fails before any node runs when no subgraph resolver is configured', async () => {
  const nodes = [spellNode('start', 'event/on_start'), spellNode('call', 'flow/subgraph'), spellNode('after', 'io/send_message', { content: 'should not run' })];
  const subgraphs = [{ node_id: 'call', spell_id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a99' }];
  await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'call', 'in'), spellWire('w_after', 'call', 'out', 'after', 'in')], async (path) => {
    const run = await runSpell([path], {});
    expect(run.code).toBe(1);
    expect(run.stderr).toContain('SubgraphResolverUnavailable');
    expect(run.stderr).toContain('0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a99');
    expect(run.events).toEqual([]);
  }, { subgraphs });
});

test('ai/embedding sends the vault credential it names and never the environment key', async () => {
  const provider = await startScriptedServer((_request, response) => response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ data: [{ embedding: [0.25, 0.5] }] })));
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-credentials-'));
  try {
    const credentials = join(dir, 'credentials.json');
    await writeFile(credentials, JSON.stringify({ 'vault-embed-1': 'resolved-embedding-secret' }));
    const nodes = [spellNode('start', 'event/on_start'), spellNode('embed', 'ai/embedding', { text: 'hello', model: 'text-embedding-3-small', credential_id: 'vault-embed-1' })];
    await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'embed', 'in')], async (path) => {
      const env = { MAGICKLALA_LLM_BASE_URL: provider.baseUrl, MAGICKLALA_LLM_API_KEY: 'env-key-that-must-not-be-used' };
      const unresolved = await runSpell([path], env);
      expect(unresolved.code).toBe(1);
      expect(failureOf(unresolved, 'embed')).toBe('invalid config: credential vault-embed-1 cannot be resolved: no credential resolver is configured');
      expect(provider.requests).toEqual([]);
      const resolved = await runSpell([path], { ...env, MAGICKLALA_CREDENTIALS_FILE: credentials });
      expect(resolved.code).toBe(0);
      expect(provider.requests.map((request) => [request.url, request.headers.authorization])).toEqual([['/v1/embeddings', 'Bearer resolved-embedding-secret']]);
      expect(JSON.parse(provider.requests[0]?.body ?? '{}')).toEqual({ model: 'text-embedding-3-small', input: ['hello'] });
      expect(summaryOf(resolved)?.outputs['embed.embedding']).toEqual({ type: 'embedding', value: [0.25, 0.5] });
    });
  } finally {
    await rm(dir, { recursive: true, force: true });
    await provider.close();
  }
});

test('a caller-supplied run id is used by the run summary and every trace event', async () => {
  const runId = '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4b01';
  const nodes = [spellNode('start', 'event/on_start'), spellNode('send', 'io/send_message', { content: 'hi' })];
  await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'send', 'in')], async (path) => {
    const run = await runSpell([path, '--run-id', runId], {});
    expect(run.code).toBe(0);
    expect(run.events.length).toBeGreaterThan(4);
    expect([...new Set(run.events.map((event) => event.run_id))]).toEqual([runId]);
    expect(summaryOf(run)?.run_id).toBe(runId);
  });
});
function dataChain(length: number) {
  const links = Array.from({ length }, (_, index) => `link_${index}`);
  const nodes = [
    spellNode('start', 'event/on_start'),
    ...links.map((id, index) => spellNode(id, 'text/template', { template: index === 0 ? 'seed' : '{{text}}' })),
    spellNode('send', 'io/send_message'),
  ];
  const wires = [
    spellWire('w_start', 'start', 'out', 'send', 'in'),
    ...links.slice(1).map((id, index) => spellWire(`w_${id}`, links[index], 'text', id, 'text')),
    spellWire('w_content', links[length - 1], 'text', 'send', 'content'),
  ];
  return { nodes, wires };
}

test('validating a 50,000-node pure data chain finishes without overflowing the stack', async () => {
  const chain = dataChain(50_000);
  await withSpellFile(chain.nodes, chain.wires, async (path) => {
    const validation = await runSpell([path, '--validate'], {});
    expect(validation.signal).toBeNull();
    expect(validation.code).toBe(0);
    expect(JSON.parse(validation.stdout)).toEqual({ valid: true, errors: [] });
  });
});

test('a 3,500-node pure data chain runs without overflowing the stack and every data node counts against the step limit', async () => {
  const chain = dataChain(3_500);
  const steps = chain.nodes.length;
  await withSpellFile(chain.nodes, chain.wires, async (path) => {
    const over = await runSpell([path, '--step-limit', String(steps - 1)], {});
    expect(over.signal).toBeNull();
    expect(over.code).toBe(1);
    expect(summaryOf(over)?.error).toBe(`step limit ${steps - 1} exceeded`);
    expect(eventIndex(over.events, 'node_finished', 'send')).toBe(-1);
    const within = await runSpell([path, '--step-limit', String(steps)], {});
    expect(within.signal).toBeNull();
    expect(within.code).toBe(0);
    expect(summaryOf(within)?.replies).toEqual(['seed']);
    expect(summaryOf(within)?.nodes_executed).toBe(steps);
  });
});

const childSpellId = '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4c01';
const childSpell = {
  schema_version: 1,
  id: childSpellId,
  name: 'child',
  nodes: [spellNode('child_start', 'event/on_start'), spellNode('child_words', 'text/template', { template: 'child saw {{topic}}' }), spellNode('child_send', 'io/send_message')],
  wires: [
    spellWire('cw_flow', 'child_start', 'out', 'child_send', 'in'),
    spellWire('cw_topic', 'child_start', 'payload', 'child_words', 'topic'),
    spellWire('cw_content', 'child_words', 'text', 'child_send', 'content'),
  ],
};

async function withChildSpell<T>(use: (childPath: string) => Promise<T>) {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-child-'));
  try {
    const childPath = join(dir, 'child.json');
    await writeFile(childPath, JSON.stringify(childSpell));
    return await use(childPath);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

test('a flow/subgraph node runs the child spell with its payload, returns its result, outputs and replies, and shares the parent step limit', async () => {
  const nodes = [spellNode('start', 'event/on_start'), spellNode('call', 'flow/subgraph'), spellNode('words', 'text/template', { template: 'parent heard {{result}}' }), spellNode('send', 'io/send_message')];
  const wires = [
    spellWire('w_call', 'start', 'out', 'call', 'in'),
    spellWire('w_payload', 'start', 'payload', 'call', 'payload'),
    spellWire('w_send', 'call', 'out', 'send', 'in'),
    spellWire('w_result', 'call', 'result', 'words', 'result'),
    spellWire('w_content', 'words', 'text', 'send', 'content'),
  ];
  const totalSteps = nodes.length + childSpell.nodes.length;
  await withChildSpell(async (childPath) => {
    await withSpellFile(nodes, wires, async (path) => {
      const run = await runSpell([path, '--subgraph', childPath, '--payload', JSON.stringify('rust'), '--step-limit', String(totalSteps)], {});
      expect(run.code).toBe(0);
      const summary = summaryOf(run);
      expect(summary?.replies).toEqual(['child saw rust', 'parent heard child saw rust']);
      expect(summary?.outputs['call.result']).toEqual({ type: 'string', value: 'child saw rust' });
      expect(summary?.outputs['call.outputs']?.value).toMatchObject({ 'child_send.sent': 'child saw rust', 'child_words.text': 'child saw rust' });
      expect(eventIndex(run.events, 'node_finished', 'child_send')).toBeLessThan(eventIndex(run.events, 'node_finished', 'send'));
      expect(run.events.filter((event) => event.event === 'run_finished')).toHaveLength(1);

      const starved = await runSpell([path, '--subgraph', childPath, '--payload', JSON.stringify('rust'), '--step-limit', String(nodes.length)], {});
      expect(starved.code).toBe(1);
      expect(summaryOf(starved)?.error).toBe(`step limit ${nodes.length} exceeded`);
      expect(eventIndex(starved.events, 'node_finished', 'child_send')).toBe(-1);
      expect(eventIndex(starved.events, 'node_started', 'send')).toBe(-1);
    }, { subgraphs: [{ node_id: 'call', spell_id: childSpellId }] });
  });
});

test('the validator rejects a subgraphs entry that no flow/subgraph node calls and a flow/subgraph node with no target', async () => {
  const nodes = [spellNode('start', 'event/on_start'), spellNode('send', 'io/send_message', { content: 'hi' }), spellNode('call', 'flow/subgraph')];
  const wires = [spellWire('w_send', 'start', 'out', 'send', 'in'), spellWire('w_call', 'send', 'out', 'call', 'in')];
  await withChildSpell(async (childPath) => {
    await withSpellFile(nodes, wires, async (path) => {
      const validation = await runSpell([path, '--validate'], {});
      expect(validation.code).toBe(2);
      expect(JSON.parse(validation.stdout)).toEqual({
        valid: false,
        errors: [
          { kind: 'orphan_subgraph', detail: { node: 'send', spell: childSpellId } },
          { kind: 'subgraph_target_missing', detail: { node: 'call' } },
        ],
      });
      const run = await runSpell([path, '--subgraph', childPath], {});
      expect(run.code).toBe(1);
      expect(run.events).toEqual([]);
    }, { subgraphs: [{ node_id: 'send', spell_id: childSpellId }] });
  });
});

test('a run whose node outputs pass 1 MiB truncates the activation log but keeps every reply in order and sends a compact run_finished event', async () => {
  const bulk = 'z'.repeat(600 * 1024);
  const nodes = [
    spellNode('start', 'event/on_start'),
    spellNode('before', 'io/send_message', { content: 'before the flood' }),
    spellNode('bulk_a', 'var/set', { name: 'a', value: bulk }),
    spellNode('bulk_b', 'var/set', { name: 'b', value: bulk }),
    spellNode('after', 'io/send_message', { content: 'after the flood' }),
  ];
  const wires = [
    spellWire('w_before', 'start', 'out', 'before', 'in'),
    spellWire('w_a', 'before', 'out', 'bulk_a', 'in'),
    spellWire('w_b', 'bulk_a', 'out', 'bulk_b', 'in'),
    spellWire('w_after', 'bulk_b', 'out', 'after', 'in'),
  ];
  await withSpellFile(nodes, wires, async (path) => {
    const run = await runSpell([path], {});
    expect(run.code).toBe(0);
    const summary = summaryOf(run);
    expect(summary?.activations_truncated).toBe(true);
    expect(summary?.replies).toEqual(['before the flood', 'after the flood']);
    expect(summary?.activations?.find((activation) => activation.node_id === 'after')?.outputs).toEqual({});
    const finished = run.events.find((event) => event.event === 'run_finished');
    expect(finished?.summary).toMatchObject({ run_id: summary?.run_id, status: 'succeeded', activations_truncated: true, outputs: {}, activations: [], replies: [] });
    expect(JSON.stringify(finished).length).toBeLessThan(2_048);
  });
});

test('an OpenAI [DONE] marker does not end a Gemini or Anthropic stream that never sent its own completion marker', async () => {
  const provider = await startScriptedServer((request, response) => {
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    const chunk = request.url.includes('gemini') ? { candidates: [{ content: { parts: [{ text: 'half of it' }] } }] } : { type: 'content_block_delta', delta: { type: 'text_delta', text: 'half of it' } };
    response.end(`data: ${JSON.stringify(chunk)}\n\ndata: [DONE]\n\n`);
  });
  try {
    const cases = [
      { provider: 'gemini', model: 'gemini-2.0-flash', failure: 'gemini stream ended without a finishReason' },
      { provider: 'anthropic', model: 'claude-haiku-5-5', failure: 'anthropic stream sent [DONE] instead of message_stop' },
    ];
    for (const { provider: name, model, failure } of cases) {
      const nodes = [spellNode('start', 'event/on_start'), spellNode('writer', 'ai/stream_text', { provider: name, model, prompt: 'write' })];
      await withSpellFile(nodes, [spellWire('w_start', 'start', 'out', 'writer', 'in')], async (path) => {
        const run = await runSpell([path], { MAGICKLALA_LLM_BASE_URL: provider.baseUrl });
        expect(run.code).toBe(1);
        expect(failureOf(run, 'writer')).toContain(failure);
        expect(summaryOf(run)?.outputs['writer.text']).toBeUndefined();
      });
    }
    expect(provider.requests.map((request) => request.url)).toEqual(['/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse', '/v1/messages']);
  } finally {
    await provider.close();
  }
});
