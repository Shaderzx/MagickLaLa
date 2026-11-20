import { test, expect } from 'e2e';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const pngSignature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

test('magicklala studio --snap writes a non-empty Studio PNG without a display server', async () => {
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
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});