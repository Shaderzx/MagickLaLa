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
    expect(report.noDb.saveError).toContain('Pass --db <file.sqlite> to save drafts on disk');
    expect(report.noDb.publishError).toContain('Pass --db <file.sqlite> to publish releases on disk');
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
    expect(form.saveError).toBe('Pass --db <file.sqlite> to save drafts on disk');
    expect(form.publishError).toBe('Pass --db <file.sqlite> to publish releases on disk');
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
    expect(report.noDb.storeError).toBe('Pass --db <file.sqlite> to save drafts on disk');
    expect(report.noDb.shown).toEqual({ text: report.noDb.storeError, insideTrace: true, onScreen: true });

    expect(report.saved.leftTab).toBe('Palette');
    expect(report.saved.status).toBe('Draft saved');
    expect(report.saved.shown).toEqual({ text: 'Draft saved', insideTrace: true, onScreen: true });

    expect(report.published.leftTab).toBe('Palette');
    expect(report.published.version).toBe(releases[0].version);
    expect(report.published.shown).toEqual({ text: `Published v${releases[0].version}`, insideTrace: true, onScreen: true });

    expect(report.resaved.shown).toEqual({ text: 'Draft saved', insideTrace: true, onScreen: true });
    expect(report.resaved.live).toBe(releases[0].version);
    expect(report.resaved.releasesAfterPublish).toEqual([releases[0].version]);
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
    expect(report.saveError).toContain('The database passed with --db could not be opened: ');
    expect(report.saveError).not.toContain('Pass --db');
    expect(report.publishError).toContain('The database passed with --db could not be opened: ');
    expect(report.publishError).not.toContain('Pass --db');
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
      expect(form.saveError).toBe('Pass --db <file.sqlite> to save drafts on disk');
      expect(form.publishError).toBe('Pass --db <file.sqlite> to publish releases on disk');
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
    expect(report.published.storeError).toContain('Cannot publish an invalid potion: ');
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

    expect(report.refused.storeError).toContain('Cannot publish an invalid potion: ');
    expect(report.refused.shown).toEqual({ text: report.refused.storeError, insideTrace: true, onScreen: true });
    expect(report.refusedEdited.storeError).toBeNull();
    expect(report.refusedEdited.shown).toBeNull();

    expect(report.unopenable.storeError).toContain('The database passed with --db could not be opened: ');
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

test('Dragging a Get Variable onto a cauldron whose every input is already fed adds it set aside and leaves every existing wire untouched', { timeout: 900_000 }, async () => {
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

test('The toolbar pill and the cauldron plaque tell the truth about a brew: Brewing while it runs, Brewed only when the run succeeded, Failed when a step failed with the card, log and summary saying Failed too and never Hexed, Cancelled when it was stopped and Timed out when it ran too long in the log too, and the pot stops simmering the moment a run ends', { timeout: 900_000 }, async () => {
  const { succeeded, failed, running, cancelled, timedWords } = await studioProbe(['brew-truth']);

  expect(running.pill).toBe('Brewing');
  expect(running.runningBrew).toBe(true);
  expect(succeeded.outcome).toBe('Succeeded');
  expect(succeeded.pill).toBe('Brewed');
  expect(succeeded.plaque).toMatch(/^Brewed · /);
  expect(failed.outcome).toBe('Failed');
  expect(failed.pill).toBe('Failed');
  expect(failed.plaque).not.toMatch(/^Brewed/);
  expect(failed.hexedWords).toEqual([]);
  expect(failed.failedWords).toBeGreaterThan(1);
  expect(cancelled.outcome).toBe('Cancelled');
  expect(cancelled.pill).toBe('Cancelled');
  expect(cancelled.plaque).not.toMatch(/^Brewed/);
  for (const ended of [succeeded, failed, cancelled]) expect(ended.runningBrew).toBe(false);
  expect(cancelled.logWords).not.toContain('Quenched');
  expect(cancelled.logWords).toContain('Cancelled');
  expect(timedWords).toContain('Timed out');
  expect(timedWords).not.toContain('Boiled over');
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

test('The cauldron reads cleanly at any size: no ingredient name is cut off, no pour line runs over another card, the idle broth already shows the last ingredient colour, steps number in feeding order, a Send that carries on is still a bottle, set-aside cards sit on a visible shelf, the pot grows on a roomy 1600 by 1000 window, branch bottles have distinct names, and Grimoire text stays 11px or taller', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['reads-cleanly']);

  expect(report.cut).toEqual([]);
  expect(report.crossings).toEqual([]);
  expect(report.readyBroth).toBe(report.lastLiquid);
  expect(report.chainSteps).toEqual([1, 2, 3, 4, 5]);
  expect(report.relayRoles).toEqual({ trigger: 'Ingredient', send: 'Bottle', keep: 'Ingredient' });
  expect(report.asideShelf).toEqual({ shelf: true, holdsCard: true });
  expect(report.potWidth).toBeGreaterThanOrEqual(340);
  expect(new Set(report.bottleTitles).size).toBe(report.bottleTitles.length);
  expect(report.bottleTitles).toHaveLength(3);
  expect(report.grimoireSmallest).toBeGreaterThanOrEqual(11);
});

test('Setting aside or deleting the middle step of a recipe joins the steps around it, so the spell still checks valid and the reply still brews, and a brew whose bottle never fills says so on the plaque', { timeout: 900_000 }, async () => {
  const { setAside, deleted, emptyBottle } = await studioProbe(['pull-middle']);

  for (const pulled of [setAside, deleted]) {
    expect(pulled.flowIntoReply).toBe(true);
    expect(pulled.check).toBe('valid');
    expect(pulled.outcome).toBe('Succeeded');
    expect(pulled.reply).toBe('done');
    expect(pulled.plaque).toMatch(/^Brewed · /);
    expect(pulled.bottleFull).toBe(true);
  }
  expect(emptyBottle.reply).not.toBe('done');
  expect(emptyBottle.bottleFull).toBe(false);
  expect(emptyBottle.plaque).toMatch(/bottle empty/);
});

test('Studio tells the truth in the corners: a tossed Replace really shapes the reply in either order, a spare trigger never hijacks the brew, nothing is added mid-brew, the Trace panel says Brewing then marks an edited result as the last brew, a wired prompt shows its source with a way to type it instead, a long name never pushes toolbar buttons off a 960 window, re-publishing an unchanged potion makes no new release, saved potions stay in the picker, and checkmarks are drawn rather than font glyphs', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-corners-'));
  try {
    const report = await studioProbe(['corner-truths', join(dir, 'studio.sqlite')]);

    for (const shaped of [report.tossed, report.spliced]) {
      expect(shaped.role).toBe('Ingredient');
      expect(shaped.replaceFeeders).toEqual(expect.arrayContaining([expect.stringMatching(/^on-message-\d+\.content->text$/)]));
      expect(shaped.replyFeeders).toContain(shaped.replace + '.text->content');
    }
    expect(report.twinReply).toBe('done');
    expect(report.traceRunning).toMatch(/Brewing/);
    expect(report.traceRunning).not.toMatch(/Brewed/);
    expect(report.addedMidBrew).toBe(0);
    expect(report.traceAfterEdit).toMatch(/Last brew/);
    expect(report.promptField).toMatch(/On Message · content/);
    expect(report.unplugShown).toBe(true);
    expect(report.promptAfter).toEqual({ wired: false, editor: true });
    expect(report.clipped).toEqual([]);
    expect(report.releases).toBe(1);
    expect(report.listed).toBe(true);
    expect(report.reopened).toBe('Tasting Agent');
    expect(report.checkGlyphs).toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Dropping a pantry ingredient on the set-aside tray adds it set aside with no wires instead of stirring it in', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['tray-drop']);

  expect(report).toEqual({ added: 1, role: 'Aside', wires: 0 });
});

test('Every step keeps one name and every card keeps its words: ingredient titles never run under their vial at 960 by 640 or 1280 by 800, the cauldron, Grimoire and Cauldron Log name each branch bottle by its route, look-alike bottles without routes are numbered, the log icon sits clear of the first step, and the empty pot offers its triggers in one place only', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['one-name']);

  expect(report.titleMisfits).toEqual([]);
  const routed = { order_reply: 'Send · orders', support_reply: 'Send · support', ignored_reply: 'Send · default' };
  expect(report.cauldronNames).toEqual(routed);
  expect(report.grimoireNames).toEqual(routed);
  expect(report.traceNames).toEqual({ order_reply: 'Send · orders' });
  expect(report.twinNames).toEqual({ first_reply: 'Send · 1', second_reply: 'Send · 2' });
  expect(report.logGap).toBeGreaterThanOrEqual(4);
  expect(report.logIcons).toEqual([[34, 28], [34, 28]]);
  expect(report.empty.secondAdd).toBe(false);
});
test('Studio fits and counts honestly at 960 by 640: variable names keep their own line, the hex card keeps its buttons inside, the opened pantry can be folded away again, the Grimoire draws its checkmarks and chevrons instead of font glyphs, the recipe panel uses the same bottle name as the cauldron, the plaque counts ingredients and set-aside cards the way the pot does, and a Check before any brew never reads Brew #0', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['compact-fit']);

  expect(report.varLabelWidths.length).toBeGreaterThan(0);
  for (const width of report.varLabelWidths) expect(width).toBeGreaterThanOrEqual(150);
  expect(report.errorSpill).toEqual([]);
  expect(report.rail).toEqual({ opened: true, collapsed: true });
  expect(report.grimoireGlyphs).toEqual([]);
  expect(report.recipeTitle).toBe('Send · orders');
  expect(report.stats).toEqual({ aside: 'Draft · 3 ingredients · 1 aside', relay: 'Draft · 2 ingredients' });
  expect(report.checkHeader).toMatch(/^Check/);
  expect(report.singularPlurals).toEqual([]);
});

