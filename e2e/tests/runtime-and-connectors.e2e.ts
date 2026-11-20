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