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
  let warmLinenPixels = 0;
  let emeraldBrothPixels = 0;
  let emberPixels = 0;
  for (let offset = 0; offset < pixels.length; offset += channels) {
    const r = pixels[offset];
    const g = pixels[offset + 1];
    const b = pixels[offset + 2];
    if (r >= 225 && g >= 215 && b >= 195) warmLinenPixels += 1;
    if (g >= 110 && g > r + 25 && g >= b) emeraldBrothPixels += 1;
    if (r >= 190 && g >= 95 && g <= 235 && b <= 130 && r - b >= 70) emberPixels += 1;
  }
  const entropy = [...colors.values()].reduce((sum, count) => sum - (count / total) * Math.log2(count / total), 0);
  return { width, height, distinctColors: colors.size, entropy, warmLinenShare: warmLinenPixels / total, emeraldBrothPixels, emberPixels };
}

test('magicklala studio --snap writes a detailed Bubble & Sparkle Hearth cauldron PNG with warm linen ground, emerald broth, and hearth embers', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-snap-'));
  const out = join(dir, 'studio.png');
  try {
    const env = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '' };
    await run('cargo', ['run', '-q', '-p', 'magicklala-cli', '--', 'studio', '--snap', out], { cwd: repoRoot, env, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
    const bytes = await readFile(out);
    expect([...bytes.subarray(0, 8)]).toEqual(pngSignature);
    expect((await stat(out)).size).toBeGreaterThan(1024);
    expect(bytes.readUInt32BE(16)).toBeGreaterThan(0);
    expect(bytes.readUInt32BE(20)).toBeGreaterThan(0);
    const facts = pngFacts(bytes);
    expect(facts.width).toBe(bytes.readUInt32BE(16));
    expect(facts.distinctColors).toBeGreaterThan(200);
    expect(facts.entropy).toBeGreaterThan(1.5);
    expect(facts.warmLinenShare).toBeGreaterThan(0.45);
    expect(facts.emeraldBrothPixels).toBeGreaterThan(1500);
    expect(facts.emberPixels).toBeGreaterThan(300);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