test('Studio mends and guards the recipe around every step: pulling the model out keeps the reply fed, setting aside a Replace gives the reply its words back, a tossed Template is wired between the message and the reply, a bottle missing its words says so on the bottle, a single bottle has the potion colour, twelve ingredients pour down twelve separate strands that never cross the pour from pot to bottle, nothing changes mid-brew, and the card menu can set an ingredient aside and pour it back in', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['recipe-mending']);

  for (const pulled of [report.pullModel.aside, report.pullModel.deleted]) {
    expect(pulled).toEqual({ check: 'valid', contentFed: true });
  }
  expect(report.replaceAside.check).toBe('valid');
  expect(report.replaceAside.replyFeeders).toEqual(expect.arrayContaining([expect.stringMatching(/^on-message-\d+\.content->content$/)]));
  expect(report.templateToss.check).toBe('valid');
  expect(report.templateToss.role).toBe('Ingredient');
  expect(report.templateToss.templateFed).toBe(true);
  expect(report.templateToss.replyFeeders).toContain(report.templateToss.template + '.text->content');
  expect(report.bottlePill).toBe('Missing "content"');
  expect(report.tints.bottle).toBe(report.tints.potion);
  expect(report.midBrew).toEqual({ nodes: 0, vars: 0, stillSimmering: true });
  expect(report.dozenLanes).toEqual({ crossing: [], strands: 12, pours: 12 });
  expect(report.menu).toEqual({ setAside: true, afterAside: 'Aside', pourIn: true, afterPour: 'Ingredient' });
});

