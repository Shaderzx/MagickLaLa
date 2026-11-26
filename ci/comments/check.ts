import { execFileSync } from 'node:child_process'
import { lstatSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { findComments, isCode, minus, most, parseStatus, report, subtract } from './register.ts'
import type { Located } from './register.ts'

const args = process.argv.slice(2)
const at = args.indexOf('--base')
const base = at >= 0 && args[at + 1] ? args[at + 1] : 'origin/main'
const git = (...a: string[]) => execFileSync('git', a, { encoding: 'utf8', maxBuffer: 1 << 28, stdio: ['ignore', 'pipe', 'pipe'] })
const show = (rev: string, path: string) => {
  try {
    return git('show', rev + ':' + path)
  } catch {
    return ''
  }
}

const root = git('rev-parse', '--show-toplevel').trim()
const from = git('merge-base', base, 'HEAD').trim()
const files = [
  ...parseStatus(git('-C', root, 'diff', '-z', '--name-status', '-M', '--diff-filter=ACMRT', from)),
  ...git('-C', root, 'ls-files', '-z', '--others', '--exclude-standard').split('\0').filter(Boolean).map((p) => ({ path: p, from: '' })),
]
const found: Located[] = []
const released: string[] = []
for (const f of files) {
  const full = join(root, f.path)
  if (lstatSync(full, { throwIfNoEntry: false })?.isFile() !== true) continue
  const after = readFileSync(full, 'utf8')
  if (!isCode(f.path, after)) continue
  const keys = (rev: string, path: string) => findComments(path, show(rev, path)).map((x) => x.key)
  const mergeBase = f.from ? keys(from, f.from) : []
  const before = most(mergeBase, keys(base, f.path))
  const now = findComments(f.path, after)
  released.push(...minus(mergeBase, now.map((x) => x.key)))
  for (const x of subtract(now, before)) found.push({ ...x, path: f.path })
}
const gone = git('-C', root, 'diff', '-z', '--name-only', '--diff-filter=D', from).split('\0').filter(Boolean)
found.splice(0, found.length, ...(subtract(found, [...released, ...gone.flatMap((p) => findComments(p, show(from, p)).map((x) => x.key))]) as Located[]))
if (found.length) {
  process.stdout.write('This change adds ' + found.length + ' code comment(s) since ' + base + ' (' + from.slice(0, 10) + "), and fleet's rule allows none:\n" + report(found, 200) + '\n')
  process.exit(1)
}
process.stdout.write('No new code comments since ' + base + '.\n')
