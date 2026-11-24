import { test, expect } from 'e2e';
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
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
    onCard: { items: string[]; duplicateClicked: boolean; duplicated: string[]; closed: boolean };
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

test('studio probe keeps example configs and dynamic wires, runs spells and chat through the real engine, publishes releases that survive a reopen, and drives palette, drag, a context menu that adds on empty canvas and duplicates or removes the card it was opened on, 960px toolbar and dark and light PNGs headlessly', { timeout: 900_000 }, async () => {
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

    expect(report.theme).toEqual({ before: 'light', after: 'dark' });

    const expectedSizes = new Map([[desktop, [1280, 800]], [compact, [960, 640]]]);
    const canonicalKinds = report.registry.kinds.filter((kind) => !kind.startsWith('core/'));
    expect([...report.registry.appKinds].sort()).toEqual([...canonicalKinds].sort());
    expect([...report.registry.paletteKinds].sort()).toEqual([...canonicalKinds].sort());
    expect(report.registry.paletteKinds).toEqual(expect.arrayContaining(report.defaultSpell.kinds));

    expect(report.toolbar.map((layout) => [layout.width, layout.theme])).toEqual([[960, 'light'], [960, 'dark'], [1280, 'light'], [1280, 'dark']]);
    for (const layout of report.toolbar) {
      expect(layout.items.map((item) => item.key)).toEqual(['toolbar-left', 'validation-pill', 'validate', 'run-spell', 'save', 'publish', 'theme-toggle', 'zoom-out', 'zoom-readout', 'zoom-in', 'zoom-fit']);
      const placed = layout.items.filter((item): item is { key: string; left: number; right: number; top: number; bottom: number } => item.left !== null && item.right !== null && item.top !== null && item.bottom !== null);
      expect(placed.map((item) => item.key)).toEqual(layout.items.map((item) => item.key));
      expect(Math.max(...placed.map((item) => item.right))).toBeLessThanOrEqual(layout.width - 8);
      expect(placed.find((item) => item.key === 'zoom-fit')?.right).toBeLessThanOrEqual(layout.width - 8);
      expect(Math.min(...placed.map((item) => item.left))).toBeGreaterThanOrEqual(0);
      for (const item of placed) {
        expect(item.top).toBeGreaterThanOrEqual(0);
        expect(item.bottom).toBeLessThanOrEqual(52);
      }
      const byLeft = [...placed].sort((a, b) => a.left - b.left);
      for (let index = 1; index < byLeft.length; index += 1) expect(byLeft[index].left).toBeGreaterThanOrEqual(byLeft[index - 1].right);
    }

    const menu = report.contextMenu;
    expect(menu.opened).toBe(true);
    expect(menu.insideCanvas).toBe(true);
    expect(menu.items).toEqual(['context-add-ai-generate_text', 'context-fit-view', 'context-dismiss']);
    expect(menu.add).toEqual({ clicked: true, before: report.defaultSpell.nodes, after: report.defaultSpell.nodes + 1, kind: 'ai/generate_text', closed: true, spellNodes: report.defaultSpell.nodes + 1 });
    expect(menu.onCard).toEqual({ items: ['context-duplicate-node', 'context-delete-node', 'context-dismiss'], duplicateClicked: true, duplicated: ['knowledge/search'], closed: true });
    expect(menu.delete).toEqual({ selected: 'search', clicked: true, before: report.defaultSpell.nodes + 2, after: report.defaultSpell.nodes + 1, searchRemains: false, spellNodes: report.defaultSpell.nodes + 1 });
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

test('a child spell published and then saved with the Save button in Studio keeps one release and a new draft, and a flow/subgraph node placed in Studio runs that saved draft of its child when unpinned and the pinned release once RELEASE VERSION is set, exactly like the server, and leaves no orphan entry once deleted', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-subgraph-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['subgraph', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const published = db.prepare("SELECT version, json_extract(graph, '$.id') AS spellId, json_extract(graph, '$.nodes[1].config.template') AS template FROM spell_releases ORDER BY version").all() as { version: number; spellId: string; template: string }[];
    const drafts = db.prepare("SELECT json_extract(graph, '$.nodes[1].config.template') AS template FROM spells WHERE json_extract(graph, '$.id') = ?").all(report.childSpellId) as { template: string }[];
    db.close();
    expect(published).toEqual([{ version: 1, spellId: report.childSpellId, template: 'v1 saw {{topic}}' }]);
    expect(drafts).toEqual([{ template: 'draft saw {{topic}}' }]);
    expect(report.childSave).toEqual({ published: published[0].version, saveButton: true, status: 'Draft saved', releases: [published[0].version], storeError: null });
    const said = (template: string) => template.replace('{{topic}}', report.content);

    expect(report.placed).toEqual({ kind: 'flow/subgraph', editor: true, caption: 'Spell ID', versionField: true, versionCaption: 'Release version' });
    expect(report.pointed.subgraphs).toEqual([{ node_id: report.callNode, spell_id: report.childSpellId, release_version: null }]);
    expect(report.pointed.config).toMatchObject({ spell_id: report.childSpellId });
    expect(report.pointed.validation).toBe('valid');
    expect(report.pointed.errors).toEqual([]);

    expect(report.run.status).toBe('Succeeded');
    expect(report.run.error).toBeNull();
    expect(report.run.failedNodes).toEqual([]);
    expect(report.run.replies).toEqual([said(drafts[0].template), `parent heard ${said(drafts[0].template)}`]);

    expect(report.pinned.subgraphs).toEqual([{ node_id: report.callNode, spell_id: report.childSpellId, release_version: published[0].version }]);
    expect(report.pinned.config).toMatchObject({ spell_id: report.childSpellId, release_version: published[0].version });
    expect(report.pinned.validation).toBe('valid');
    expect(report.pinned.run.status).toBe('Succeeded');
    expect(report.pinned.run.replies).toEqual([said(published[0].template), `parent heard ${said(published[0].template)}`]);

    expect(report.badVersion.validation).toBe('invalid');
    expect(report.badVersion.errors.join('\n')).toContain('release version');
    expect(report.badVersion.subgraphs).toEqual([]);

    expect(report.unpinned.subgraphs).toEqual([{ node_id: report.callNode, spell_id: report.childSpellId, release_version: null }]);
    expect(report.unpinned.validation).toBe('valid');

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

test('a Studio spell saved through the API with an empty flow/subgraph config follows RELEASE VERSION edits: it runs v1 as saved, v2 once 2 is typed, refuses zero and runs the saved draft once the field is cleared, and reads a saved release_version of 1.0 as release 1', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-subgraph-ref-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['subgraph-ref-edit', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const published = db.prepare("SELECT version, json_extract(graph, '$.nodes[1].config.template') AS template FROM spell_releases ORDER BY version").all() as { version: number; template: string }[];
    const drafts = db.prepare("SELECT json_extract(graph, '$.nodes[1].config.template') AS template FROM spells WHERE json_extract(graph, '$.id') = ?").all(report.childSpellId) as { template: string }[];
    db.close();
    expect(published).toEqual([
      { version: 1, template: 'v1 saw {{topic}}' },
      { version: 2, template: 'v2 saw {{topic}}' },
    ]);
    expect(drafts).toEqual([{ template: 'draft saw {{topic}}' }]);
    const said = (template: string) => template.replace('{{topic}}', report.content);
    const replies = (template: string) => [said(template), `parent heard ${said(template)}`];
    const target = (version: number | null) => [{ node_id: report.callNode, spell_id: report.childSpellId, release_version: version }];

    expect(report.saved.config).toEqual({});
    expect(report.saved.versionText).toBe(String(published[0].version));
    expect(report.saved.subgraphs).toEqual(target(published[0].version));
    expect(report.saved.run.status).toBe('Succeeded');
    expect(report.saved.run.replies).toEqual(replies(published[0].template));

    expect(report.second.validation).toBe('valid');
    expect(report.second.subgraphs).toEqual(target(published[1].version));
    expect(report.second.run.status).toBe('Succeeded');
    expect(report.second.run.replies).toEqual(replies(published[1].template));

    expect(report.badVersion.validation).toBe('invalid');
    expect(report.badVersion.errors.join('\n')).toContain('release version "zero" must be a positive whole number');

    expect(report.cleared.validation).toBe('valid');
    expect(report.cleared.subgraphs).toEqual(target(null));
    expect(report.cleared.run.status).toBe('Succeeded');
    expect(report.cleared.run.replies).toEqual(replies(drafts[0].template));

    expect(report.floatPinned.config).toEqual({ spell_id: report.childSpellId, release_version: 1.0 });
    expect(report.floatPinned.versionText).toBe(String(published[0].version));
    expect(report.floatPinned.validation).toBe('valid');
    expect(report.floatPinned.errors).toEqual([]);
    expect(report.floatPinned.subgraphs).toEqual(target(published[0].version));
    expect(report.floatPinned.run.status).toBe('Succeeded');
    expect(report.floatPinned.run.replies).toEqual(replies(published[0].template));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('a Studio run whose subgraph node call runs a child node send leaves the unrelated root node named call/send idle and shows the child event under its call path', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-collision-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['subgraph-collision', dbPath]);
    expect(report.run.status).toBe('Succeeded');
    expect(report.run.replies).toEqual([report.childReply]);
    expect(report.statuses).toEqual({ trigger: 'Succeeded', gate: 'Succeeded', call: 'Succeeded', 'call/send': 'Idle' });
    expect(report.trace.filter((line: { kind: string; node: string }) => line.kind === 'NodeFinished' && line.node === 'send')).toEqual([{ kind: 'NodeFinished', path: ['call'], node: 'send' }]);
    expect(report.trace.filter((line: { node: string; path: string[] }) => line.node === 'call/send')).toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio reopens the newest saved draft instead of the demo or an older release, clears Draft saved as soon as the spell is edited, and a Save after reopening keeps that draft', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-draft-reopen-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['draft-reopen', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const prompt = `json_extract(graph, '$.nodes[' || (SELECT key FROM json_each(graph, '$.nodes') WHERE json_extract(value, '$.id') = '${report.node}') || '].config.prompt')`;
    const drafts = db.prepare(`SELECT id, ${prompt} AS prompt FROM spells`).all() as { id: string; prompt: string }[];
    const releases = db.prepare(`SELECT spell_id AS spellId, version, ${prompt} AS prompt FROM spell_releases ORDER BY version`).all() as { spellId: string; version: number; prompt: string }[];
    db.close();

    expect(drafts).toEqual([{ id: report.firstReopen.recordId, prompt: report.typed.afterRelease }]);
    expect(releases).toEqual([{ spellId: report.firstReopen.recordId, version: 1, prompt: report.typed.first }]);

    expect(report.firstSave).toEqual({ status: 'Draft saved', storeError: null });
    expect(report.firstReopen.prompt).toBe(report.typed.first);
    expect(report.firstReopen.releases).toEqual([]);
    expect(report.firstReopen.activeRelease).toBeNull();

    expect(report.published).toEqual({ version: releases[0].version, storeError: null });
    expect(report.savedAfterRelease).toEqual({ status: 'Draft saved', storeError: null });
    expect(report.editedAfterSave).toEqual({ prompt: report.typed.afterRelease, status: null });
    expect(report.savedAgain).toEqual({ status: 'Draft saved', storeError: null });

    expect(report.secondReopen.prompt).toBe(drafts[0].prompt);
    expect(report.secondReopen.releases).toEqual([releases[0].version]);
    expect(report.secondReopen.activeRelease).toBeNull();
    expect(report.secondReopen.recordId).toBe(drafts[0].id);
    expect(report.resaved).toEqual({ status: 'Draft saved', storeError: null, prompt: drafts[0].prompt });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio without --db refuses Save and Publish with a message naming --db instead of claiming Draft saved, and Save refuses a RELEASE VERSION of zero while keeping the stored draft on release 1', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-save-guards-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['save-guards', dbPath]);

    expect(report.noDb.status).toBeNull();
    expect(report.noDb.saveError).toContain('pass --db <file.sqlite> to save drafts on disk');
    expect(report.noDb.publishError).toContain('pass --db <file.sqlite> to publish releases on disk');
    expect(report.noDb.releases).toEqual([]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const stored = db
      .prepare("SELECT json_extract(graph, '$.nodes[1].config.release_version') AS configVersion, json_extract(graph, '$.subgraphs[0].release_version') AS refVersion FROM spells")
      .all() as { configVersion: unknown; refVersion: unknown }[];
    db.close();

    expect(report.pinned).toEqual({ status: 'Draft saved', storeError: null });
    expect(stored).toEqual([{ configVersion: 1, refVersion: 1 }]);
    expect(report.zero.status).toBeNull();
    expect(report.zero.storeError).toContain('release version "zero" must be a positive whole number');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio opened on release A@1, which calls the B draft that calls the leaf A draft, keeps the A@1 identity: it validates, runs with the A draft reply and publishes A@2', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-release-identity-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['release-identity', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const releases = db.prepare('SELECT spell_id AS spellId, version FROM spell_releases ORDER BY version').all() as { spellId: string; version: number }[];
    const aRecord = db.prepare("SELECT id FROM spells WHERE json_extract(graph, '$.id') = ?").all(report.aSpellId) as { id: string }[];
    db.close();

    expect(releases).toEqual([
      { spellId: aRecord[0].id, version: 1 },
      { spellId: aRecord[0].id, version: 2 },
    ]);
    expect(report.opened).toEqual({ spellId: report.aSpellId, recordId: aRecord[0].id, activeRelease: releases[0].version, releases: [releases[0].version] });
    expect(report.validation).toEqual({ validation: 'valid', errors: [] });
    expect(report.run.status).toBe('Succeeded');
    expect(report.run.error).toBeNull();
    expect(report.run.replies).toEqual([report.aDraftReply]);
    expect(report.published).toEqual({ version: releases[1].version, storeError: null, activeRelease: releases[1].version, releases: releases.map((release) => release.version) });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio treats --db :memory: and every in-memory SQLite form as not on disk, so Save and Publish refuse with a message naming --db and claim nothing was saved', { timeout: 900_000 }, async () => {
  const forms = [':memory:', 'sqlite::memory:', 'sqlite://:memory:', 'file::memory:', 'file:studio?mode=memory'];
  const report = await studioProbe(['memory-db', ...forms]);

  expect(report.forms.map((form: { db: string }) => form.db)).toEqual(forms);
  for (const form of report.forms) {
    expect(form.persistent).toBe(false);
    expect(form.status).toBeNull();
    expect(form.saveError).toBe('pass --db <file.sqlite> to save drafts on disk');
    expect(form.publishError).toBe('pass --db <file.sqlite> to publish releases on disk');
    expect(form.releases).toEqual([]);
  }
});

test('Save and Publish show their outcome in the TRACE strip on the default Nodes tab: the --db hint without a database, Draft saved and Published v1 with one, and Draft saved disappears after the next edit', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-visible-status-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['visible-status', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const releases = db.prepare('SELECT version FROM spell_releases ORDER BY version').all() as { version: number }[];
    const drafts = db.prepare('SELECT id FROM spells').all() as { id: string }[];
    db.close();
    expect(releases).toEqual([{ version: 1 }]);
    expect(drafts).toHaveLength(1);

    expect(report.noDb.leftTab).toBe('Palette');
    expect(report.noDb.storeError).toBe('pass --db <file.sqlite> to save drafts on disk');
    expect(report.noDb.shown).toEqual({ text: report.noDb.storeError, insideTrace: true, onScreen: true });

    expect(report.saved.leftTab).toBe('Palette');
    expect(report.saved.status).toBe('Draft saved');
    expect(report.saved.shown).toEqual({ text: 'Draft saved', insideTrace: true, onScreen: true });

    expect(report.published.leftTab).toBe('Palette');
    expect(report.published.version).toBe(releases[0].version);
    expect(report.published.shown).toEqual({ text: `Published v${releases[0].version}`, insideTrace: true, onScreen: true });

    expect(report.resaved.shown).toEqual({ text: 'Draft saved', insideTrace: true, onScreen: true });
    expect(report.edited.status).toBeNull();
    expect(report.edited.shown).toBeNull();
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio launched with a --db it cannot open keeps that open error on Save and Publish instead of telling the user to pass --db', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-unopenable-db-'));
  const blocker = join(dir, 'blocker');
  const dbPath = join(blocker, 'studio.sqlite');
  try {
    await writeFile(blocker, 'a plain file where a folder should be');
    const report = await studioProbe(['unopenable-db', dbPath]);

    expect(report.persistent).toBeNull();
    expect(report.saveError).toContain('the database passed with --db could not be opened: ');
    expect(report.saveError).not.toContain('pass --db');
    expect(report.publishError).toContain('the database passed with --db could not be opened: ');
    expect(report.publishError).not.toContain('pass --db');
    expect(report.status).toBeNull();
    expect(report.shown).toEqual({ text: report.publishError, insideTrace: true, onScreen: true });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio runs a subgraph from the opened spell project only: a Project 2 parent calling a graph id that a Project 1 child also uses replies from the Project 2 child', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-project-scope-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['project-scope', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const spells = db.prepare("SELECT s.id AS id, s.name AS name, p.name AS project, json_extract(s.graph, '$.id') AS graphId FROM spells s JOIN projects p ON p.id = s.project_id ORDER BY s.name").all() as { id: string; name: string; project: string; graphId: string }[];
    db.close();
    const byName = Object.fromEntries(spells.map((spell) => [spell.name, spell]));
    expect(spells.map((spell) => spell.name)).toEqual(['child one', 'child two', 'parent two']);
    expect(byName['child one'].project).toBe('Project 1');
    expect(byName['child two'].project).toBe('Project 2');
    expect(byName['parent two'].project).toBe('Project 2');
    expect(byName['child one'].graphId).toBe(report.sharedId);
    expect(byName['child two'].graphId).toBe(report.sharedId);

    expect(report.opened.recordId).toBe(byName['parent two'].id);
    expect(report.validation).toEqual({ validation: 'valid', errors: [] });
    expect(report.run.status).toBe('Succeeded');
    expect(report.run.error).toBeNull();
    expect(report.run.replies).toEqual([report.replies.two]);
    expect(report.run.replies).not.toContain(report.replies.one);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio opened on A@1 publishes A@3 after another session published A@2 into the same database, even when A now calls A@2, and lists releases 1, 2 and 3', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-out-of-band-release-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['out-of-band-release', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const releases = db.prepare('SELECT spell_id AS spellId, version FROM spell_releases ORDER BY version').all() as { spellId: string; version: number }[];
    const latest = db.prepare("SELECT json_extract(graph, '$.subgraphs') AS subgraphs FROM spell_releases WHERE version = 3").all() as { subgraphs: string }[];
    db.close();

    expect(releases).toEqual([1, 2, 3].map((version) => ({ spellId: report.recordId, version })));
    expect(report.opened).toEqual({ recordId: report.recordId, activeRelease: 1, releases: [1] });
    expect(report.outside).toEqual({ version: 2 });
    expect(report.edited.subgraphs).toEqual([{ node_id: report.callNode, spell_id: report.recordId, release_version: 2 }]);
    expect(report.published).toEqual({ version: 3, storeError: null, validation: 'valid', releases: [1, 2, 3] });
    expect(JSON.parse(latest[0].subgraphs)).toEqual([{ node_id: report.callNode, spell_id: report.recordId, release_version: 2 }]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio asks SQLite whether a percent-encoded in-memory URI like file:memtest?mode=mem%6fry is on disk, so Save and Publish refuse with the --db hint and no file is created', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-encoded-memory-'));
  try {
    const forms = [`file:${join(dir, 'memtest')}?mode=mem%6fry`, `sqlite:file:${join(dir, 'memtest-two')}?mode=mem%6fry`, ':memory:'];
    const report = await studioProbe(['memory-db', ...forms]);

    expect(report.forms.map((form: { db: string }) => form.db)).toEqual(forms);
    for (const form of report.forms) {
      expect(form.persistent).toBe(false);
      expect(form.status).toBeNull();
      expect(form.saveError).toBe('pass --db <file.sqlite> to save drafts on disk');
      expect(form.publishError).toBe('pass --db <file.sqlite> to publish releases on disk');
      expect(form.releases).toEqual([]);
    }
    expect(await readdir(dir)).toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio saves, publishes and reopens real SQLite files whose paths contain mode=memory, such as mode=memory.sqlite and a mode=memory folder', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-mode-memory-name-'));
  try {
    await mkdir(join(dir, 'mode=memory'));
    const paths = [join(dir, 'mode=memory.sqlite'), join(dir, 'mode=memory', 'studio.sqlite')];

    const first = await studioProbe(['memory-db', ...paths]);
    const stored = (path: string) => {
      const db = new DatabaseSync(path, { readOnly: true });
      const releases = db.prepare('SELECT spell_id AS spellId, version FROM spell_releases ORDER BY version').all() as { spellId: string; version: number }[];
      const spells = db.prepare('SELECT id FROM spells').all() as { id: string }[];
      db.close();
      return { releases, spells };
    };
    const afterFirst = paths.map(stored);
    const second = await studioProbe(['memory-db', ...paths]);
    const afterSecond = paths.map(stored);

    for (const [index, path] of paths.entries()) {
      expect((await stat(path)).size).toBeGreaterThan(0);
      expect(first.forms[index]).toEqual({ db: path, persistent: true, status: 'Draft saved', saveError: null, publishError: null, releases: [1] });
      expect(afterFirst[index].spells).toHaveLength(1);
      expect(afterFirst[index].releases).toEqual([{ spellId: afterFirst[index].spells[0].id, version: 1 }]);
      expect(second.forms[index]).toEqual({ db: path, persistent: true, status: 'Draft saved', saveError: null, publishError: null, releases: [1, 2] });
      expect(afterSecond[index].spells).toEqual(afterFirst[index].spells);
      expect(afterSecond[index].releases).toEqual([1, 2].map((version) => ({ spellId: afterFirst[index].spells[0].id, version })));
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio opened on an empty database refuses to validate or publish a flow/subgraph call to a child that another client saved in a different project, and publishes nothing into Studio', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-foreign-child-'));
  const dbPath = join(dir, 'studio.sqlite');
  try {
    const report = await studioProbe(['foreign-child', dbPath]);

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const projects = db.prepare('SELECT name FROM projects ORDER BY name').all() as { name: string }[];
    const spells = db.prepare('SELECT s.id AS id, p.name AS project FROM spells s JOIN projects p ON p.id = s.project_id').all() as { id: string; project: string }[];
    const releases = db.prepare('SELECT version FROM spell_releases').all() as { version: number }[];
    db.close();

    expect(projects).toEqual([{ name: 'Other' }]);
    expect(spells).toEqual([{ id: report.childRecordId, project: 'Other' }]);
    expect(releases).toEqual([]);

    expect(report.openedProject).toBeNull();
    expect(report.edited.subgraphs).toEqual([{ node_id: report.callNode, spell_id: report.childRecordId, release_version: null }]);
    expect(report.validation.validation).toBe('invalid');
    expect(report.validation.errors.join('\n')).toContain(report.childRecordId);
    expect(report.published.version).toBeNull();
    expect(report.published.releases).toEqual([]);
    expect(report.published.storeError).toContain('cannot publish an invalid spell: ');
    expect(report.published.storeError).toContain(report.childRecordId);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Editing the spell clears a one-off Publish or Save error from the TRACE strip, while a --db open error still comes back on the next Save', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-edit-clears-error-'));
  const dbPath = join(dir, 'studio.sqlite');
  const blocker = join(dir, 'blocker');
  const unopenable = join(blocker, 'studio.sqlite');
  try {
    await writeFile(blocker, 'a plain file where a folder should be');
    const report = await studioProbe(['edit-clears-store-error', dbPath, unopenable]);

    expect(report.refused.storeError).toContain('cannot publish an invalid spell: ');
    expect(report.refused.shown).toEqual({ text: report.refused.storeError, insideTrace: true, onScreen: true });
    expect(report.refusedEdited.storeError).toBeNull();
    expect(report.refusedEdited.shown).toBeNull();

    expect(report.unopenable.storeError).toContain('the database passed with --db could not be opened: ');
    expect(report.unopenable.shown).toEqual({ text: report.unopenable.storeError, insideTrace: true, onScreen: true });
    expect(report.unopenableEdited.storeError).toBeNull();
    expect(report.unopenableEdited.shown).toBeNull();
    expect(report.unopenableEdited.openError).not.toBeNull();
    expect(report.unopenableSavedAgain.storeError).toBe(report.unopenable.storeError);
    expect(report.unopenableSavedAgain.status).toBeNull();
    expect(report.unopenableSavedAgain.shown).toEqual({ text: report.unopenable.storeError, insideTrace: true, onScreen: true });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('magicklala studio --snap opens its --db exactly as given: a plain path becomes a migrated SQLite file, a sqlite:// URL whose mode decodes to memory writes nothing, and a file name ending in a space keeps that space', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-cli-db-'));
  try {
    await run('cargo', ['build', '-q', '-p', 'magicklala-cli'], { cwd: repoRoot, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
    const memoryDir = join(dir, 'memory');
    const spaceDir = join(dir, 'space');
    await mkdir(memoryDir);
    await mkdir(spaceDir);
    const realDb = join(dir, 'real.sqlite');
    const launches = [
      { png: join(dir, 'real.png'), db: realDb },
      { png: join(dir, 'memory.png'), db: `sqlite://${join(memoryDir, 'demo')}?mode=mem%6fry` },
      { png: join(dir, 'space.png'), db: join(spaceDir, 'intended.sqlite ') },
    ];
    const env: NodeJS.ProcessEnv = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '', MAGICKLALA_LLM_BASE_URL: unreachableLlm, MAGICKLALA_LLM_API_KEY: '', OPENAI_API_KEY: '' };
    delete env.MAGICKLALA_DB;
    for (const launch of launches) {
      await run(join(repoRoot, 'target', 'debug', 'magicklala'), ['studio', '--snap', launch.png, '--db', launch.db], { cwd: dir, env, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
    }

    for (const launch of launches) {
      expect([...(await readFile(launch.png)).subarray(0, 8)]).toEqual(pngSignature);
    }
    expect(await readdir(dir)).toContain('real.sqlite');
    const db = new DatabaseSync(realDb, { readOnly: true });
    const tables = db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'").all();
    db.close();
    expect(tables).toEqual([{ name: 'schema_migrations' }]);
    expect(await readdir(memoryDir)).toEqual([]);
    const spaceFiles = await readdir(spaceDir);
    expect(spaceFiles).toContain('intended.sqlite ');
    expect(spaceFiles).not.toContain('intended.sqlite');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio saves, publishes and reopens SQLite files whose names end in a space or contain %, ? and #, so releases 1 and 2 land in those exact files on disk', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-odd-names-'));
  try {
    const paths = [join(dir, 'intended.sqlite '), join(dir, 'q?t#1%2.sqlite')];

    const first = await studioProbe(['memory-db', ...paths]);
    const second = await studioProbe(['memory-db', ...paths]);

    const files = await readdir(dir);
    expect(files).toContain('intended.sqlite ');
    expect(files).toContain('q?t#1%2.sqlite');
    expect(files).not.toContain('intended.sqlite');
    for (const [index, path] of paths.entries()) {
      expect(first.forms[index]).toEqual({ db: path, persistent: true, status: 'Draft saved', saveError: null, publishError: null, releases: [1] });
      expect(second.forms[index]).toEqual({ db: path, persistent: true, status: 'Draft saved', saveError: null, publishError: null, releases: [1, 2] });
      const db = new DatabaseSync(path, { readOnly: true });
      const versions = db.prepare('SELECT version FROM spell_releases ORDER BY version').all() as { version: number }[];
      db.close();
      expect(versions.map((row) => row.version)).toEqual([1, 2]);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Check pins a hex on the exact ingredient and input that are broken: a missing query wire flags only that socket and names it on the cauldron, and reply_support missing content never flags its look-alike reply', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['hex-pinpoint']);

  const missing = report.missingQuery;
  expect(missing.report.validation).toBe('invalid');
  expect(missing.report.flagged).toEqual([{ node: missing.removed.node, sockets: [missing.removed.socket] }]);
  expect(missing.report.plaque).toBe(`Hexed · missing "${missing.removed.socket}"`);
  expect(missing.report.log).toEqual([`${missing.removed.title}: missing required input "${missing.removed.socket}"`]);

  const twins = report.twinIds;
  expect(twins.report.validation).toBe('invalid');
  expect(twins.report.flagged).toEqual([{ node: twins.twin, sockets: ['content'] }]);
  expect(twins.report.flagged.map((entry: { node: string }) => entry.node)).not.toContain(twins.lookalike);
  expect(twins.report.plaque).toBe('Hexed · missing "content"');
});

test('The hex fix button on a node missing a required input pours the matching upstream output back into that input, so the spell checks valid with exactly that one wire restored', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['hex-pinpoint']);
  const { removed, mend } = report.missingQuery;

  expect(mend.added).toEqual([{ from_node: removed.from_node, from_socket: removed.from_socket, to_node: removed.node, to_socket: removed.socket }]);
  expect(mend.validation).toBe('valid');
});

test('The cauldron shows every ingredient and bottle of the support, webhook router, parallel research and twelve-step spells at 1280x800 and 960x640 with the pantry open or closed, with no two cards overlapping, every card label at least 11px tall when opened, and zooming all the way out and in never crashes', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['pot-layout']);
  const parallel = JSON.parse(await readFile(join(repoRoot, 'examples', 'parallel_research.json'), 'utf8')) as ExampleSpell;

  expect(report.cases.map((entry: { spell: string }) => entry.spell)).toEqual(expect.arrayContaining(['support_agent', 'webhook_router', 'parallel_research', 'dozen_ingredients']));
  expect(report.cases.find((entry: { spell: string }) => entry.spell === 'parallel_research').nodes).toEqual(parallel.nodes.map((node) => node.id));
  for (const entry of report.cases) {
    const where = `${entry.spell} at ${entry.size.join('x')} pantry ${entry.pantryOpen ? 'open' : 'closed'}`;
    expect(entry.opened.missing, where).toEqual([]);
    expect(entry.opened.outside, where).toEqual([]);
    expect(entry.opened.smallestLine, where).toBeGreaterThanOrEqual(11);
    for (const shot of entry.zooms) {
      expect(shot.panicked, `${where} zoom ${shot.zoom}`).toBe(false);
      expect(shot.overlaps, `${where} zoom ${shot.zoom}`).toEqual([]);
    }
  }
});

type SocketInfo = { name: string; ty: string; required: boolean };
type KindSockets = { inputs: SocketInfo[]; outputs: SocketInfo[] };

test('Tossing On Message, Generate Text and Send Message into an empty cauldron from the pantry brews a recipe that checks valid: flow runs trigger to model to reply and each required text input is fed by the step before it', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['toss']);
  const { ids, specs, wires, validation } = report.built;
  const [trigger, model, reply] = ids as string[];
  const flowOut = (kind: string) => (specs[kind] as KindSockets).outputs.find((socket) => socket.ty === 'Flow')!.name;
  const flowIn = (kind: string) => (specs[kind] as KindSockets).inputs.find((socket) => socket.ty === 'Flow')!.name;
  const requiredText = (kind: string) => (specs[kind] as KindSockets).inputs.filter((socket) => socket.required && socket.ty === 'String').map((socket) => socket.name);
  const textOut = (kind: string) => (specs[kind] as KindSockets).outputs.find((socket) => socket.ty === 'String')!.name;

  expect(ids.every((id: string | null) => typeof id === 'string')).toBe(true);
  const expected = [
    [trigger, flowOut('event/on_message'), model, flowIn('ai/generate_text')],
    ...requiredText('ai/generate_text').map((input) => [trigger, textOut('event/on_message'), model, input]),
    [model, flowOut('ai/generate_text'), reply, flowIn('io/send_message')],
    ...requiredText('io/send_message').map((input) => [model, textOut('ai/generate_text'), reply, input]),
  ];
  expect([...wires].sort()).toEqual([...expected].sort());
  expect(validation).toBe('valid');
});

test('Dragging Generate Text from the pantry onto a full support cauldron pours it in just before the reply bottle: the old model now flows into it, it flows into the reply, and its prompt is fed by the old model text', { timeout: 900_000 }, async () => {
  const { pantryDrop } = await studioProbe(['toss']);
  const fresh = pantryDrop.id as string;
  const wires = pantryDrop.wiresAfter as string[][];
  const has = (wire: string[]) => wires.some((candidate) => candidate.join('|') === wire.join('|'));

  expect(pantryDrop.nodesAfter).toBe(pantryDrop.nodesBefore + 1);
  expect(fresh).toEqual(expect.any(String));
  expect(pantryDrop.role).toBe('Ingredient');
  expect(has(['llm', 'out', fresh, 'in'])).toBe(true);
  expect(has([fresh, 'out', 'reply', 'in'])).toBe(true);
  expect(has(['llm', 'out', 'reply', 'in'])).toBe(false);
  expect(has(['llm', 'text', fresh, 'prompt'])).toBe(true);
  expect(has([fresh, 'text', 'reply', 'content'])).toBe(true);
  expect(has(['llm', 'text', 'reply', 'content'])).toBe(false);
  expect(pantryDrop.ghostDuringDrag).toBe(false);
});

test('Dragging a Template onto a cauldron whose every input is already fed adds it set aside and leaves every existing wire untouched', { timeout: 900_000 }, async () => {
  const { pantryFull } = await studioProbe(['toss']);

  expect(pantryFull.nodesAfter).toBe(pantryFull.nodesBefore + 1);
  expect(pantryFull.role).toBe('Aside');
  expect([...pantryFull.wiresAfter].sort()).toEqual([...pantryFull.wiresBefore].sort());
});

test('Dragging a vial off the cauldron sets it aside with every wire removed, dragging it back pours it in again, and no cauldron drag ever moves a node in the Grimoire', { timeout: 900_000 }, async () => {
  const { setAside } = await studioProbe(['toss']);

  expect(setAside.wiresBefore).toBeGreaterThan(0);
  expect(setAside.wiresAside).toBe(0);
  expect(setAside.roleAside).toBe('Aside');
  expect(setAside.wiresBack).toBeGreaterThan(0);
  expect(setAside.roleBack).toBe('Ingredient');
  expect(setAside.positionsAfter).toEqual(setAside.positionsBefore);
});

test('Every Studio panel names things in plain words: pantry shelves are Events, Flow, AI, Memory, Knowledge, Outputs and so on, the right tabs read Recipe, Trace, Variables, Tasting, counts say variables, and no hint line anywhere (empty pot, hexed spell, nothing picked, releases) tells the user what to click, drag or fix', { timeout: 900_000 }, async () => {
  const { texts } = await studioProbe(['plain-words']);
  const shown = new Set(texts as string[]);

  for (const heading of ['Events', 'Flow', 'AI', 'Memory', 'Knowledge', 'Outputs', 'Code', 'Recipe', 'Trace', 'Variables', 'Tasting']) expect(shown.has(heading)).toBe(true);
  for (const flourish of ['Sparks & Events', 'Whisk & Flow', 'Arcane AI', 'Memory Jars', 'Herb Lore', 'Potion Bottles', 'Rhai Runes', 'Essences', 'Pours in', 'Distills out', 'Prompt · Incantation', 'Potion spices', 'Spices']) {
    expect(shown.has(flourish)).toBe(false);
  }
  const hints = /\b(click|press|tap|drag|drop|select an?|start with|fix the|to add|to edit|send a prompt|herb|incantation|essence|spices?|vintages?|taste this potion)\b/i;
  expect((texts as string[]).filter((line) => line !== 'Add On Message' && hints.test(line))).toEqual([]);
});

test('The toolbar pill and the cauldron plaque tell the truth about a brew: Brewing while it runs, Brewed only when the run succeeded, Failed when a step failed, Cancelled when it was stopped, and the pot stops simmering the moment a run ends', { timeout: 900_000 }, async () => {
  const { succeeded, failed, running, cancelled } = await studioProbe(['brew-truth']);

  expect(running.pill).toBe('Brewing');
  expect(running.runningBrew).toBe(true);
  expect(succeeded.outcome).toBe('Succeeded');
  expect(succeeded.pill).toBe('Brewed');
  expect(succeeded.plaque).toMatch(/^Brewed · /);
  expect(failed.outcome).toBe('Failed');
  expect(failed.pill).toBe('Failed');
  expect(failed.plaque).not.toMatch(/^Brewed/);
  expect(cancelled.outcome).toBe('Cancelled');
  expect(cancelled.pill).toBe('Cancelled');
  expect(cancelled.plaque).not.toMatch(/^Brewed/);
  for (const ended of [succeeded, failed, cancelled]) expect(ended.runningBrew).toBe(false);
});

test('Opening another potion while one is still brewing stops the old brew: the new potion shows no running brew, no old chat, no old trace, no lit wires, and the old reply never arrives in it', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['switch-mid-brew']);

  expect(report.runningBefore).toBe(true);
  expect(report.spell).toBe('Research Agent');
  expect(report.running).toBe(false);
  expect(report.runningBrew).toBe(false);
  expect(report.chat).toEqual([]);
  expect(report.trace).toBe(0);
  expect(report.lastRun).toBeNull();
  expect(report.lastBrew).toBeNull();
  expect(report.busyCards).toBe(0);
  expect(report.activeWires).toBe(0);
  expect(report.backgroundStopped).toBe(true);
});

test('Opening a spell and looking at its Variables never rewrites them: an unset text default stays unset, an empty one stays empty, a JSON default stays JSON, nothing is marked edited, and typing new JSON saves JSON', { timeout: 900_000 }, async () => {
  const { original, opened, dirtyAfterOpen, edited } = await studioProbe(['variable-defaults']);

  expect(opened).toEqual(original);
  expect(dirtyAfterOpen).toBe(false);
  expect(edited[2].default).toEqual({ type: 'json', value: { tone: 'brisk' } });
  expect(edited.filter((_: unknown, index: number) => index !== 2)).toEqual(original.filter((_: unknown, index: number) => index !== 2));
});

test('After a publish the cauldron plaque moves on: it says Published, then follows the brew, then says Draft once the recipe is edited, and the edit clears the lit streams and step marks of the old brew', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-plaque-'));
  try {
    const { published, brewed, edited } = await studioProbe(['publish-plaque', join(dir, 'studio.sqlite')]);

    expect(published).toMatch(/^Published v1 · /);
    expect(brewed.pill).toBe('Draft brewed');
    expect(brewed.plaque).toMatch(/^Brewed · /);
    expect(edited.plaque).toMatch(/^Draft · /);
    expect(edited.pill).toBe('Draft');
    expect(edited.litWires).toBe(0);
    expect(edited.markedCards).toBe(0);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Cauldron vials are safe to handle: a small nudge keeps every wire, setting one aside can be undone with its wires back, set-aside wires never follow you into another potion, right-clicking empty space after picking a card offers the empty-space menu, the card menu never covers its card, and nothing can be tossed while a brew is running', { timeout: 900_000 }, async () => {
  const { nudge, undo, leak, menu, whileRunning } = await studioProbe(['toss-safety']);

  expect(nudge.after).toBe(nudge.before);
  expect(nudge.role).toBe('Ingredient');
  expect(undo.asideWires).toBe(0);
  expect(undo.asideRole).toBe('Aside');
  expect(undo.undoShown).toBe(true);
  expect(undo.restoredWires).toBe(nudge.before);
  expect(undo.restoredRole).toBe('Ingredient');
  expect(leak.stashedBefore).toBe(1);
  expect(leak.stashedAfterSwitch).toBe(0);
  expect(menu.emptyItems).toEqual(['context-add-ai-generate_text', 'context-fit-view', 'context-dismiss']);
  expect(menu.cardItems).toEqual(['context-duplicate-node', 'context-delete-node', 'context-dismiss']);
  expect(menu.coversCard).toBe(false);
  expect(whileRunning.running).toBe(true);
  expect(whileRunning.after).toBe(whileRunning.before);
});

test('The cauldron keeps the recipe honest: a tossed Add Memory is fed only from ingredients in the pot and never takes over the reply, the Research Agent sample passes Check, a list wired into a text prompt reads Wrong type, the first lone vial waits by the pot, a spell that ends in Add Memory bottles it instead of a ghost, and disabled buttons and the ghost bottle stay readable in both themes', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['recipe-honesty']);

  expect(report.memory.feeders.some((wire: string) => wire.startsWith('recall.'))).toBe(false);
  expect(report.memory.feeders).toContain('llm.text->content');
  expect(report.memory.replyFeeders).toEqual(expect.arrayContaining(['llm.text->content']));
  expect(report.memory.replyFeeders.filter((wire: string) => wire.endsWith('->content'))).toEqual(['llm.text->content']);
  expect(report.memory.replyFeeders).toContain(report.memory.id + '.out->in');
  expect(report.researchCheck).toBe('valid');
  expect(report.mistyped.card).toContain('Wrong type "prompt"');
  expect(report.mistyped.plaque).toBe('Hexed · wrong type "prompt"');
  expect(report.loneFirst).toBe('Ingredient');
  expect(report.sideEffect).toEqual({ role: 'Bottle', ghost: false });
  for (const shade of report.shades) {
    expect(shade.disabledText).toBeGreaterThanOrEqual(4.5);
    expect(shade.ghostOutline).toBeGreaterThanOrEqual(3);
  }
});