test('What Studio shows, brews and publishes is the same recipe: a spare reply stays on the shelf and out of the release while the draft keeps it, a reopened potion keeps its live release, new potions get their own names, setting aside a Switch never opens every route, undo brings back every wire, a wired Delay sits in the pot, a trigger dropped on the shelf never brews, and mid-brew the bottle stays empty and Publish waits', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-one-recipe-'));
  try {
    const report = await studioProbe(['one-recipe', join(dir, 'studio.sqlite')]);

    expect(report.spare).toEqual({ check: 'valid', role: 'Aside', releaseValid: true, releaseHasSpare: false, draftKeepsSpare: true });
    expect(report.reopened).toEqual({ name: 'Spare Agent', live: 1, releases: 1 });
    expect(report.newNames[0].saved).toBe(true);
    expect(report.newNames[1].saved).toBe(true);
    expect(report.newNames[0].name).not.toBe(report.newNames[1].name);
    expect(report.routeBridges).toBe(0);
    expect(report.join.after).toBe(report.join.before);
    expect(report.shelf).toEqual({ wiredDelay: 'Ingredient', looseSend: 'Aside' });
    expect(report.tray).toEqual({ role: 'Aside', brews: 0 });
    expect(report.midBrew.bottle).toBe('Ready');
    expect(report.midBrew.publish).toMatch(/until the brew ends/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Studio reads as one finished product: strands clear every card at 960, the empty ring sits below the mode switch, card lines show whole at 960, the log has one idle line, Publish is off mid-brew, messages are sentences about potions, Potions and release rows say each thing once, the theme button names the current theme, the card menu clears the plaque, and untaken Grimoire drops never cross', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-finish-'));
  try {
    const report = await studioProbe(['fit-and-wording', join(dir, 'studio.sqlite')]);

    expect(report.strandClearance).toBeGreaterThanOrEqual(6);
    for (const gap of report.ringBelowSwitch) expect(gap).toBeGreaterThanOrEqual(8);
    expect(report.formulasMissingAt960).toEqual([]);
    expect(report.formulasCutAt960).toEqual([]);
    expect(report.logs.empty).toContain('Not brewed yet');
    expect(report.logs.support).toContain('Not brewed yet');
    expect(report.logs.empty).not.toMatch(/Idle/);
    expect(report.midBrewPublishError).toBeNull();
    expect(report.noDbPublishError).toBe('Pass --db <file.sqlite> to publish releases on disk');
    expect(report.shelf).toMatch(/Potions/);
    expect(report.shelf).not.toMatch(/Spells/);
    expect(report.shelf).not.toMatch(/Published brew/);
    expect(report.shelf).toMatch(/v1 2 ingredients/);
    expect(report.theme).toBe('Sunlit');
    expect(report.menuCoversPlaque).toBe(false);
    expect(report.untakenLanes).toEqual({ 'default->ignored_reply': 1, 'support->support_reply': -1 });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('The shelf is kept and respected: a set-aside step survives save and reopen and pours back wired, publishing without saving keeps spares, a saved copy of a sample opens with its releases, adding a step never pulls a shelved trigger back, a shelved sub-spell passes Check, an unreachable reply is no bottle, pulling the model passes the message straight through, and a second trigger pours back connected', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-shelf-'));
  try {
    const report = await studioProbe(['shelf-keeps', join(dir, 'studio.sqlite')]);

    expect(report.asideReply).toBe('hi there');
    expect(report.reopenedRole).toBe('Aside');
    expect(report.poured.check).toBe('valid');
    expect(report.poured.echoWires).toBeGreaterThanOrEqual(2);
    expect(report.reopenedTrigger).toBe('Aside');
    expect(report.shelvedTriggerBrews).toBe(0);
    expect(report.publishedOnly).toEqual({ listed: true, spare: true, live: 1 });
    expect(report.sampleCopy).toEqual({ releases: 1, live: 1 });
    expect(report.triggerAfterAdd).toBe('Aside');
    expect(report.shelvedSubgraph).toBe('valid');
    expect(report.unreachableRole).toBe('Aside');
    expect(report.modelPulledReply).toBe('what is brewing');
    expect(report.secondRestored).toBe(true);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Set-aside steps come back exactly as they left: the shelf survives a restart and a reopen from Potions, pouring back drops the bypass so one message gets one reply, a template keeps its own named input, pouring one step leaves its shelved neighbour on the shelf, a shelved sub-spell with a bad version never blocks Check, a reply no flow reaches is no bottle, and the more-issues chip lists every Check finding with rows that open the ingredient', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-shelf-exact-'));
  try {
    const report = await studioProbe(['shelf-exact', join(dir, 'studio.sqlite')]);

    const back = { reply: ['fill.text->content', 'once.out->in'], fill: ['trigger.content->person'], said: 'Hi Ada' };
    expect(report.restart).toEqual({ once: 'Aside', fill: 'Aside', pouredBack: back });
    expect(report.fromPotions).toEqual({ once: 'Aside', pouredBack: back });
    expect(report.pairSecond).toBe('Aside');
    expect(report.shelvedBadVersion).toBe('valid');
    expect(report.muteReply).toBe('Aside');
    expect(report.moreIssues.chip).toBe(true);
    expect(report.moreIssues.total).toBeGreaterThan(2);
    expect(report.moreIssues.rows).toBe(report.moreIssues.total);
    expect(report.moreIssues.opensIngredient).toBe(true);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('The cauldron draws only what is true: potions leave through the tap, ribbons never cross cards or turn sharply, the plaque shows the brew total, zooming out stops while text stays readable, a routed brew reads 1 bottle filled, no legend chips, the shelf is a captionless plank, the plaque clears cards and shelf, a dozen keep both shelves after Check and Brew, Brew is disabled without a recipe trigger, an all-shelved pot goes cold, and the stand has no Test pill', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['cauldron-truths']);

  expect(report.offTap).toEqual([]);
  expect(report.sharpTurns).toEqual([]);
  for (const brew of report.plaqueTime) expect(brew.plaque).toContain(brew.total);
  expect(report.farScale).toBeGreaterThanOrEqual(0.6);
  expect(report.routedPlaque).toContain('1 bottle filled');
  expect(report.throughCards).toEqual([]);
  expect(report.legends).toEqual([false, false, false]);
  expect(report.shelfWords).toEqual([]);
  expect(report.plank).toBe(true);
  expect(report.plaqueHits).toEqual([]);
  expect(report.flankAfter).toEqual([true, true]);
  expect(report.brewDisabled).toBe(true);
  expect(report.emptyMood).toBe('Empty');
  expect(report.standHasTest).toBe(false);
});
test('Quiet corners tell the truth: saved potions count ingredients, a brew locks drags, menus, restores and Check, copy says variable and Script, undo puts a deleted shelved step back on the shelf, bad typed defaults are flagged, Json is a type, only unshelved and var steps claim a variable, drips stay on their stream, twin names reach the undo banner, moving a card keeps the brew lit, the Grimoire menu offers the Cauldron, Stream Text has a model, the Tasting divider fits at 960, an empty pot offers triggers', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-quiet-'));
  try {
    const report = await studioProbe(['quiet-corners', join(dir, 'studio.sqlite')]);

    expect(report.potionRow).toContain('3 ingredients · 1 aside');
    expect(report.potionRow.some((line: string) => line.endsWith('steps'))).toBe(false);
    expect(report.midBrew).toEqual({ running: true, modelMenu: false, typeMenu: null, restore: null, pantryDrag: false, tray: false, checkUnchanged: true, checkDisabled: true });
    expect(report.banner).toMatch(/^Removed variable /);
    expect(report.runeCaption).toBe(false);
    expect(report.undo.role).toBe('Aside');
    expect(report.undo.parked).toBeGreaterThan(0);
    expect(report.undo.wiresAfterPour).toBeGreaterThan(0);
    expect(report.variables).toEqual({ flagged: true, json: true, shelvedHint: false, getterHint: true });
    expect(report.dripDrift).toBeLessThanOrEqual(2);
    expect(report.twinBanner).toBe('Removed Template · 1');
    expect(report.litAfterMove).toBe(true);
    expect(report.grimoireMenu).toContain('Show in Cauldron');
    expect(report.grimoireMenu).not.toContain('Inspect in Grimoire');
    expect(report.streamModel).toBe(true);
    expect(report.divider.width).toBeLessThanOrEqual(report.divider.panel - 28);
    expect(report.starter).toEqual({ clicked: true, nodes: ['event/on_message'] });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Studio state stays honest: two shelved neighbours poured back in either order after a reopen give one reply, a reply no trigger flow reaches is no bottle, a bad typed variable fails Check and blocks Save and Publish while Json resets to no value, undo and parked sub-spells keep a potion live, mending a hex clears it, wiring a shelved step in the Grimoire drops its bypass and the Grimoire marks shelved steps, and the 960 prompt editor fits after Check', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-honest-'));
  try {
    const report = await studioProbe(['honest-state', join(dir, 'studio.sqlite')]);

    expect(report.shelvedReply).toBe('solo');
    for (const poured of report.orders) {
      expect(poured.flowIntoReply).toEqual(['second.out->in']);
      expect(poured.said).toBe('once');
    }
    expect(report.cutReply).toBe('Aside');
    expect(report.badDefault).toEqual({ check: 'invalid', save: null, published: 0, json: null });
    expect(report.live).toEqual({ afterUndo: 1, afterParkedSub: 1, duplicate: 0 });
    expect(report.mended).toEqual({ before: 'invalid', after: 'unchecked', flagged: 0 });
    expect(report.grimoire.reply).toEqual(['once.out->in']);
    expect(report.grimoire.badge).toBe(true);
    expect(report.editor.bottom).toBeLessThanOrEqual(report.editor.panel);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Every drawn line keeps to itself: crowded lanes never share a run, ribbons leave the tap level and skip their own bottle tag, no pour runs through another card on any sample at either size, status pills on mirrored cards clear the step badge, untaken and hexed outlines pass 3 to 1, split twins are numbered instead of showing socket ids, and a plain Brew of the webhook router brews', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['drawn-truths']);

  expect(report.sharedRuns).toEqual([]);
  expect(report.ownTag).toEqual([]);
  expect(report.spigot).toEqual([]);
  expect(report.pourCrossings).toEqual([]);
  expect(report.pills.found).toBeGreaterThan(0);
  expect(report.pills.hits).toEqual([]);
  for (const ratio of report.borders) expect(ratio).toBeGreaterThanOrEqual(3);
  expect(report.twinNames.some((title: string) => /out_\d/.test(title))).toBe(false);
  expect(report.twinNames.length).toBeGreaterThan(0);
  expect(report.hookBrew).toBe('Some(Succeeded)');
});
test('One voice for status: a published potion set aside and poured back still reads Published v1 in the header, plaque and bottle, a moved and saved potion reopens live and refuses a duplicate release, the Cauldron tells its counts only on the plaque while the Grimoire keeps its chip, and no bottle ribbon climbs far above its mouth', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-voice-'));
  try {
    const report = await studioProbe(['one-voice', join(dir, 'studio.sqlite')]);

    expect(report.roundTrip.header).toContain('Published v1');
    expect(report.roundTrip.plaque).toContain('Published v1');
    expect(report.roundTrip.bottle).toContain('v1');
    expect(report.liveAfterReopen).toBe(1);
    expect(report.releasesAfterReopen).toEqual([1]);
    expect(report.counts).toEqual({ toolbar: false, pantry: false, grimoire: true });
    expect(report.peaks).toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Studio keeps its promises: a prompt typed over a shelf shortcut stays typed after reopen, a sub-spell caller publishes once and reopens live, a chat route replies beside a webhook route, a mended number unlocks Brew and a bad one drops the seal, twin or blank variable names fail Check, a lone trigger stays while an unfed reply is set aside, restoring the live release keeps it live, and hand-drawn Grimoire wires drop the shortcut and say why one was refused', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-promises-'));
  try {
    const report = await studioProbe(['kept-promises', join(dir, 'studio.sqlite')]);

    expect(report.typed).toEqual({ bridged: true, wired: false, prompt: 'typed words' });
    expect(report.subSpell).toEqual({ releases: [1], liveAfterReopen: 1 });
    expect(JSON.stringify(report.twoDoors.replies)).toContain(report.twoDoors.sent);
    expect(report.number.liveBefore).toBe(1);
    expect(report.number.whileBad).toEqual({ live: null, dirty: true, badge: null });
    expect(report.number.hexed).toBe('invalid');
    expect(report.number.after).toBe('unchecked');
    expect(report.number.brewLocked).toBe(false);
    expect(report.names.duplicate).toBe('invalid');
    expect(report.names.blank).toBe('invalid');
    expect(report.names.typeKeys).toBe(report.names.rows);
    expect(report.lone).toEqual({ trigger: 'Ingredient', reply: 'Aside', check: 'valid' });
    expect(report.restored).toEqual({ live: 1, badge: 'v1' });
    expect(report.liveDrag).toEqual({ flowIntoReply: ['once.out->in'], refusalShown: true });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});test('Everything stays in plain sight: bottle ribbons never run on top of each other, the log keeps a short result in view and scrolls its full events, a failed step keeps Type it instead above the fold at 960, step names in the brew summary are never cut, the Recipe tab does not repeat the Trace, a failed brew marks nothing Untaken and no pill names a socket, every ingredient outline passes 3 to 1, crowded vials keep their words inside, and an empty pot cannot be sent a message', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['plain-sight']);

  expect(report.ribbonShares).toEqual([]);
  for (const log of report.logs) {
    expect(log.resultVisible).toBe(true);
    expect(log.clipped).toEqual([]);
  }
  expect(report.failedInspector.typeInsteadInView).toBe(true);
  expect(report.summary).toEqual({ cut: [], recipeRepeatsTrace: false });
  expect(report.untakenAfterFail).toEqual([]);
  expect(report.socketSuffixes).toEqual([]);
  expect(report.pillOverlaps).toEqual([]);
  expect(report.weakBorders).toEqual([]);
  for (const ratio of report.broth) expect(ratio).toBeGreaterThanOrEqual(3);
  expect(report.spilling).toEqual([]);
  expect(report.chatOnEmpty).toBe(0);
  expect(report.menuHeaderClash).toBe(false);
});test('Close quarters stay readable: the 960 prompt source shows its whole name, bottle ribbons keep a stream width clear of their own tag and badge, a crowded pot keeps equal room above and below, a setting typed as 5.0 no longer matches a release saved as 5 because steps read whole and decimal numbers differently, and the card menu stays tied to the card it opened from', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-quarters-'));
  try {
    const report = await studioProbe(['close-quarters', join(dir, 'studio.sqlite')]);

    expect(report.promptSource).toEqual({ text: 'From On Message · content', whole: true, inside: true });
    expect(report.ribbonCrowding).toEqual([]);
    expect(Math.abs(report.balance.above - report.balance.below)).toBeLessThanOrEqual(8);
    expect(report.sameNumber).toEqual({ live: null, unchanged: false });
    expect(report.menuAttached).toBe(true);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});test('What you see is what runs: a bottle added to the router stays out of routes it cannot feed, a child potion with a spare reply set aside passes its caller Check, undoing a deleted trigger keeps its place, undo keeps a refused shortcut refused, Check keeps a failed brew until an edit, Tasting never fires a webhook, a reply with no trigger waits aside, an unpinned sub-spell caller stays live after Save, undo never doubles a socket, and timer and variable triggers brew with sample words', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-runs-'));
  try {
    const report = await studioProbe(['true-runs', join(dir, 'studio.sqlite')]);

    expect(report.addedBottle.failed).toEqual([]);
    expect(report.addedBottle.status).toBe('Succeeded');
    expect(report.spareChild).toBe('valid');
    expect(report.undoTrigger).toEqual({ sameOrder: true, live: 1 });
    expect(report.undoRefusalWired).toBe(false);
    expect(report.checkAfter.staleLogSaysBrewed).toBe(false);
    expect(report.checkAfter.staleLogSaysHexed).toBe(true);
    expect(report.checkAfter.failedPill).toContain('Failed');
    expect(report.checkAfter.failedPlaque).toContain('Failed');
    expect(report.hookChat).toBe(0);
    expect(report.loneReply).toBe('Aside');
    expect(report.unpinnedSeal).toEqual({ afterSave: 1, releases: [1, 2] });
    expect(report.replyContentWires).toBe(1);
    expect(report.samples).toEqual({ tickEmpty: false, changeEmpty: false });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});test('Every signal is clear: a failed step names its real cause while finished pours stay lit, 960 model tiles never slice, the 960 Grimoire keeps the potion name, a wrong-type wire says so, bad variable names are marked on rows and plaque, stacked ribbons never cross and dim on a hex, long route names stay inside, Structured JSON shows its model, temperature changes one step, the ladle never touches a pour yet stays in an everyday pot, and a wire can be pulled off an input', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['clear-signals']);

  expect(report.failure).toEqual({ headlineHasCause: true, traceHasCause: true, dimmedFinishedPours: [] });
  expect(report.tileWhole).toBe(true);
  expect(report.grimoireTitleWhole).toBe(true);
  expect(report.wrongHeadline).not.toContain('needs "content"');
  expect(report.wrongHeadline).toContain('Int');
  expect(report.nameErrors).toEqual(['Duplicate name', 'Needs a name']);
  expect(report.namePlaque).not.toBe('Hexed');
  expect(report.stackedCrossings).toEqual([]);
  expect(report.stackedLanes).toEqual([]);
  expect(report.branchMoods).toEqual(['Hexed', 'Hexed']);
  expect(report.longSpilled).toEqual([]);
  expect(report.longResultInside).toBe(true);
  expect(report.structuredModel).toBe(true);
  expect(report.temperature).toEqual({ second: 0.4, variable: 'Float(0.4)' });
  expect(report.ladleCrossed).toBe(false);
  expect(report.ladleBrushes).toBe(false);
  expect(report.everydayLadle).toBe(true);
  expect(report.pulledWires).toBe(0);
});test('Every dial tells the truth: a step with no temperature reads Auto and the first press writes one, the model tile names its provider properly, only real variable links are shown and a changed default changes the brew, the 960 Grimoire names every socket at fit or on hover, step badges match the log after a brew, lane pours turn on wide curves, the first log chip keeps its name, the header counts bottles, big whole numbers keep their meaning, and a missing AI gateway says where to set one', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-dials-'));
  try {
    const report = await studioProbe(['honest-dials', join(dir, 'studio.sqlite')]);

    expect(report.temperature).toEqual({ shown: 'Auto', written: 0.8, providerShown: true });
    expect(report.bindings.invented).toEqual([]);
    expect(report.bindings.greetingLinked).toBe(true);
    expect(report.bindings.first).toEqual(['hi friend']);
    expect(report.bindings.second).toEqual(['hey friend']);
    if (report.compactGrimoire.zoom < 0.42) expect(report.compactGrimoire.hoverTip).toBe('body');
    else expect(report.compactGrimoire.socketNames).toBeGreaterThan(0);
    expect(report.badgesSeen).toBeGreaterThanOrEqual(4);
    expect(report.badgeMismatch).toEqual([]);
    expect(report.laneRadius).toBeGreaterThanOrEqual(16);
    expect(report.firstChipNamed).toBe(true);
    expect(report.headerSaysBottles).toBe(true);
    expect(report.bigNumbersLive).toBeNull();

    const env: NodeJS.ProcessEnv = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '' };
    delete env.MAGICKLALA_LLM_BASE_URL;
    delete env.MAGICKLALA_LLM_API_KEY;
    delete env.OPENAI_API_KEY;
    const { stdout } = await run('cargo', ['run', '-q', '-p', 'magicklala-studio', '--example', 'studio_probe', '--', 'gateway-hint'], { cwd: repoRoot, env, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
    const hint = JSON.parse(stdout.trim().split('\n').at(-1) ?? '');
    expect(hint.error).toContain('MAGICKLALA_LLM_BASE_URL');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});test('A brew locks the recipe and the bottles tell the truth: typing, saving, adding, dragging cards and drawing wires all wait until the brew ends and the lit result survives, branch bottles show no step numbers before a brew, a running bottle simmers without a check, a published branch potion shows its sealed version on the bottle, and thirteen-step and thirty-eight-step brews and a many-issue Check keep every log chip and the result inside the log at 960 and 1280', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-locks-'));
  try {
    const report = await studioProbe(['brew-locks', join(dir, 'studio.sqlite')]);

    expect(report.midBrew).toEqual({ promptUnchanged: true, saveError: null, added: 0, moved: false, wiresAdded: 0, litAfterBrew: true });
    expect(report.idleBadges).toEqual([]);
    expect(report.runningBottle).toContain('Simmering');
    expect(report.runningBottle.some((line: string) => /ms$/.test(line))).toBe(false);
    expect(report.sealedBottle).toContain('v1');
    expect(report.logs.map((log: { steps: number }) => log.steps)).toEqual(expect.arrayContaining([38]));
    for (const log of report.logs) {
      expect(log.resultVisible).toBe(true);
      expect(log.clipped).toEqual([]);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Every pour, name and count reads true: twelve ingredients land on twelve rim spots with no crossings, low vials pour over the rim, bottle ribbons arc clear of the pot, running sockets pass 3 to 1, compact tabs keep their words, step badges never cover names, twin steps carry numbered full names everywhere, set-aside steps lose their pantry mark, potion and release rows count real ingredients, a lone trigger reads Brewed, and Grimoire cards at half zoom never cut titles or stack labels', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-polish-'));
  try {
    const report = await studioProbe(['geometry-and-polish', join(dir, 'studio.sqlite')]);

    expect(report.dozen).toEqual({ landings: 12, pours: 12, ribbonCrossings: 0, pourCrossings: 0, lanes: false });
    expect(report.throughBelly).toEqual([]);
    expect(report.ribbonThroughPot).toBe(0);
    for (const ratio of report.runningContrast) expect(ratio).toBeGreaterThanOrEqual(3);
    for (const ratio of report.sunContrast) expect(ratio).toBeGreaterThanOrEqual(3);
    for (const ratio of report.markerContrast) expect(ratio).toBeGreaterThanOrEqual(3);
    expect(report.summaryContrast).toBeGreaterThanOrEqual(4.5);
    expect(report.tabs.gap).toBeGreaterThanOrEqual(6);
    expect(report.tabs.wordsFit).toBe(true);
    expect(report.badges.found).toBeGreaterThan(0);
    expect(report.badges.overlaps).toEqual([]);
    expect(report.names).toEqual({ vials: ['Delay · 1', 'Delay · 2'], triggers: ['On Message · 1', 'On Message · 2'], menu: 'Delay · 2' });
    expect(report.pantryMarksAside).toBe(false);
    expect(report.pantryMarksInPot).toBe(true);
    expect(report.potionRow).toMatch(/3 ingredients · 1 aside/);
    expect(report.releaseRows).toMatch(/v1 4 ingredients/);
    expect(report.loneChip).toMatch(/Brewed/);
    expect(report.grimoireAtHalf).toEqual([]);
    expect(report.grimoireNamesAt64).toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('The workbench tells the truth: undo shows after a brew and for a lone card, the demo has no ignored settings, variable steps pick their variable and Check flags a blank one, a new Rhai step brews, clicking a Grimoire input keeps its wire and a pulled shortcut stays pulled after reopen, embeddings ask for Text and a blank prompt fails Check, a mend only offers a source on the same route, a bottle beside a word loop feeds from the trigger, and changing the recipe clears old Brewed and Checked marks', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-truths-'));
  try {
    const report = await studioProbe(['workbench-truths', join(dir, 'studio.sqlite')]);

    expect(report.undo).toEqual({ afterBrewShown: true, restored: true, unwiredAsideUndoable: true });
    expect(report.deadKeys).toEqual([]);
    expect(report.variables).toEqual({ seeded: true, nameField: true, blankCheck: 'invalid' });
    expect(report.rhai.status).toBe('Succeeded');
    expect(report.rhai.failed).toEqual([]);
    expect(report.pull).toEqual({ clickKeepsWire: true, pulledStaysPulled: true });
    expect(report.fields).toEqual({ embedCaption: true, blankPromptCheck: 'invalid' });
    expect(report.mendOffer).toBe('chat');
    expect(report.loopBottle).toEqual({ role: 'Bottle', wordsFrom: 'trigger' });
    expect(report.staleMarks.shelvedLogSaysBrewed).toBe(false);
    expect(report.staleMarks.logSaysChecked).toBe(false);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Hands on the workbench: Escape clears a pick and closes a menu, Delete removes a picked step and Cmd or Ctrl Z brings it back, Add is greyed while a brew runs, a duplicate of a set aside card stays aside and keeps its extra sockets, a bottle can be set aside from its menu, the menu says Duplicate without the long name, and no card menu covers the pot at 1280 or 960', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['hands-on']);

  expect(report.keyboard).toEqual({ escapeClears: true, escapeClosesMenu: true, deleteRemoves: true, commandZRestores: true });
  expect(report.addDuringBrew).toEqual({ running: true, disabled: true });
  expect(report.duplicate).toEqual({ asideCopyStaysAside: true, copyHasContent: true });
  expect(report.bottle).toEqual({ caption: ['Duplicate'], offered: true, aside: true });
  expect(report.menuCoversPot).toEqual([]);
});
test('Crowded shelves stay untangled and readable: valid potions with four, eight, or twelve steps and fourteen bottles, at 1280 and at 960 with the Pantry open, pour through one shared outlet that splits in clear gaps, no ribbon crosses another or runs through a card, and every card label stays at least 12px tall inside its card', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['crowded-bottles']);

  expect(report.map((shelf: { case: string }) => shelf.case)).toEqual(['4@1280', '4@960', '8@1280', '8@960', 'kitchen@1280', 'kitchen@960']);
  for (const shelf of report) {
    expect({ case: shelf.case, check: shelf.check, crossings: shelf.crossings, through: shelf.through, outlets: shelf.outlets, spills: shelf.spills }).toEqual({ case: shelf.case, check: 'valid', crossings: [], through: [], outlets: 1, spills: [] });
    expect(shelf.smallestLabel).toBeGreaterThanOrEqual(12);
  }
});
test('Every edit agrees with Check: pulling a Grimoire wire off or onto a refused socket ends the pull and keeps or drops the wire honestly, Publish refuses a recipe Check would reject, undoing a deleted sub-spell call keeps its link and brews, adding a variable drops the old seal and brew marks, the Grimoire names every socket at 45% without stacking, and every potion row shows a bottle', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-agreed-'));
  try {
    const report = await studioProbe(['agreed-edits', join(dir, 'studio.sqlite')]);

    expect(report.pulledOff).toEqual({ before: 'trigger.content', after: null, dragIdle: true });
    expect(report.refusedDrop).toEqual({ after: 'trigger.content', dragIdle: true });
    expect(report.blankPublish).toEqual({ published: false, releases: [] });
    expect(report.undoLink).toEqual({ before: 'Succeeded', link: { child: true, version: 1 }, after: 'Succeeded' });
    expect(report.variableMarks).toEqual({ liveBefore: 1, liveAfter: null, badge: null, lastBrew: null });
    expect(report.grimoire45.shown).toBe(report.grimoire45.expected);
    expect(report.grimoire45.stacked).toEqual([]);
    expect(report.unbottledPotions).toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Steady hands: a card menu opens beside its card clear of the pot and other cards, its thread meets the facing edges at 1280, 960 and from the shelf, Escape closes the open menu first, Delete only acts on the workbench tabs, typing keeps its keys, Check lists every problem at once, a cleared variable name drops its old link, and twin titles never strand their number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['steady-hands']);

  expect(report.menus.wide).toEqual({ coversPot: false, hides: [], attached: true });
  expect(report.menus.aside).toEqual({ coversPot: false, hides: [], attached: true });
  expect(report.menus.compact.coversPot).toBe(false);
  expect(report.menus.compact.hides).toEqual([]);
  expect(report.keys).toEqual({ escapeClosesModelMenu: true, escapeClosesTypeMenu: true, deleteOnVariablesKeepsStep: true, typingKeepsStep: true });
  expect(report.findings).toEqual({ variableNamed: true, replyFlagged: true });
  expect(report.hintBefore).toBe('→ Get Variable');
  expect(report.staleHint).toBe(false);
  expect(report.orphans).toEqual([]);
});
test('Fair shelves: a short recipe with four or eight bottles keeps the pot centred instead of stacking, bottle ribbons turn on wide curves, the zoomed-out socket tip floats above every card inside the canvas, bottles show their kind the same way at 960, hex pills stay inside right-hand cards, and prompts never seed words nothing fills while a literal word in an AI prompt gets a note instead of a hex', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['fair-shelves']);

  expect(report.layouts).toEqual([{ case: '4', stacked: false }, { case: '8', stacked: false }]);
  expect(report.bends).toEqual([]);
  expect(report.tooltip).toEqual({ shown: true, inside: true, onTop: true });
  expect(report.bottleKinds.dozen).toBe(report.bottleKinds.support);
  expect(report.pillInside).toBe(true);
  expect(report.prompts).toEqual({ seededTokens: 0, unfilledCheck: 'valid', namesWord: true, filledCheck: 'valid' });
});
test('The last mile: a refused Publish lists every problem Check lists, adding a variable can be undone, Cmd or Ctrl Z waits out a brew, and an empty pot offers its triggers on the shelf beside it instead of repeating the Pantry in the Recipe panel', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-last-mile-'));
  try {
    const report = await studioProbe(['last-mile', join(dir, 'studio.sqlite')]);

    expect(report.publish).toEqual({ published: false, publishCount: 2, same: true, checkCount: 2 });
    expect(report.variableUndo).toEqual({ added: 1, afterUndo: 0 });
    expect(report.undoDuringBrewKept).toBe(true);
    expect(report.emptyPot).toEqual({ inPot: 6, inPanel: 0, starterOnShelf: true });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Honest edits: a second bottle on a Branch takes the free way, a cut shortcut stays cut, the layout follows a resized window, a refused wire drop changes nothing, a refused Publish marks the steps Check marks, setting aside the last step clears old brew marks, a variable type change is undoable and unseals Check, the Research sample checks clean, each potion shows its own mood, Tasting explains a hexed brew, Cmd or Ctrl Z works on the Variables tab, and Check names an undeclared variable', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-honest-edits-'));
  try {
    const report = await studioProbe(['honest-edits', join(dir, 'studio.sqlite')]);

    expect(report.branchOuts).toEqual(['false', 'true']);
    expect(report.cut).toEqual({ bridged: true, cameBack: false });
    expect(report.resize).toEqual({ size: [960, 640], compact: true });
    expect(report.refusedDrop.sameOrder).toBe(true);
    expect(report.refusedDrop.dirtyAfter).toBe(report.refusedDrop.dirtyBefore);
    expect(report.refusedDrop.brewKept).toBe(true);
    expect(report.refusedDrop.storeError).toBeNull();
    expect(report.refusedDrop.wireError).toBe(true);
    expect(report.refusedDrop.wireErrorShown).toBe(true);
    expect(report.publishMarks).toEqual({ failed: ['echo'], sameAsCheck: true, rejected: 1, debugIds: false });
    expect(report.loneAside).toEqual({ brewed: true, marksAfterAside: false });
    expect(report.variableType).toEqual({ checked: 'valid', afterType: 'unchecked', type: 'Int', undone: 'String' });
    expect(report.research.validation).toBe('valid');
    expect(report.moods).toEqual({ hexed: 'Hexed', other: 'Ready' });
    expect(report.tastingReason).toBe(true);
    expect(report.undoOnVariables).toBe(1);
    expect(report.undeclaredNamed).toBe(true);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Legible shelves: in crowded, brewed, eight-bottle, compact and zoomed-out cauldrons every word on the canvas stays at 10.5px or larger, every ingredient keeps its vial, and the pot never shrinks below 140px', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['legible-shelves']);

  expect(report.reference).toBe(15);
  for (const [scene, measured] of Object.entries<any>(report.cases)) {
    expect({ scene, tooSmall: measured.tooSmall }).toEqual({ scene, tooSmall: [] });
    expect({ scene, vialless: measured.vialless }).toEqual({ scene, vialless: 0 });
    expect(measured.potWidth, scene).toBeGreaterThanOrEqual(140);
  }
});
test('Clear sight: the card menu at 960 by 640 parks in open canvas instead of burying the other ingredients, docks in the Recipe pane with working actions when no open spot exists, and Grimoire card headers keep every word inside from 60% to 100% zoom', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['clear-sight']);

  expect(report.menuCovers).toEqual([]);
  expect(report.headerSpills).toEqual([]);
  expect(report.docked).toEqual({ inRecipePane: true, hiddenByMenu: [], duplicateWorks: true });
});
test('Mixed shelves: six or eight ingredients with two to six bottles never hide a card behind another in a back row, and no pour, free or in a lane between columns, runs behind a card or its step number on its way to the rim', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['mixed-shelves']);

  expect(report.length).toBe(16);
  for (const shelf of report) {
    expect({ case: shelf.case, shape: shelf.shape === 'back-row ring' ? shelf.shape : 'ok' }).toEqual({ case: shelf.case, shape: 'ok' });
    expect({ case: shelf.case, through: shelf.through }).toEqual({ case: shelf.case, through: [] });
  }
});
test('Undo keeps time in order and reaches every edit: Cmd or Ctrl Z first takes back a new wire and then the older removed step, a typed prompt undoes as one step before the variable added earlier, a Switch case list undoes to its old outputs, and undo right after a removal still brings the step back', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['undo-order']);

  expect(report.removed).toBe(true);
  expect(report.wireUndone).toBe(true);
  expect(report.stepKeptAfterFirst).toBe(true);
  expect(report.stepBackAfterSecond).toBe(true);
  expect(report.undoAfterTracked).toBe(true);
  expect(report.typed).toEqual({ promptAfterOne: 'You asked: {{content}}', variablesAfterOne: 1, variablesAfterTwo: 0 });
  expect(report.listed).toEqual({ before: ['refund', 'billing', 'default'], after: ['default'] });
});
test('Potion rack: four or more bottles stand in rows on shelf planks fed by one glass pipe per row instead of a branch per bottle, crowded ingredient names stay whole, every bottle name keeps one line, no unowned kind caption floats under the shelves, pipes keep air above the tags below them, and each plank ends at its own bottles', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['potion-rack']);

  for (const [scene, rack] of Object.entries<any>(report)) {
    expect({ scene, cut: rack.cut }).toEqual({ scene, cut: [] });
    expect({ scene, rack: rack.rack }).toEqual({ scene, rack: true });
    expect({ scene, pipes: rack.pipes }).toEqual({ scene, pipes: rack.rows });
    expect(rack.rows, scene).toBeLessThanOrEqual(3);
    expect({ scene, wrapped: rack.wrapped }).toEqual({ scene, wrapped: [] });
    expect({ scene, kindShown: rack.kindShown }).toEqual({ scene, kindShown: false });
    expect({ scene, grazes: rack.grazes }).toEqual({ scene, grazes: [] });
    expect({ scene, planks: rack.overhangingPlanks }).toEqual({ scene, planks: [] });
  }
});
test('True records: Check keeps a good brew lit, an AI prompt may say {{customer}} literally with a note instead of a hex, a Send that hands off is still a bottle, only steps that ran wear step numbers after a brew, a pantry toss undoes as one step, an empty draft with variables saves and asks before you leave it, and a wire message does not follow you to the next potion', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-true-records-'));
  try {
    const report = await studioProbe(['true-records', join(dir, 'studio.sqlite')]);

    expect(report.checkKeepsBrew).toEqual({ brew: 'Succeeded', stale: false });
    expect(report.literalBraces).toEqual({ check: 'valid', noted: true });
    expect(report.replies).toEqual(['first', 'second']);
    expect(report.stepNumbers).toEqual({ duplicates: 0, untakenNumbered: [] });
    expect(report.tossUndo).toEqual({ added: true, removedByUndo: true, variables: 1 });
    expect(report.emptyDraft).toEqual({ saved: true, asked: true, kept: 2 });
    expect(report.wireMessageLeft).toBe(false);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Tidy bottles: after a brew no step number covers a bottle name, no label on a bottle overlaps another, and a bottle shows its kind only when the whole kind fits', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['tidy-bottles']);

  for (const [scene, bottles] of Object.entries<any>(report)) {
    expect({ scene, clashes: bottles.clashes }).toEqual({ scene, clashes: [] });
    expect({ scene, cutKinds: bottles.cutKinds }).toEqual({ scene, cutKinds: [] });
  }
});
test('Starter shelf: an empty pot offers its glowing trigger vials only while the Pantry is closed, since the open Pantry already lists them, and the vials never cover the pot', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['starter-shelf']);

  expect(report.wideWithPantry).toEqual({ pantryOpen: true, starters: 0, coverPot: false });
  expect(report.compactWithPantry).toEqual({ pantryOpen: true, starters: 0, coverPot: false });
  expect(report.wideWithPotions).toEqual({ pantryOpen: false, starters: 6, coverPot: false });
  expect(report.compactClosed).toEqual({ pantryOpen: false, starters: 6, coverPot: false });
});
test('Wire records survive the shelf: setting a step aside and pouring it back keeps every original wire id in its original place, in one session and after Save and reopen', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-wire-records-'));
  try {
    const report = await studioProbe(['wire-records', join(dir, 'studio.sqlite')]);

    expect(report).toEqual({ sameSession: true, saved: true, afterReopen: true, missing: [] });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Grimoire room: the default Support and Parallel Research potions leave a wide channel between every wired pair of cards so no wire loops back behind a card, and twin names keep their number when a title is shortened', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['grimoire-room']);

  for (const [potion, room] of Object.entries<any>(report)) {
    expect({ potion, tight: room.tight }).toEqual({ potion, tight: [] });
    expect({ potion, twinsCut: room.twinsCut }).toEqual({ potion, twinsCut: [] });
  }
});
test('Log counts: a brew where one template feeds eight bottles counts each step once in the Cauldron Log, matching the ten steps on the cauldron', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['log-counts']);

  expect(report).toEqual({ distinct: 10, saysSteps: true });
});

