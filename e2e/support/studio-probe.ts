import { execFile } from 'node:child_process';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);
const repoRoot = resolve(import.meta.dirname, '..', '..');
const unreachableLlm = 'http://127.0.0.1:9';

export async function studioProbe(args: string[]): Promise<any> {
  const env = { ...process.env, DISPLAY: '', WAYLAND_DISPLAY: '', MAGICKLALA_LLM_BASE_URL: unreachableLlm, MAGICKLALA_LLM_API_KEY: '', OPENAI_API_KEY: '' };
  const { stdout } = await run('cargo', ['run', '-q', '-p', 'magicklala-studio', '--example', 'studio_probe', '--', ...args], { cwd: repoRoot, env, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
  return JSON.parse(stdout.trim().split('\n').at(-1) ?? '');
}
