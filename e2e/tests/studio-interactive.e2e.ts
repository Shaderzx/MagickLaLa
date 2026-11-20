import { test, expect } from 'e2e';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { DatabaseSync } from 'node:sqlite';
import { inflateSync } from 'node:zlib';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const pngSignature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const catalogCategoryOrder = ['events', 'flow', 'ai', 'memory', 'knowledge', 'io', 'code', 'variables', 'text', 'logic', 'json', 'network', 'time'];
const unreachableLlm = 'http://127.0.0.1:9';

type ExampleSpell = { nodes: { id: string; config: unknown }[]; wires: { id: string }[] };
type RunReport = { status: string; events: string[]; failedNodes: string[]; replies: string[]; error: string | null; validation: string };

type InspectorLayout = {
  width: number;
  height: number;
  editorBottom: number | null;
  panelBottom: number | null;
  editorHeight: number;
  card: { left: number; right: number; top: number; bottom: number };
  field: { left: number; right: number; top: number };
  promptLines: number;
  lineHeight: number;
  padding: number;
};

type Bounds = { key: string; left: number | null; right: number | null; top: number | null; bottom: number | null };

type ProbeReport = {
  registry: { kinds: string[]; appKinds: string[]; paletteKinds: string[]; categories: string[] };
  defaultSpell: { nodes: number; wires: number; titles: string[]; kinds: string[]; registryErrors: string[] };
  toolbar: { width: number; height: number; theme: string; items: Bounds[] }[];
  contextMenu: {
    opened: boolean;
    insideCanvas: boolean;
    items: string[];
    add: { clicked: boolean; before: number; after: number; kind: string | null; closed: boolean; spellNodes: number };
    delete: { selected: string | null; clicked: boolean; before: number; after: number; searchRemains: boolean; spellNodes: number };
    fit: { clicked: boolean; zoomBefore: number; zoomAfter: number; expectedZoom: number };
    dismiss: { reopened: boolean; clicked: boolean; dismissed: boolean };
  };
  spawn: { paletteKind: string; nodesBefore: number; nodesAfter: number; spawnedKind: string };
  drag: { node: string; before: [number, number]; after: [number, number]; pointerDelta: [number, number] };
  validate: { status: string };
  run: RunReport & { succeededNodes: number; activeWires: number };
  preserve: { file: string; canvasWires: number; validation: string; wireIds: string[]; configs: Record<string, unknown> }[];
  edit: { node: string; script: string; config: Record<string, unknown> };
  execution: { webhook: RunReport; invalid: RunReport };
  chat: { message: string; echoReply: string; failingReply: string; failingError: string | null };
  publish: { version: number; graph: unknown; reopened: { versions: number[]; spell: unknown } };
  palette: { categories: string[]; order: string[]; accents: Record<string, number[]> };
  inspector: InspectorLayout;
  inspectorDesktop: InspectorLayout;
  theme: { before: string; after: string };
  renders: { path: string; width: number; height: number }[];
};

function pngFacts(bytes: Buffer) {
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  const chunks: Buffer[] = [];
  let offset = 8;
  while (offset < bytes.length) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString('ascii', offset + 4, offset + 8);
    if (type === 'IDAT') chunks.push(bytes.subarray(offset + 8, offset + 8 + length));
    offset += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(chunks));
  const channels = { 0: 1, 2: 3, 4: 2, 6: 4 }[bytes[25]] ?? 4;
  const stride = width * channels;
  const pixels = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x += 1) {
      const left = x >= channels ? pixels[y * stride + x - channels] : 0;
      const up = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const upLeft = y > 0 && x >= channels ? pixels[(y - 1) * stride + x - channels] : 0;
      const estimate = left + up - upLeft;
      const nearest = [left, up, upLeft].sort((a, b) => Math.abs(estimate - a) - Math.abs(estimate - b))[0];
      const predictor = [0, left, up, (left + up) >> 1, Math.abs(estimate - left) <= Math.abs(estimate - up) && Math.abs(estimate - left) <= Math.abs(estimate - upLeft) ? left : Math.abs(estimate - up) <= Math.abs(estimate - upLeft) ? up : nearest][filter];
      pixels[y * stride + x] = (line[x] + predictor) & 0xff;
    }
  }
  const colors = new Map<number, number>();
  for (let offset = 0; offset < pixels.length; offset += channels) {
    const color = pixels.readUIntBE(offset, Math.min(channels, 3));
    colors.set(color, (colors.get(color) ?? 0) + 1);
  }
  const total = pixels.length / channels;
  const entropy = [...colors.values()].reduce((sum, count) => sum - (count / total) * Math.log2(count / total), 0);
  return { width, height, distinctColors: colors.size, entropy };
}