test('Pantry steps that need a list can be set up: a Switch takes its cases as plain words, grows one output per case before default, routes a matching message down its case and anything else to default, keeps its cases after reopening, drops an unwired case when it is removed, and Destructure JSON grows one output per key', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['pantry-settings']);

  expect(report.fieldShown).toBe(true);
  expect(report.typedOutputs).toEqual(['refund', 'billing', 'default']);
  expect(report.saved).toEqual(['refund', 'billing']);
  expect(report.check).toBe('valid');
  expect(report.refundReply).toBe('Refunds desk');
  expect(report.otherReply).toBe('Front desk');
  expect(report.reopenedOutputs).toEqual(['refund', 'billing', 'default']);
  expect(report.reopenedText).toBe('refund, billing');
  expect(report.trimmedOutputs).toEqual(['refund', 'default']);
  expect(report.keysField).toBe(true);
  expect(report.keyOutputs).toEqual(['name', 'age']);
  expect(report.keyConfig).toEqual(['name', 'age']);
});

test('An empty pot shows its recipe card in the Recipe tab, naming the potion, counting nothing yet and listing the empty pot and bottle sections, instead of a lone heading over blank space', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['empty-recipe']);

  expect(report.placeholder).toBe(false);
  expect(report.sections).toBe(true);
  expect(report.texts).toEqual(expect.arrayContaining(['Fresh Potion', '0 ingredients · 0 bottles', 'In the pot', 'Bottles']));
  expect(report.texts.filter((line: string) => line === 'None yet')).toHaveLength(2);
});

