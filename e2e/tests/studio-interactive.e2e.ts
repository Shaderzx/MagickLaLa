import { test, expect } from 'e2e';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { inflateSync } from 'node:zlib';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const pngSignature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

type Bounds = { key: string; left: number | null; right: number | null; top: number | null; bottom: number | null };

type ProbeReport = {
  registry: { kinds: string[]; appKinds: string[]; paletteKinds: string[] };
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
  run: { events: string[]; succeededNodes: number; activeWires: number };
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

test('studio probe drives a registry palette, spawn, drag, validate, run trace, theme toggle, a right-click canvas menu and an unclipped 960px toolbar, then renders dark and light desktop and compact PNGs headlessly', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-probe-'));
  const desktop = join(dir, 'studio-desktop.png');
  const compact = join(dir, 'studio-compact.png');
  try {
    const env = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '' };
    const { stdout } = await run('cargo', ['run', '-q', '-p', 'magicklala-studio', '--example', 'studio_probe', '--', desktop, compact], {
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
    expect(report.run.succeededNodes).toBe(report.defaultSpell.nodes);
    expect(report.run.activeWires).toBe(report.defaultSpell.wires);

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