test('A brewed potion takes the colour of the last ingredient poured in, so the template tasting spell and the switch-routed webhook spell brew different potions', { timeout: 900_000 }, async () => {
  const { tasting, webhook } = await studioProbe(['potion-colours']);

  expect(tasting.lastPoured[0]).toBe('Template');
  expect(tasting.potion).toBe(tasting.lastPoured[1]);
  expect(webhook.lastPoured[0]).toBe('Switch');
  expect(webhook.potion).toBe(webhook.lastPoured[1]);
  expect(tasting.potion).not.toBe(webhook.potion);
});

test('Opening the Grimoire for the first time on a 960 by 640 window with the pantry pinned shows every node on screen, a Generate Text tossed in the cauldron sits after the old model without covering any node, and every branch route label sits inside its own bottle card', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['grimoire-place']);

  expect(report.offscreen).toEqual([]);
  expect(report.overlaps).toEqual([]);
  expect(report.freshX).toBeGreaterThan(report.llmX);
  expect(report.routes).toHaveLength(report.bottles);
  for (const route of report.routes) expect(route.insideCard, route.bottle).toBe(true);
});

test('The cauldron is the biggest thing on the support canvas and every ingredient vial sits on the pot side of its card, ready to pour', { timeout: 900_000 }, async () => {
  const { pot, cards } = await studioProbe(['pour-shape']);

  expect(cards).toHaveLength(4);
  for (const entry of cards) {
    expect(pot[2]).toBeGreaterThan(entry.card[2] * 1.4);
    expect(entry.vial, entry.id).not.toBeNull();
    const vialCentre = entry.vial[0] + entry.vial[2] / 2;
    expect(vialCentre, entry.id).toBeGreaterThan(entry.card[0] + entry.card[2] * 0.7);
  }
});