test('A published potion keeps saying which version is live in the Potions list after another potion is opened', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-seals-'));
  try {
    const report = await studioProbe(['shelf-seals', join(dir, 'studio.sqlite')]);

    expect(report.openSpell).toBe('Research Agent');
    expect(report.publishedRow).toContain('v1 live');
    expect(report.draftRow).not.toContain('live');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('List settings stay true: an imported case with a comma survives Check and shows quoted, the boxes are labelled Cases, Outputs and Keys, an empty case list is caught by Check, a Split shows only the outputs it lists, and a case still wired after it is removed is named by Check', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['list-truth']);

  expect(report.kept).toEqual({ check: 'valid', cases: ['north,west', 'south'], shown: '"north,west", south', outputs: ['north,west', 'south', 'default'] });
  expect(report.captions).toEqual({ switch: 'Cases', split: 'Outputs', pick: 'Keys' });
  expect(report.blank.check).toBe('invalid');
  expect(report.blank.errors.join(' ')).toMatch(/needs cases/);
  expect(report.split).toEqual({ default: 'out_0, out_1', outputs: ['left', 'right'] });
  expect(report.stale.check).toBe('invalid');
  expect(report.stale.errors.join(' ')).toMatch(/billing/);
  expect(report.stale.outputs).toEqual(['refund', 'billing', 'default']);
});

