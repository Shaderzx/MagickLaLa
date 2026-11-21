import { test, expect } from 'e2e';
import { execFile } from 'node:child_process';
import { createHmac, randomUUID } from 'node:crypto';
import { createServer, type IncomingMessage, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const probePath = resolve(repoRoot, 'target', 'debug', 'examples', 'runtime_probe');

type CapturedRequest = { method: string; path: string; authorization: string; body: Record<string, unknown> };

let probeBuild: Promise<unknown> | undefined;

function buildProbe(): Promise<unknown> {
  probeBuild ??= run('cargo', ['build', '-q', '-p', 'magicklala-runtime', '--example', 'runtime_probe'], { cwd: repoRoot, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
  return probeBuild;
}

async function probe(args: string[], input?: string): Promise<Record<string, any>> {
  await buildProbe();
  const child = execFile(probePath, args, { cwd: repoRoot, timeout: 60_000, maxBuffer: 16 * 1024 * 1024 });
  const stdout: string[] = [];
  child.stdout?.on('data', (chunk) => stdout.push(String(chunk)));
  const exitCode = await new Promise<number | null>((done) => {
    child.on('close', (code) => done(code));
    child.stdin?.end(input ?? '');
  });
  expect(exitCode).toBe(0);
  return JSON.parse(stdout.join(''));
}

async function startCapturingServer(reply: Record<string, unknown>): Promise<{ server: Server; baseUrl: string; captured: CapturedRequest[] }> {
  const captured: CapturedRequest[] = [];
  const server = createServer(async (request: IncomingMessage, response) => {
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(chunk as Buffer);
    captured.push({
      method: request.method ?? '',
      path: request.url ?? '',
      authorization: String(request.headers.authorization ?? ''),
      body: JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}'),
    });
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(reply));
  });
  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const { port } = server.address() as AddressInfo;
  return { server, baseUrl: `http://127.0.0.1:${port}`, captured };
}

async function closeServer(server: Server): Promise<void> {
  await new Promise<void>((done) => server.close(() => done()));
}

test('TickConnector emits the requested number of scheduled ticks and closes its stream once stopped', { timeout: 900_000 }, async () => {
  const requestedTicks = 3;
  const result = await probe(['tick', '--interval-ms', '25', '--count', String(requestedTicks)]);
  expect(result.ticks).toHaveLength(requestedTicks);
  expect(result.ticks.map((tick: any) => tick.kind)).toEqual(['tick', 'tick', 'tick']);
  expect(result.ticks.map((tick: any) => tick.payload.sequence)).toEqual([1, 2, 3]);
  expect(result.status_after_stop).toEqual({ state: 'stopped' });
  expect(result.stream_closed_after_stop).toBe(true);
});

test('WebhookConnector accepts a correctly signed payload and rejects a forged signature', { timeout: 900_000 }, async () => {
  const secret = `whsec-${randomUUID()}`;
  const body = JSON.stringify({ channel: 'orders', sender: 'shop', content: 'order 42 paid', amount: 42 });
  const goodSignature = `sha256=${createHmac('sha256', secret).update(body).digest('hex')}`;
  const forgedSignature = `sha256=${createHmac('sha256', `${secret}-wrong`).update(body).digest('hex')}`;

  const accepted = await probe(['webhook', '--secret', secret, '--signature', goodSignature], body);
  expect(accepted.accepted).toBe(true);
  expect(accepted.event.kind).toBe('webhook');
  expect(accepted.event.channel).toBe('orders');
  expect(accepted.event.content).toBe('order 42 paid');
  expect(accepted.event.payload).toEqual(JSON.parse(body));

  const rejected = await probe(['webhook', '--secret', secret, '--signature', forgedSignature], body);
  expect(rejected.accepted).toBe(false);
  expect(rejected.error).toContain('signature');
});

test('DiscordConnector posts the message to the channel endpoint with bot authorization', { timeout: 900_000 }, async () => {
  const { server, baseUrl, captured } = await startCapturingServer({ id: '1', channel_id: 'general' });
  try {
    const token = `discord-${randomUUID()}`;
    const result = await probe(['discord', '--base-url', baseUrl, '--token', token, '--channel', '998877', '--content', 'hello discord']);
    expect(result.sent).toBe(true);
    expect(captured).toEqual([{ method: 'POST', path: '/channels/998877/messages', authorization: `Bot ${token}`, body: { content: 'hello discord' } }]);
    expect(JSON.stringify(result)).not.toContain(token);
  } finally {
    await closeServer(server);
  }
});

test('SlackConnector posts chat.postMessage with bearer authorization and the channel payload', { timeout: 900_000 }, async () => {
  const { server, baseUrl, captured } = await startCapturingServer({ ok: true, ts: '1.0' });
  try {
    const token = `xoxb-${randomUUID()}`;
    const result = await probe(['slack', '--base-url', baseUrl, '--token', token, '--channel', 'C0LAB', '--content', 'hello slack']);
    expect(result.sent).toBe(true);
    expect(captured).toEqual([{ method: 'POST', path: '/chat.postMessage', authorization: `Bearer ${token}`, body: { channel: 'C0LAB', text: 'hello slack' } }]);
    expect(JSON.stringify(result)).not.toContain(token);
  } finally {
    await closeServer(server);
  }
});

test('AgentSupervisor replies in order per channel and hot-swaps the release while the agent keeps running', { timeout: 900_000 }, async () => {
  const result = await probe(['supervisor']);
  const repliesOn = (channel: string) => result.outbound.filter((action: any) => action.channel === channel).map((action: any) => action.content);
  expect(repliesOn('alpha')).toEqual(['v1:a1', 'v1:a2', 'v1:a3', 'v2:a4', 'v2:a5']);
  expect(repliesOn('beta')).toEqual(['v1:b1', 'v1:b2', 'v2:b3']);
  expect(result.status_after_swap).toEqual({ state: 'running' });
  expect(result.status_changes).toEqual([{ state: 'running' }]);
  expect(result.release_version).toBe(2);
  expect(result.runs_completed).toBe(8);
  expect(result.trace_runs_finished).toBe(8);
});

test('CopilotEngine applies a multi-op patch and returns a graph that passes validation', { timeout: 900_000 }, async () => {
  const graph = {
    schema_version: 1,
    id: randomUUID(),
    name: 'patch me',
    nodes: [
      { id: 'trigger', kind: 'probe.trigger', position: [0, 0], config: {} },
      { id: 'old', kind: 'probe.reply', position: [200, 0], config: { template: 'old' } },
    ],
    wires: [{ id: 'w-old', from_node: 'trigger', from_socket: 'out', to_node: 'old', to_socket: 'in' }],
  };
  const ops = [
    { op: 'add_node', node: { id: 'reply', kind: 'probe.reply', position: [0, 0], config: { template: 'v1:{content}' } } },
    { op: 'disconnect', wire_id: 'w-old' },
    { op: 'remove_node', node_id: 'old' },
    { op: 'connect', wire: { id: 'w-new', from_node: 'trigger', from_socket: 'out', to_node: 'reply', to_socket: 'in' } },
    { op: 'update_config', node_id: 'reply', config: { template: 'v2:{content}' } },
    { op: 'move_node', node_id: 'reply', position: [320, 40] },
  ];
  const result = await probe(['copilot'], JSON.stringify({ graph, ops }));
  expect(result.accepted).toBe(true);
  expect(result.validation_errors).toEqual([]);
  expect(result.graph.nodes.map((node: any) => node.id)).toEqual(['trigger', 'reply']);
  expect(result.graph.nodes[1]).toEqual({ id: 'reply', kind: 'probe.reply', position: [320, 40], config: { template: 'v2:{content}' } });
  expect(result.graph.wires).toEqual([{ id: 'w-new', from_node: 'trigger', from_socket: 'out', to_node: 'reply', to_socket: 'in' }]);
  expect(result.original_unchanged).toBe(true);

  const broken = await probe(['copilot'], JSON.stringify({ graph, ops: [{ op: 'connect', wire: { id: 'w-bad', from_node: 'trigger', from_socket: 'out', to_node: 'ghost', to_socket: 'in' } }] }));
  expect(broken.accepted).toBe(false);
  expect(broken.validation_errors.join(' ')).toContain('ghost');
});

test('AgentSupervisor correlates a failed and a succeeded overlapping run to their own correlation ids and records both runs', { timeout: 900_000 }, async () => {
  const result = await probe(['correlated-runs']);
  const completion = (correlation: string) => result.completed.filter((event: any) => event.correlation_id === correlation);
  expect(result.completed).toHaveLength(2);
  expect(completion(result.ok_correlation)).toEqual([
    { type: 'run_completed', agent_id: result.agent_id, run_id: expect.any(String), correlation_id: result.ok_correlation, status: 'succeeded', reply: 'hello there', error: null },
  ]);
  const [failed] = completion(result.failing_correlation);
  expect(failed.status).toBe('failed');
  expect(failed.reply).toBeNull();
  expect(failed.error).toContain('boom requested');
  expect(result.outbound).toEqual([{ channel: 'ok-lane', content: 'hello there', correlation_id: result.ok_correlation }]);
  const recorded = result.recorded_runs.map((run: any) => ({ run_id: run.run_id, agent_id: run.agent_id, spell_id: run.spell_id, status: run.status, trigger: run.trigger, finished: run.finished_at !== null }));
  expect(recorded.sort((left: any, right: any) => left.status.localeCompare(right.status))).toEqual([
    { run_id: failed.run_id, agent_id: result.agent_id, spell_id: result.spell_id, status: 'failed', trigger: 'studio', finished: true },
    { run_id: completion(result.ok_correlation)[0].run_id, agent_id: result.agent_id, spell_id: result.spell_id, status: 'succeeded', trigger: 'studio', finished: true },
  ]);
  expect(result.recorded_runs.find((run: any) => run.status === 'failed').error).toBe(failed.error);
  expect(result.active_runs).toBe(0);
});

test('A webhook inbound event fires the event/on_webhook trigger with top-level method, path and body fields', { timeout: 900_000 }, async () => {
  const correlation = `hook-${randomUUID()}`;
  const body = { amount: 42, order: 'A-7' };
  const result = await probe(['webhook-run', '--correlation', correlation], JSON.stringify({ method: 'POST', path: '/hooks/orders', headers: { shop: 'demo' }, query: { live: 'yes' }, body }));
  expect(result.completed).toEqual([
    { type: 'run_completed', agent_id: result.agent_id, run_id: expect.any(String), correlation_id: correlation, status: 'succeeded', reply: 'POST /hooks/orders amount=42 order=A-7 shop=demo live=yes', error: null },
  ]);
});

test('AgentSupervisor times out a stuck run, saves it with a closing timed_out run_finished event, returns active runs to zero, and forget removes the agent completely', { timeout: 900_000 }, async () => {
  const result = await probe(['forget', '--timeout-ms', '200']);
  expect(result.completed).toHaveLength(1);
  expect(result.completed[0].correlation_id).toBe(result.correlation);
  expect(result.completed[0].status).toBe('timed_out');
  expect(result.completed[0].error).toContain('200 ms');
  expect(result.recorded.trace.events.at(0).event).toBe('run_started');
  expect(result.recorded.trace.events.at(-1)).toMatchObject({
    event: 'run_finished',
    run_id: result.completed[0].run_id,
    summary: { run_id: result.completed[0].run_id, status: 'timed_out', error: result.completed[0].error, outputs: {}, replies: [] },
  });
  expect(result.active_runs_after_timeout).toBe(0);
  expect(result.runs_failed).toBe(1);
  expect(result.status_after_forget).toBeNull();
  expect(result.listed_after_forget).toEqual([]);
  expect(result.release_after_forget).toBeNull();
  expect(result.dispatch_after_forget).toBe(`agent ${result.agent_id} is unknown`);
});

test('A saturated channel lane rejects overflow with busy completions while a sibling channel on the same agent still replies', { timeout: 900_000 }, async () => {
  const result = await probe(['busy']);
  const busy = result.completed.filter((event: any) => event.status === 'failed');
  expect(busy.length).toBeGreaterThan(0);
  expect(busy.length + result.queued_on_stalled_lane).toBe(result.dispatched_on_stalled_lane);
  for (const event of busy) {
    expect(event.error).toBe(`channel stalled-lane of agent ${result.agent_id} is busy`);
    expect(result.stalled_correlations).toContain(event.correlation_id);
  }
  expect(new Set(busy.map((event: any) => event.correlation_id)).size).toBe(busy.length);
  const sibling = result.completed.filter((event: any) => event.correlation_id === result.sibling_correlation);
  expect(sibling).toEqual([
    { type: 'run_completed', agent_id: result.agent_id, run_id: expect.any(String), correlation_id: result.sibling_correlation, status: 'succeeded', reply: 'sibling ping', error: null },
  ]);
});

test('WebhookConnector.verify_timestamped accepts an HMAC over timestamp.delivery.body inside the five minute window and rejects stale, future, legacy, re-addressed, missing or malformed deliveries', { timeout: 900_000 }, async () => {
  const secret = `whsec-${randomUUID()}`;
  const body = JSON.stringify({ order: 'A-9', amount: 9 });
  const now = Math.floor(Date.now() / 1000);
  const delivery = randomUUID();
  const hmac = (key: string, message: string) => `sha256=${createHmac('sha256', key).update(message).digest('hex')}`;
  const signAt = (timestamp: number, key = secret, id = delivery) => hmac(key, `${timestamp}.${id}.${body}`);
  const verify = (timestamp: number, signature: string, id: string | null = delivery, mode = 'header') =>
    probe(['webhook-timestamped', '--secret', secret, '--timestamp', String(timestamp), '--now', String(now), '--signature', signature, '--mode', mode, ...(id === null ? [] : ['--delivery', id])], body);
  const charsetError = "X-MagickLaLa-Delivery must be 1 to 128 characters of ASCII letters, digits, '-' or '_'";

  expect(await verify(now - 299, signAt(now - 299))).toEqual({ accepted: true, max_skew_secs: 300, delivery_id: delivery, max_delivery_id_len: 128 });
  const legacy = await verify(now, hmac(secret, `${now}.${body}`));
  expect(legacy.accepted).toBe(false);
  expect(legacy.error).toContain('timestamp.delivery.body');
  const readdressed = await verify(now, signAt(now), randomUUID());
  expect(readdressed.accepted).toBe(false);
  expect(readdressed.error).toContain('signature');
  const missing = await verify(now, signAt(now), null);
  expect(missing).toEqual({ accepted: false, error: 'missing X-MagickLaLa-Delivery header' });
  const overlong = 'd'.repeat(129);
  expect(await verify(now, signAt(now, secret, overlong), overlong)).toEqual({ accepted: false, error: charsetError });
  expect((await verify(now, signAt(now, secret, 'has space'), 'has space')).accepted).toBe(false);
  for (const dotted of ['evil.1', 'a.b']) {
    expect(await verify(now, signAt(now, secret, dotted), dotted)).toEqual({ accepted: false, error: charsetError });
    expect(await verify(now, signAt(now, secret, dotted), dotted, 'direct')).toEqual({ accepted: false, error: charsetError });
  }
  expect(await verify(now, signAt(now, secret, 'Ab-9_z'), 'Ab-9_z', 'direct')).toEqual({ accepted: true, max_skew_secs: 300, delivery_id: 'Ab-9_z', max_delivery_id_len: 128 });
  const stale = await verify(now - 301, signAt(now - 301));
  expect(stale.accepted).toBe(false);
  expect(stale.error).toContain('timestamp');
  const future = await verify(now + 301, signAt(now + 301));
  expect(future.accepted).toBe(false);
  expect(future.error).toContain('timestamp');
  const bodyOnly = await verify(now, `sha256=${createHmac('sha256', secret).update(body).digest('hex')}`);
  expect(bodyOnly.accepted).toBe(false);
  expect(bodyOnly.error).toContain('signature');
  const forged = await verify(now, signAt(now, `${secret}-wrong`));
  expect(forged.accepted).toBe(false);
  expect(forged.error).toContain('signature');
});

test('A channel lane that idles out between bursts still runs every queued event in order instead of cancelling the ones drained after it closes', { timeout: 900_000 }, async () => {
  const count = 300;
  const result = await probe(['lane-drain', '--count', String(count)]);
  expect(result.completed).toHaveLength(count);
  expect(result.completed.filter((event: any) => event.status !== 'succeeded')).toEqual([]);
  expect(result.outbound).toEqual(Array.from({ length: count }, (_, index) => `drain:m${index}`));
  expect(result.runs_rejected).toBe(0);
});

test('An agent past its live channel lane cap answers new channels with a busy completion while channels it already serves keep replying', { timeout: 900_000 }, async () => {
  const result = await probe(['lane-cap', '--cap', '2']);
  const byChannel = (channel: string) => result.completed.filter((event: any) => event.correlation_id === `${channel}-corr`);
  expect(byChannel('c0')).toEqual([expect.objectContaining({ status: 'succeeded', reply: 'cap:c0' })]);
  expect(byChannel('c1')).toEqual([expect.objectContaining({ status: 'succeeded', reply: 'cap:c1' })]);
  expect(byChannel('c2')).toEqual([expect.objectContaining({ status: 'failed', reply: null, error: `agent ${result.agent_id} is busy: 2 channel lanes are already live` })]);
  expect(result.live_lanes).toBe(2);
  expect(result.runs_rejected).toBe(1);
  expect(result.repeat_on_c0).toEqual(expect.objectContaining({ status: 'succeeded', reply: 'cap:c0 again' }));
});

test('Stopping an agent turns every event still queued in its mailbox and channel lanes into a recorded cancelled run with a completion and a closing run_finished event, dropping none', { timeout: 900_000 }, async () => {
  const count = 40;
  const result = await probe(['stop-drain', '--count', String(count)]);
  expect(result.completed.map((event: any) => event.correlation_id).sort()).toEqual([...result.correlations].sort());
  expect(result.completed.every((event: any) => event.status === 'cancelled')).toBe(true);
  expect(result.recorded_runs.map((run: any) => run.trace.correlation_id).sort()).toEqual([...result.correlations].sort());
  expect(result.recorded_runs.every((run: any) => run.status === 'cancelled')).toBe(true);
  for (const run of result.recorded_runs) {
    expect(run.trace.events.at(-1)).toMatchObject({ event: 'run_finished', run_id: run.run_id, summary: { run_id: run.run_id, status: 'cancelled', error: run.error } });
  }
  expect(result.replies.map((reply: any) => reply.status)).toEqual(Array(count).fill('cancelled'));
});

test('An event whose deadline expired while it waited in its lane is recorded as timed out and answered on its reply channel without ever running', { timeout: 900_000 }, async () => {
  const result = await probe(['deadline', '--timeout-ms', '400', '--deadline-ms', '100']);
  const second = result.completed.find((event: any) => event.correlation_id === result.second_correlation);
  expect(second).toEqual(expect.objectContaining({ status: 'timed_out', reply: null }));
  expect(second.error).toContain('deadline expired while queued');
  expect(result.reply).toEqual(expect.objectContaining({ run_id: second.run_id, status: 'timed_out' }));
  expect(result.second_run_started).toBe(false);
  expect(result.recorded_second).toEqual(expect.objectContaining({ run_id: second.run_id, status: 'timed_out', nodes_executed: 0 }));
});

test('A webhook event never fires an on_message trigger: it falls back only to on_start and otherwise fails with no trigger', { timeout: 900_000 }, async () => {
  const result = await probe(['strict-trigger']);
  expect(result.message_only.status).toBe('failed');
  expect(result.message_only.error).toContain('no trigger');
  expect(result.message_only_outbound).toEqual([]);
  expect(result.start_only).toEqual(expect.objectContaining({ status: 'succeeded', reply: 'started by webhook' }));
});

test('An agent whose spell sends a message on every loop iteration delivers every reply in firing order', { timeout: 900_000 }, async () => {
  const result = await probe(['loop-replies']);
  expect(result.outbound).toEqual(['x', 'xx', 'xxx']);
  expect(result.completed).toEqual([expect.objectContaining({ status: 'succeeded', reply: 'x' })]);
});

test('One run id ties together the live trace events, the run completion, the recorded run and the bounded trace saved with it', { timeout: 900_000 }, async () => {
  const result = await probe(['trace-run-id']);
  const [completion] = result.completed;
  expect(completion.status).toBe('succeeded');
  expect(new Set(result.live_run_ids)).toEqual(new Set([completion.run_id]));
  expect(result.recorded.run_id).toBe(completion.run_id);
  expect(result.recorded.trace.correlation_id).toBe(result.correlation);
  expect(result.recorded.trace.truncated).toBe(false);
  expect(result.recorded.trace.events.map((event: any) => event.event)).toEqual(result.live_events);
  expect(new Set(result.recorded.trace.events.map((event: any) => event.run_id))).toEqual(new Set([completion.run_id]));
  expect(result.found_by_correlation).toBe(completion.run_id);
});

test('A run with more trace events than the cap saves a truncated trace of at most 256 events and 65536 bytes that still ends with a slim run_finished', { timeout: 900_000 }, async () => {
  const result = await probe(['trace-cap']);
  expect(result.completed[0].status).toBe('succeeded');
  expect(result.trace_event_cap).toBe(256);
  expect(result.trace_byte_cap).toBe(65_536);
  expect(result.recorded.trace.truncated).toBe(true);
  expect(result.recorded.trace.events.length).toBeGreaterThan(200);
  expect(result.recorded.trace.events.length).toBeLessThanOrEqual(256);
  expect(Buffer.byteLength(JSON.stringify(result.recorded.trace))).toBeLessThanOrEqual(65_536);
  expect(result.recorded.trace.events.at(-1)).toMatchObject({ event: 'run_finished', run_id: result.completed[0].run_id, summary: { status: 'succeeded', outputs: {}, activations: [], replies: [] } });
  expect(result.recorded.trace.events.filter((event: any) => event.event === 'run_finished')).toHaveLength(1);
  expect(result.recorded.trace.correlation_id).toBe(result.correlation);
});

test('Busy rejections are saved through a bounded persistence queue that stop drains before it returns, so every rejected run is in SQLite with a closing run_finished the moment stop finishes', { timeout: 900_000 }, async () => {
  const result = await probe(['busy-persist']);
  expect(result.rejected_run_queue_capacity).toBe(256);
  const busy = result.completed.filter((event: any) => event.status === 'failed');
  expect(busy.length).toBeGreaterThan(0);
  const rejected = result.recorded_after_stop.filter((run: any) => run.status === 'failed');
  expect(rejected.map((run: any) => run.trace.correlation_id).sort()).toEqual(busy.map((event: any) => event.correlation_id).sort());
  expect(new Set(rejected.map((run: any) => run.run_id))).toEqual(new Set(busy.map((event: any) => event.run_id)));
  for (const run of rejected) {
    expect(run.error).toBe(`channel stalled-lane of agent ${result.agent_id} is busy`);
    expect(run.trace.events.at(-1)).toMatchObject({ event: 'run_finished', run_id: run.run_id, summary: { status: 'failed', error: run.error } });
  }
  expect(result.runs_unrecorded).toBe(0);
  expect(result.recorded_after_stop.length).toBe(result.dispatched);
});

test('Closing a trace with the final timed out summary replaces an earlier succeeded run_finished, so the sealed trace holds exactly one run_finished matching the recorded outcome and stays within 65536 bytes even with escaped envelope fields', { timeout: 900_000 }, async () => {
  const result = await probe(['trace-close']);
  expect(result.early_status).toBe('succeeded');
  expect(result.final_status).toBe('timed_out');
  const finished = result.sealed.events.filter((event: any) => event.event === 'run_finished');
  expect(finished).toHaveLength(1);
  expect(result.sealed.events.at(0).event).toBe('run_started');
  expect(result.sealed.events.at(-1)).toMatchObject({ event: 'run_finished', run_id: result.run_id, summary: { run_id: result.run_id, status: result.final_status, error: result.final_error } });
  expect(result.serialized_bytes).toBeLessThanOrEqual(65_536);
  expect(Buffer.byteLength(JSON.stringify(result.sealed), 'utf8')).toBeLessThanOrEqual(65_536);
  expect(Buffer.byteLength(JSON.stringify(result.sealed.correlation_id), 'utf8')).toBeLessThanOrEqual(result.trace_field_cap);
  expect(result.sealed.correlation_id.startsWith('\u0001')).toBe(true);
});