test('studio probe keeps example configs and dynamic wires, runs spells and chat through the real engine, publishes releases that survive a reopen, and drives palette, drag, context menu, 960px toolbar and dark and light PNGs headlessly', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-probe-'));
  const desktop = join(dir, 'studio-desktop.png');
  const compact = join(dir, 'studio-compact.png');
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const env = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '', MAGICKLALA_LLM_BASE_URL: unreachableLlm, MAGICKLALA_LLM_API_KEY: '', OPENAI_API_KEY: '' };
    const { stdout } = await run('cargo', ['run', '-q', '-p', 'magicklala-studio', '--example', 'studio_probe', '--', desktop, compact, dbPath], {
      cwd: repoRoot,
      env,
      timeout: 900_000,
      maxBuffer: 64 * 1024 * 1024,
    });
    const report = JSON.parse(stdout.trim().split('\n').at(-1) ?? '') as ProbeReport;

    expect(report.defaultSpell.nodes).toBe(5);
    expect(report.defaultSpell.wires).toBeGreaterThanOrEqual(4);
    expect(report.defaultSpell.kinds).toEqual(['event/on_message', 'memory/search', 'knowledge/search', 'ai/generate_text', 'io/send_message']);
    expect(report.defaultSpell.titles).toEqual(['On Message', 'Search Memory', 'Search Knowledge', 'Generate Text', 'Send Message']);
    expect(report.defaultSpell.registryErrors).toEqual([]);

    expect(report.spawn.nodesAfter).toBe(report.spawn.nodesBefore + 1);
    expect(report.spawn.spawnedKind).toBe(report.spawn.paletteKind);

    expect(report.drag.after[0] - report.drag.before[0]).toBeCloseTo(report.drag.pointerDelta[0], 1);
    expect(report.drag.after[1] - report.drag.before[1]).toBeCloseTo(report.drag.pointerDelta[1], 1);

    expect(report.validate.status).toBe('valid');

    expect(report.run.events[0]).toBe('RunStarted');
    expect(report.run.events.at(-1)).toBe('RunFinished');
    expect(report.run.events).toContain('NodeStarted');
    expect(report.run.events).toContain('WireActivated');
    expect(report.run.events).toContain('NodeFinished');
    expect(report.run.events).toContain('NodeFailed');
    expect(report.run.status).toBe('Failed');
    expect(report.run.failedNodes).toEqual(['llm']);
    expect(report.run.error).toContain(unreachableLlm.replace('http://', ''));
    expect(report.run.replies).toEqual([]);
    expect(report.run.succeededNodes).toBeLessThan(report.defaultSpell.nodes);

    for (const name of ['webhook_router.json', 'parallel_research.json']) {
      const example = JSON.parse(await readFile(join(repoRoot, 'examples', name), 'utf8')) as ExampleSpell;
      const loaded = report.preserve.find((entry) => entry.file === name);
      expect(loaded?.validation).toBe('valid');
      expect(loaded?.canvasWires).toBe(example.wires.length);
      expect([...(loaded?.wireIds ?? [])].sort()).toEqual(example.wires.map((wire) => wire.id).sort());
      expect(loaded?.configs).toEqual(Object.fromEntries(example.nodes.map((node) => [node.id, node.config])));
    }
    const router = JSON.parse(await readFile(join(repoRoot, 'examples', 'webhook_router.json'), 'utf8')) as ExampleSpell;
    const classify = router.nodes.find((node) => node.id === report.edit.node)?.config as Record<string, unknown>;
    expect(report.edit.script).not.toBe(classify.script);
    expect(report.edit.config).toEqual({ ...classify, script: report.edit.script });

    expect(report.execution.webhook.status).toBe('Succeeded');
    expect(report.execution.webhook.failedNodes).toEqual([]);
    expect(report.execution.webhook.events).toContain('NodeFinished');
    expect(report.execution.webhook.replies).toEqual(['Order 42 received']);
    expect(report.execution.invalid.status).toBe('Invalid');
    expect(report.execution.invalid.validation).toBe('invalid');
    expect(report.execution.invalid.events).not.toContain('RunFinished');
    expect(report.execution.invalid.error).toContain('nope/unknown');

    expect(report.chat.echoReply).toBe(`Echo: ${report.chat.message}`);
    expect(report.chat.failingError).toContain(unreachableLlm.replace('http://', ''));
    expect(report.chat.failingReply).toContain(report.chat.failingError ?? 'missing error');

    expect((await stat(dbPath)).size).toBeGreaterThan(0);
    const db = new DatabaseSync(dbPath, { readOnly: true });
    const rows = db.prepare('SELECT version, graph FROM spell_releases ORDER BY version').all() as { version: number; graph: string }[];
    db.close();
    expect(rows.map((row) => row.version)).toEqual([report.publish.version]);
    expect(JSON.parse(rows[0].graph)).toEqual(report.publish.graph);
    expect(report.publish.reopened.versions).toEqual([report.publish.version]);
    expect(report.publish.reopened.spell).toEqual(report.publish.graph);

    expect(report.palette.order).toEqual(catalogCategoryOrder);
    expect(report.registry.categories.every((category) => catalogCategoryOrder.includes(category))).toBe(true);
    expect(report.palette.categories).toEqual(catalogCategoryOrder.filter((category) => report.registry.categories.includes(category)));
    expect(new Set(catalogCategoryOrder.map((category) => JSON.stringify(report.palette.accents[category]))).size).toBe(catalogCategoryOrder.length);

    expect(report.inspector).toMatchObject({ width: 960, height: 640 });
    expect(report.inspector.editorBottom).not.toBeNull();
    expect(report.inspector.panelBottom).not.toBeNull();
    expect((report.inspector.panelBottom ?? 0) - (report.inspector.editorBottom ?? 0)).toBeGreaterThanOrEqual(12);
    expect(report.inspector.editorHeight).toBeGreaterThanOrEqual(148);
    expect(report.inspectorDesktop).toMatchObject({ width: 1280, height: 800 });
    expect(report.inspectorDesktop.editorHeight).toBeGreaterThanOrEqual(report.inspectorDesktop.promptLines * report.inspectorDesktop.lineHeight + 2 * report.inspectorDesktop.padding);
    for (const layout of [report.inspector, report.inspectorDesktop]) {
      expect(layout.field.left).toBeGreaterThanOrEqual(layout.card.left + layout.padding);
      expect(layout.field.right).toBeLessThanOrEqual(layout.card.right - layout.padding);
      expect(layout.field.top).toBeGreaterThanOrEqual(layout.card.top + layout.padding);
      expect(layout.card.bottom - layout.card.top).toBe(layout.editorHeight);
    }

    expect(report.theme).toEqual({ before: 'dark', after: 'light' });

    const expectedSizes = new Map([[desktop, [1280, 800]], [compact, [960, 640]]]);
    const canonicalKinds = report.registry.kinds.filter((kind) => !kind.startsWith('core/'));
    expect([...report.registry.appKinds].sort()).toEqual([...canonicalKinds].sort());
    expect([...report.registry.paletteKinds].sort()).toEqual([...canonicalKinds].sort());
    expect(report.registry.paletteKinds).toEqual(expect.arrayContaining(report.defaultSpell.kinds));

    expect(report.toolbar.map((layout) => [layout.width, layout.theme])).toEqual([[960, 'dark'], [960, 'light'], [1280, 'dark'], [1280, 'light']]);
    for (const layout of report.toolbar) {
      const placed = layout.items.filter((item): item is { key: string; left: number; right: number; top: number; bottom: number } => item.left !== null && item.right !== null && item.top !== null && item.bottom !== null);
      expect(placed.map((item) => item.key)).toEqual(layout.items.map((item) => item.key));
      expect(Math.max(...placed.map((item) => item.right))).toBeLessThanOrEqual(layout.width - 8);
      expect(placed.find((item) => item.key === 'zoom-fit')?.right).toBeLessThanOrEqual(layout.width - 8);
      expect(Math.min(...placed.map((item) => item.left))).toBeGreaterThanOrEqual(0);
      for (const item of placed) {
        expect(item.top).toBeGreaterThanOrEqual(0);
        expect(item.bottom).toBeLessThanOrEqual(48);
      }
      const byLeft = [...placed].sort((a, b) => a.left - b.left);
      for (let index = 1; index < byLeft.length; index += 1) expect(byLeft[index].left).toBeGreaterThanOrEqual(byLeft[index - 1].right);
    }

    const menu = report.contextMenu;
    expect(menu.opened).toBe(true);
    expect(menu.insideCanvas).toBe(true);
    expect(menu.items).toEqual(['context-add-ai-generate_text', 'context-delete-node', 'context-fit-view', 'context-dismiss']);
    expect(menu.add).toEqual({ clicked: true, before: report.defaultSpell.nodes, after: report.defaultSpell.nodes + 1, kind: 'ai/generate_text', closed: true, spellNodes: report.defaultSpell.nodes + 1 });
    expect(menu.delete).toEqual({ selected: 'search', clicked: true, before: report.defaultSpell.nodes + 1, after: report.defaultSpell.nodes, searchRemains: false, spellNodes: report.defaultSpell.nodes });
    expect(menu.fit.clicked).toBe(true);
    expect(menu.fit.zoomAfter).not.toBeCloseTo(menu.fit.zoomBefore, 3);
    expect(menu.fit.zoomAfter).toBeCloseTo(menu.fit.expectedZoom, 4);
    expect(menu.dismiss).toEqual({ reopened: true, clicked: true, dismissed: true });

    const lightTwin = (path: string) => path.replace(/\.png$/, '.light.png');
    expect(report.renders.map((render) => render.path).sort()).toEqual([...expectedSizes.keys()].flatMap((path) => [path, lightTwin(path)]).sort());
    for (const [path, [width, height]] of [...expectedSizes].flatMap(([path, size]) => [[path, size], [lightTwin(path), size]] as const)) {
      const bytes = await readFile(path);
      expect([...bytes.subarray(0, 8)]).toEqual(pngSignature);
      expect((await stat(path)).size).toBeGreaterThan(15 * 1024);
      const facts = pngFacts(bytes);
      expect(facts.width / facts.height).toBeCloseTo(width / height, 2);
      expect(facts.width).toBeGreaterThanOrEqual(width);
      expect(facts.distinctColors).toBeGreaterThan(200);
      expect(facts.entropy).toBeGreaterThan(1.5);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
async function studioProbe(args: string[]): Promise<any> {
  const env = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '', MAGICKLALA_LLM_BASE_URL: unreachableLlm, MAGICKLALA_LLM_API_KEY: '', OPENAI_API_KEY: '' };
  const { stdout } = await run('cargo', ['run', '-q', '-p', 'magicklala-studio', '--example', 'studio_probe', '--', ...args], { cwd: repoRoot, env, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
  return JSON.parse(stdout.trim().split('\n').at(-1) ?? '');
}

test('Run and chat start in the background, stream live trace events, and Cancel or the run deadline stop a slow delay spell at once', { timeout: 900_000 }, async () => {
  const delayMs = 30_000;
  const deadlineMs = 400;
  const prompt = Math.floor(delayMs / 10);

  const report = await studioProbe(['cancel-run', String(delayMs), String(deadlineMs)]);

  expect(report.delayNode).toEqual({ kind: 'time/delay', ms: delayMs });
  expect(report.run.runningAfterStart).toBe(true);
  expect(report.run.startMs).toBeLessThan(prompt);
  expect(report.run.toolbarWhileRunning).toEqual({ cancel: true, run: false });
  expect(report.run.liveEvents.slice(0, 2)).toEqual(['RunStarted', 'NodeStarted']);
  expect(report.run.liveEvents).not.toContain('RunFinished');
  expect(report.run.delayStatusWhileRunning).toBe('running');
  expect(report.run.cancelMs).toBeLessThan(prompt);
  expect(report.run.runningAfterCancel).toBe(false);
  expect(report.run.outcome).toBe('Cancelled');
  expect(report.run.lastTrace).toBe('RunCancelled');
  expect(report.run.backgroundStopped).toBe(true);
  expect(report.run.totalMs).toBeLessThan(prompt);
  expect(report.run.toolbarAfterCancel).toEqual({ cancel: false, run: true });

  expect(report.chat.runningAfterSend).toBe(true);
  expect(report.chat.sendMs).toBeLessThan(prompt);
  expect(report.chat.lastBubble).toEqual({ fromUser: true, text: report.chat.message });
  expect(report.chat.inputCleared).toBe(true);
  expect(report.chat.replyAfterCancel).toEqual({ fromUser: false, text: `Run #${report.chat.runNumber} cancelled.` });

  expect(report.deadline.timeoutMs).toBe(deadlineMs);
  expect(report.deadline.outcome).toBe('TimedOut');
  expect(report.deadline.lastTrace).toBe('RunTimedOut');
  expect(report.deadline.error).toBe(`run timed out after ${deadlineMs}ms`);
  expect(report.deadline.running).toBe(false);
  expect(report.deadline.totalMs).toBeGreaterThanOrEqual(deadlineMs);
  expect(report.deadline.totalMs).toBeLessThan(prompt);
});

test('Publishing keeps one stable spell record through a rename, so v1 and v2 both belong to it and survive reopening the SQLite file', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-rename-'));
  const dbPath = join(dir, 'studio.sqlite');
  const renamed = 'Renamed Support Agent';
  try {
    const report = await studioProbe(['publish-renamed', dbPath, renamed]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const releases = db.prepare("SELECT spell_id AS spellId, version, json_extract(graph, '$.name') AS name FROM spell_releases ORDER BY version").all();
    const spells = db.prepare('SELECT id FROM spells').all();
    db.close();

    expect(report.second.spellId).toBe(report.first.spellId);
    expect(report.first.name).not.toBe(renamed);
    expect(report.second.name).toBe(renamed);
    expect(releases).toEqual([
      { spellId: report.first.spellId, version: 1, name: report.first.name },
      { spellId: report.first.spellId, version: 2, name: renamed },
      { spellId: report.first.spellId, version: 3, name: renamed },
    ]);
    expect(spells).toEqual([{ id: report.first.spellId }]);
    expect(report.reopened).toEqual({ versions: [1, 2], spellName: renamed, spellRecordId: report.first.spellId });
    expect(report.third).toEqual({ version: 3, spellId: report.first.spellId, name: renamed });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('a flow/subgraph node placed in Studio points at a spell published to its SQLite file, validates, runs the child through the real engine, and leaves no orphan entry once deleted', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-subgraph-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['subgraph', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const published = db.prepare("SELECT json_extract(graph, '$.id') AS spellId FROM spell_releases ORDER BY version").all() as { spellId: string }[];
    db.close();
    expect(published).toEqual([{ spellId: report.childSpellId }]);

    expect(report.placed).toEqual({ kind: 'flow/subgraph', editor: true, caption: 'SPELL ID' });
    expect(report.pointed.subgraphs).toEqual([{ node_id: report.callNode, spell_id: report.childSpellId, release_version: null }]);
    expect(report.pointed.config).toMatchObject({ spell_id: report.childSpellId });
    expect(report.pointed.validation).toBe('valid');
    expect(report.pointed.errors).toEqual([]);

    expect(report.run.status).toBe('Succeeded');
    expect(report.run.error).toBeNull();
    expect(report.run.failedNodes).toEqual([]);
    expect(report.run.replies).toEqual([`child saw ${report.content}`, `parent heard child saw ${report.content}`]);

    expect(report.missing.validation).toBe('invalid');
    expect(report.missing.errors.join('\n')).toContain(report.missing.spellId);

    expect(report.deleted.nodes).not.toContain(report.callNode);
    expect(report.deleted.subgraphs).toEqual([]);
    expect(report.deleted.validation).toBe('valid');
    expect(report.deleted.errors).toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