test('Check and Save tell the truth: a draft holding only a variable saves from the toolbar and asks before you leave once it is emptied again, Check reads placeholders the way a brew does, and Check with no edits keeps a failed brew on screen', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-check-save-'));
  try {
    const report = await studioProbe(['check-save-truth', join(dir, 'studio.sqlite')]);

    expect(report.draft).toEqual({ saved: true, askedAfterEmptying: true });
    expect(report.literal).toEqual({ check: 'valid', errors: [] });
    expect(report.failedBrew).toEqual({ before: 'Some(Failed)', after: 'Some(Failed)', failedCards: 1, stale: false });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('One action is one undo: a Send Message tossed into an empty pot and set aside on its own, a duplicated step with its settings, and a step poured back from the shelf each go away with a single Cmd or Ctrl Z', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['one-step-undo']);

  expect(report.toss).toEqual({ added: 1, entries: 1, afterUndo: 0 });
  expect(report.duplicate).toEqual({ after: 4, entries: 1, afterUndo: 3 });
  expect(report.pour).toEqual({ poured: true, shelvedAfterUndo: true });
});

test('Wires come home in their own place: setting aside two steps and pouring them back in turn leaves every wire with its original id and order, and undoing a set-aside does the same', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['wire-return']);

  expect(report.poured).toEqual(report.original);
  expect(report.pouredMatches).toBe(true);
  expect(report.undoMatches).toBe(true);
});

test('Every Pantry name shows whole at 960 and 1280: a long name wraps onto a second line instead of ending in an ellipsis', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['pantry-whole']);

  for (const width of ['960', '1280']) {
    expect(report[width].seen).toBeGreaterThan(30);
    expect({ width, cut: report[width].cut }).toEqual({ width, cut: [] });
  }
});

test('An empty pot offers only things you can press: no lone heading, the triggers glow in the Pantry when it is open and stand beside the pot as starters when it is not, never both', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['empty-start']);

  expect(report['wide-pantry']).toEqual({ inertHeading: false, offered: 6, glowing: 6, starters: 0, pantryRows: 6 });
  expect(report['wide-potions']).toEqual({ inertHeading: false, offered: 6, glowing: 0, starters: 6, pantryRows: 0 });
  expect(report.compact).toEqual({ inertHeading: false, offered: 6, glowing: 0, starters: 6, pantryRows: 0 });
});

test('Potions flow downhill: in every everyday layout at 1280 and 960 no bottle neck sits above the cauldron tap, two or three bottles read in recipe order left to right then top to bottom, and no ingredient kind line is cut', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['downhill']);

  expect(report.uphill).toEqual([]);
  expect(report.misordered).toEqual([]);
  expect(report.cutKinds).toEqual([]);
});

test('A failed pour aims at the pot: the broken stub of a failed step runs on until it dives toward the cauldron or reaches its rim, and its cross never lands on a neighbouring live pour, at 960 and 1280', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['failed-stub']);

  expect(report.upward).toEqual([]);
  expect(report.crowding).toEqual([]);
  expect(report.hanging).toEqual([]);
});

test('Variables can drive step settings: a setting written as {{name}} brews with that variable, Check names one that no variable fills, the Support sample binds Search Memory and Generate Text to its variables so none reads Unused, and the temperature dial on a bound step edits the variable', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['bound-settings']);

  expect(report.quick).toEqual({ check: 'valid', outcome: 'Succeeded', waitedForVariable: true });
  expect(report.unknown.check).toBe('invalid');
  expect(report.unknown.errors.join(' ')).toMatch(/no variable is named nope/);
  expect(report.hints).toEqual([['max_memories', '→ Search Memory'], ['temperature', '→ Generate Text']]);
  expect(report.dial).toEqual({ shown: 0.4, variable: 'Float(0.5)', config: '{{temperature}}' });
});

test('Two bottles fed by the same route get their own numbered names while a lone route keeps its plain name', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['twin-routes']);

  expect(report.names).toEqual(['Send · billing 1', 'Send · billing 2', 'Send · default']);
});

test('Small things stay honest: the Pantry search Clear button is at least 24 by 24, a Switch case that reuses the name default is named by Check, and undoing an older add keeps a card you moved since where you put it', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['small-truths']);

  expect(report.clear[0]).toBeGreaterThanOrEqual(24);
  expect(report.clear[1]).toBeGreaterThanOrEqual(24);
  expect(report.clash.check).toBe('invalid');
  expect(report.clash.errors.join(' ')).toMatch(/default is already an output/);
  expect(report.undoKeepsMove).toEqual({ added: true, addUndone: true, replyStays: true });
});

test('Cauldron cards say what they are set to instead of their code name: Generate Text shows its model, Search Memory how many it recalls, a Switch how many cases it routes, while the Grimoire keeps its own headers', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['card-notes']);

  expect(report.llm).toContain('gpt-4o-mini');
  expect(report.llm).not.toContain('ai/generate_text');
  expect(report.recall).toContain('top 5 · max_memories');
  expect(report.switch).toContain('2 cases');
  expect(report.grimoireShowsNote).toBe(false);
});

test('Variable edits undo and rename cleanly: a bound temperature press, a typed default and a typed rename each undo with one Cmd or Ctrl Z, and a rename carries every {{name}} that uses it so the step stays bound and Check stays clean', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['var-undo']);

  expect(report.dial).toEqual({ after: 'Float(0.5)', undone: 'Float(0.4)' });
  expect(report.typed).toEqual({ after: 'Int(12)', undone: 'Int(5)' });
  expect(report.rename.after).toEqual({ name: 'recall_count', limit: '{{recall_count}}', hint: '→ Search Memory' });
  expect(report.rename.check).toBe('valid');
  expect(report.rename.undone).toEqual({ name: 'max_memories', limit: '{{max_memories}}' });
});

test('Bound settings say where they come from: the temperature tile shows its bound look without repeating a variable named like the field, a card bound to a variable shows its value and the variable, and an open published potion says Live once with its version on the pill', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-bound-looks-'));
  try {
    const report = await studioProbe(['bound-looks', join(dir, 'studio.sqlite')]);

    expect(report.tile).toContain('Temperature');
    expect(report.tile).not.toContain('Temperature · temperature');
    expect(report.tileCut).toEqual([]);
    expect(report.recall).toBe('top 5 · max_memories');
    expect(report.activeRow).toContain('v1 Live');
    expect(report.activeRow.filter((line: string) => /live/i.test(line))).toEqual(['v1 Live']);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Redo brings back what undo took: Cmd or Ctrl Shift Z re-applies an undone removal, undo takes it back again, and a new edit after undo clears redo so it never overwrites fresh work', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['redo-order']);

  expect(report).toEqual({ removed: true, undone: true, redone: true, undoneAgain: true, freshKept: 'Fresh: {{content}}', echoAfterStaleRedo: true });
});

test('Parked wires survive undo and reload: undoing a pour-back puts the step back on the shelf with all four parked wires so pouring again rebuilds the original recipe, and a potion saved with two steps aside reopens and pours back in the original wire order', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['aside-undo-pour']);

  expect(report.parkedAside).toBe(4);
  expect(report.pouredMatches).toBe(true);
  expect(report.undone).toEqual({ shelved: true, parked: 4 });
  expect(report.repoured).toEqual(report.original);
  expect(report.reloaded).toEqual(report.twoOriginal);
});

test('Each picker click and dial press is its own undo step: two model picks undo one at a time back to the original model, and two presses of the bound temperature dial undo one press at a time', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['picker-undo']);

  expect(report.models).toEqual(['gpt-4.1', 'gpt-4o', report.original]);
  expect(report.temperatures).toEqual(['Float(0.6)', 'Float(0.5)', 'Float(0.4)']);
});

test('A Split whose Outputs are cleared stays cleared: the saved list is empty instead of falling back to out_0 and out_1, and Check flags both the empty list and the wire still leaving the old left output', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['split-cleared']);

  expect(report.before).toBe('valid');
  expect(report.outputs).toEqual([]);
  expect(report.after).toBe('invalid');
  expect(report.errors).toContain('Split: needs outputs');
  expect(report.errors).toContain('Split: left is no longer listed but still has a wire');
});

test('One brew has one set of step numbers: the canvas badges and the Trace list agree for a subgraph call and a failed step, child steps nest under their call as 2.1 and 2.2, the header counts top-level steps, and it names a cancelled or timed-out brew as such', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['step-numbers']);
  const steps = ['1 On Message 1ms Inspect ›', '2 Subgraph 2ms Inspect ›', '2.1 call › inner 1ms Inspect ›', '2.2 call › inner_two 1ms Inspect ›', '3 Rhai Script boom Inspect ›'];

  for (const [status, word] of [['failed', 'Failed'], ['cancelled', 'Cancelled'], ['timed_out', 'Timed out']]) {
    expect(report[status].badges).toEqual({ trigger: 1, call: 2, script: 3, reply: null });
    expect(report[status].steps).toEqual(steps);
    expect(report[status].log).toBe('1 On Message · 1ms 2 Subgraph · 2ms 3 Rhai Script: boom');
    expect(report[status].inspectedChild).toBe('call');
    expect(report[status].header).toBe(`Brew #1 · ${word} 3 steps · 7 events · <1ms total`);
  }
});

test('A newer saved draft never hides the version that is still live: after publishing v1 and saving a different draft, the open potion row says Draft with v1 live, and it still says v1 live after opening another potion', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['live-after-draft']);

  expect(report.openRow).toBe('Tasting Agent 2 ingredients · v1 live Draft');
  expect(report.elsewhereRow).toBe('Tasting Agent 2 ingredients · v1 live Open ›');
});

test('Check holds a bound setting to the same rules as a typed one: a temperature variable of 5 and a memory limit variable of 500 are flagged as out of range, and a temperature variable that holds a word is flagged as not a number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['bound-checks']);

  expect(report.clean).toBe('valid');
  expect(report.ranged).toEqual([
    'Search Memory: limit needs a whole number from 1 to 100, but the variable max_memories is 500',
    'Generate Text: temperature needs a number from 0 to 2, but the variable temperature is 5',
  ]);
  expect(report.worded).toEqual(['Generate Text: temperature needs a number from 0 to 2, but the variable temperature holds text']);
});

test('A step fresh from the Pantry shows the settings it will really use: model steps name the default model, Delay shows 1000 ms, Split counts its two default outputs, searches show top 5, and a Switch with no cases shows no note', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['spawned-notes']);

  expect(report).toEqual({
    'ai/generate_text': 'gpt-4o-mini',
    'ai/stream_text': 'gpt-4o-mini',
    'ai/structured_json': 'gpt-4o-mini',
    'memory/search': 'top 5',
    'knowledge/search': 'top 5',
    'time/delay': '1000 ms',
    'flow/split': '2 outputs',
    'flow/switch': null,
  });
});

test('Filtering the Pantry never cuts a name: with "on" or "var" typed at both sizes every matching row shows its whole name as one wrapped title with the match picked out, and no row ends in an ellipsis', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['pantry-filtered']);

  for (const [key, least] of [['960-on', 8], ['1280-on', 8], ['960-var', 4], ['1280-var', 4]] as const) {
    expect(report[key].seen).toBeGreaterThanOrEqual(least);
    expect({ key, cut: report[key].cut }).toEqual({ key, cut: [] });
  }
});

test('A lone bottle tag sits centred until it has a step to show: before a brew the tag keeps its plain 10 point inset with no badge, and a bottle that fails during a brew wears its step number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['stand-badge']);

  expect(report.idleBadge).toBe(false);
  expect(report.idleInset).toBe(10);
  expect(report.failedStep).toBe(5);
  expect(report.failedBadge).toBe('5');
});

test('Rack glass carries the potion the whole way: every stream runs inside the glass from tap to bottle neck, no glass runs on dry past the last bottle, and an untaken route starts its dashes only where it leaves the brewed flow', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rack-pipes']);

  for (const key of ['router@1280', 'eight@1280', 'eight@960']) {
    expect(report[key].glass).toBeGreaterThan(0);
    expect({ key, leaks: report[key].leaks, dry: report[key].dry, overGold: report[key].overGold }).toEqual({ key, leaks: 0, dry: 0, overGold: 0 });
  }
});

test('Long bottle names never break the rack: three long route names wrap whole and every tag stays on the canvas, and in an eight-bottle rack every tag keeps the part of its name that differs instead of cutting it off', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['long-routes']);

  for (const key of ['three@1280', 'three@960', 'eight@1280', 'eight@960']) {
    expect({ key, hidden: report[key].hidden, belowCanvas: report[key].belowCanvas }).toEqual({ key, hidden: [], belowCanvas: [] });
  }
});

test('A card menu never floats over the pour fan or the Cauldron and Grimoire switch at either size: it either finds clear canvas or docks into the Recipe tab, opening that tab and sitting inside its scroll, and it does not follow you onto Variables', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['menu-fan']);

  expect(report.floatingOnFan).toEqual([]);
  expect(report.onSwitcher).toEqual([]);
  expect(report.docked).toBeGreaterThan(0);
  expect(report.dockedOffRecipe).toEqual([]);
  expect(report.dockedOutsideScroll).toEqual([]);
  expect(report.dockedRepeatsTitle).toEqual([]);
});

test('Bottles never share a visible name: two Sends fed by an orders route on two different Switches read Send · orders 1 and Send · orders 2, while a unique route keeps its plain name', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['twin-switch-names']);

  expect(report.titles).toEqual(['Send · orders 1', 'Send · refunds', 'Send · orders 2']);
});

test('Redo never crosses potions: after undoing an edit in one potion and opening another, there is nothing to redo and redo leaves the newly opened recipe untouched', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['redo-across']);

  expect(report).toEqual({ name: 'Research Agent', redoWaiting: false, unchanged: true });
});

test('Publishing ships every trigger that was not set aside: a recipe with a wired On Start and a standalone On Webhook releases both, so which trigger you test with never changes what ships', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['lone-trigger-ships']);

  expect([...report.shipped].sort()).toEqual(['hook', 'say', 'start']);
});

test('A variable rename only lands on a valid unused name and carries every use: {{ max_memories.value }} follows the rename with its path, and typing through another variable name keeps the old name and flags Check until the name is unique, so neither variable steals the other one uses', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rename-safe']);

  expect(report.pathedPrompt).toBe('Use {{ recall.value }} and {{recall}}');
  expect(report.clashCheck).toBe('invalid');
  expect(report.namesMid).toEqual(['max_memories', 'temperature']);
  expect(report.namesAfter).toEqual(['temperaturex', 'temperature']);
  expect(report.limit).toBe('{{temperaturex}}');
  expect(report.temperature).toBe('{{temperature}}');
  expect(report.spaced).toEqual({ check: ['max memories: a variable name uses only letters, digits, _ and -'], name: 'max_memories' });
});

test('A Split fresh from the Pantry shows the two outlets it will really fire, out_0 and out_1, until you change or clear its list', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['fresh-split']);

  expect(report.outlets).toEqual(['out_0', 'out_1']);
});

test('Check judges bound settings the way the brew does: a template that is just {{count}} prints the number and passes, while a loop limit variable of 1.5 is flagged because the loop needs a whole number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['check-matches-brew']);

  expect(report.printed).toEqual({ check: 'valid', errors: [] });
  expect(report.halfLoop).toContain('While Loop: max iterations needs a whole number of at least 1, but the variable limit is 1.5');
});

test('A crowded rack of broken bottles stays readable: with fourteen Sends missing their content every bottle wears its own error mark inside its tag, no mark touches another bottle or mark at either size, and the selected bottle explains the problem in Recipe', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rack-errors']);

  for (const width of ['960', '1280']) {
    expect({ width, ...report[width] }).toEqual({ width, check: 'invalid', unmarked: [], clashes: [], recipeTells: true });
  }
});

test('Untaken means skipped by a finished brew: while the first Delay is still running the steps after it never say Untaken, and a Quenched brew leaves no step marked Untaken either, while every step a real finished brew skipped does say Untaken', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['waiting-not-untaken']);

  expect(report.whileBrewing).toEqual([]);
  expect(report.afterQuench).toEqual([]);
  expect(report.finishedSkipped.idle).toBeGreaterThan(0);
  expect(report.finishedSkipped.marked).toBe(report.finishedSkipped.idle);
});

test('Selecting a vial in a crowded kitchen lights its whole pour: its strand is drawn lit and on top of the bundle all the way to the rim, and no other strand is lit', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['lit-strand']);

  expect(report.drawnLast).toBe(report.picked);
  expect(report.lit).toEqual([report.picked]);
});

test('Picking Gemini in the model menu saves a provider the step accepts: gemini-2.5-pro is stored with provider gemini, one of the provider choices the Generate Text step declares', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['gemini-pick']);

  expect(report.model).toBe('gemini-2.5-pro');
  expect(report.provider).toBe('gemini');
  expect(report.allowed).toContain(report.provider);
});

test('Undo after redo walks straight back: two model picks, undo, redo, then two undos land on gpt-4o and then the original model with no dead press, and the step you were editing stays selected throughout', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['redo-then-undo']);

  expect(report.seen).toEqual(['gpt-4o', 'gpt-4.1', 'gpt-4o', report.original]);
  expect(report.selected).toBe('llm');
});

test('Check judges a bound loop limit even without a value to read: a Float variable holding 3.0 passes, an Int variable with no value and a true-or-false variable are both flagged before the brew can fail', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['bound-unset']);

  expect(report.whole).toEqual([]);
  expect(report.unset).toEqual(['While Loop: max iterations needs a whole number of at least 1, but the variable limit has no value']);
  expect(report.flag).toEqual(['While Loop: max iterations needs a whole number of at least 1, but the variable limit holds true or false']);
});

test('A step that finishes a pass and then fails shows its failure in the log: a While Loop that hits its limit reads as a red failed step 2 with the limit message, not as a green finish, and the expanded log ends on Failed rather than Brewed', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['finish-then-fail-log']);

  expect(report.log).toBe('1 On Message · 1ms 2 While Loop: while loop exceeded max_iterations 1');
  expect(report.finalRow).toMatch(/Failed/);
  expect(report.finalRow).not.toMatch(/Brewed/);
});

test('A step that was only cut short is Stopped, not Failed: when one branch fails, the Delay still running on the other branch turns grey and reads Stopped with no error mark, while the step that failed stays red with its number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['stopped-not-failed']);

  expect(report.broken).toBe('failed');
  expect(report.brokenWords).toContain('!');
  expect(report.slow).toBe('idle');
  expect(report.slowWords).toContain('Stopped');
  expect(report.slowWords).not.toContain('!');
});

test('Every red tag names what is missing: after Check on the dozen-ingredient potion each flagged card at both sizes shows the name of the input it lacks, never a bare Missing', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['named-missing']);

  expect(report['1280'].vague).toEqual([]);
  expect(report['960'].vague).toEqual([]);
});

test('A failed pour stub keeps clear of its neighbours: with Search Memory missing its query the red stub follows its own pour toward the rim and its cross stays at least 10 points from every other stream at both sizes', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['stub-clear']);

  for (const width of ['1280', '960']) {
    expect({ width, ...report[width] }).toEqual({ width, crowded: [], hanging: [], hexed: 1 });
  }
});

test('A brewed crowded kitchen stays tidy at both sizes: no time chip covers a bottle name, every bottle step number sits on its own bottle above its tag instead of on the shelf bracket, and no card time chip covers the card icon or the card name', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['kitchen-brewed-tidy']);

  for (const width of ['960', '1280']) {
    expect({ width, ...report[width] }).toEqual({ width, coveredNames: [], looseBadges: [], coveredIcons: [], coveredTitles: [] });
  }
});

test('The Model and Temperature tiles line up: at both sizes the temperature dial sits level with the model name, and a temperature bound to a variable named temperature reads Temperature once', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['tile-align']);

  for (const width of ['1280', '960']) {
    expect({ width, offset: report[width].offset <= 1 }).toEqual({ width, offset: true });
    expect(report[width].label).toBe('Temperature');
  }
});

test('Typing a variable rename one key at a time never takes over a step input: renaming lim to query_limit passes through query, yet the Template that reads its wired query input keeps {{query}}, and only the Delay bound to the variable follows the new name', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rename-keystrokes']);

  expect(report).toEqual({
    name: 'query_limit',
    template: 'Need {{query}} now',
    ms: '{{query_limit}}',
    queryWired: true,
    check: 'valid',
    landedOnInput: { check: ['query: a step input already uses this name'], name: 'query_limit' },
  });
});

test('Big racks keep the potion in view: after a brew every bottle on a rack of four or more wears its step number up by the cork, clear of the fill line, and none of them shows a time chip, at 960 and 1280 alike', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['seal-clear']);

  for (const scene of ['kitchen@960', 'kitchen@1280', 'eight@1280']) {
    expect(report[scene].bottles).toBeGreaterThanOrEqual(8);
    expect({ scene, onFill: report[scene].onFill, timeChips: report[scene].timeChips }).toEqual({ scene, onFill: [], timeChips: 0 });
  }
});

test('Grimoire status tags never touch the card above: after brewing the webhook router every Untaken or time tag at both sizes keeps at least 2 points of air from every other card', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['perched-clear']);

  for (const width of ['1280', '960']) {
    expect(report[width].perched).toBeGreaterThan(3);
    expect({ width, touching: report[width].touching }).toEqual({ width, touching: [] });
  }
});

test('Card icons stay readable on crowded pots: the smallest ingredient icon is at least 14 points across in the crowded kitchen and the dozen-step potion at 960 by 640 and in the Support Agent at 1280', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['icon-size']);

  for (const scene of ['kitchen@960', 'dozen@960', 'support@1280']) {
    expect({ scene, readable: report[scene] >= 14 }).toEqual({ scene, readable: true });
  }
});

test('The live release keeps its Live mark while the draft changes: v1 says Live with no Restore right after publishing, still says Live after an edit and also offers Restore, and once v2 is published v1 offers only Restore while v2 says Live', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['live-mark-kept']);

  expect(report.fresh).toMatch(/Live/);
  expect(report.fresh).not.toMatch(/Restore/);
  expect(report.edited).toMatch(/Live/);
  expect(report.edited).toMatch(/Restore/);
  expect(report.pending).toBe(1);
  expect(report.older).toMatch(/Restore/);
  expect(report.older).not.toMatch(/Live/);
  expect(report.newest).toMatch(/Live/);
  expect(report.newest).not.toMatch(/Restore/);
});

test('A number variable set earlier on the route can be read by a later step: On Start, Set ms, then Delay {{ms}} passes Check even though ms has no starting value, while a Delay that reads ms before any Set still says it has no value', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['set-before-read']);

  expect(report.setFirst).toEqual([]);
  expect(report.neverSet).toEqual(['Delay: ms needs a whole number of at least 0, but the variable ms has no value']);
  expect(report.setAfter).toEqual(report.neverSet);
});

test('A long route name fits its Recipe socket row at both window sizes: urgent_customer_reconciliation_notification shows whole, wrapping if it must, and its Flow label stays inside the panel', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['long-route-row']);

  for (const size of ['960', '1280']) {
    expect(report[size].spill).toEqual([]);
    expect(report[size].nameWhole).toBe(true);
  }
});

test('Bottles whose long route names differ only at the start still get different tags: four Send bottles on alpha_, bravo_, charlie_ and delta_customer_reconciliation_notification show four different names at both window sizes', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['twin-tails']);

  for (const size of ['960', '1280']) {
    expect(report[size].tags.every((tag: string) => tag.length > 0)).toBe(true);
    expect(report[size].distinct).toBe(4);
  }
});
