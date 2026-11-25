import type { Register } from 'claude-code'

export type Found = { line: number; text: string; key: string }
export type Located = Found & { path: string }
type Baseline = { head: string; dirty: Record<string, string[]>; at?: number }
type Session = { at: number; roots: Record<string, Baseline>; cwd?: string; shells?: string[]; loose?: Record<string, string[]> }

type Spec = {
  line: string[]
  block: [string, string][]
  quotes: string
  multiline: string
  raw: string
  hashGuard: string
  lua: boolean
  triple: boolean
  wordHash: boolean
  tokenQuotes: boolean
  py: boolean
  shell: boolean
  rust: boolean
  regex: boolean
  template: boolean
  url: boolean
  php: boolean
  nested: boolean
  jsx: boolean
  yaml: boolean
  dollar: boolean
  tags: boolean
  toml: boolean
}

const blank: Spec = {
  line: [],
  block: [],
  quotes: '',
  multiline: '',
  raw: '',
  hashGuard: '',
  lua: false,
  triple: false,
  wordHash: false,
  tokenQuotes: false,
  py: false,
  shell: false,
  rust: false,
  regex: false,
  template: false,
  url: false,
  php: false,
  nested: false,
  jsx: false,
  yaml: false,
  dollar: false,
  tags: false,
  toml: false,
}
const spec = (s: Partial<Spec>): Spec => ({ ...blank, ...s })

const C = spec({ line: ['//'], block: [['/*', '*/']], quotes: '\'"' })
const C3 = spec({ line: ['//'], block: [['/*', '*/']], quotes: '\'"', triple: true })
const JS = spec({ line: ['//'], block: [['/*', '*/']], quotes: '\'"', regex: true, template: true })
const JSX = spec({ ...JS, jsx: true })
const GO = spec({ line: ['//'], block: [['/*', '*/']], quotes: '\'"' + String.fromCharCode(96), multiline: String.fromCharCode(96), raw: String.fromCharCode(96) })
const RUST = spec({ line: ['//'], block: [['/*', '*/']], quotes: '"', multiline: '"', rust: true, nested: true })
const PY = spec({ line: ['#'], quotes: '\'"', triple: true, py: true })
const SH = spec({ line: ['#'], quotes: '\'"', multiline: '\'"', wordHash: true, shell: true })
const HASH = spec({ line: ['#'], quotes: '\'"', hashGuard: '?$' })
const PERL = spec({ line: ['#'], quotes: '\'"', wordHash: true })
const TEXTY = spec({ line: ['#'], quotes: '\'"', wordHash: true, tokenQuotes: true })
const YAML = spec({ ...TEXTY, yaml: true })
const TOML = spec({ line: ['#'], quotes: '\'"', triple: true, toml: true })
const HCL = spec({ line: ['#', '//'], block: [['/*', '*/']], quotes: '"', wordHash: true })
const NIX = spec({ line: ['#'], block: [['/*', '*/']], quotes: '"', multiline: '"', wordHash: true })
const PS = spec({ line: ['#'], block: [['<#', '#>']], quotes: '\'"', multiline: '\'"', wordHash: true })
const JULIA = spec({ line: ['#'], block: [['#=', '=#']], quotes: '"', triple: true })
const CSS = spec({ block: [['/*', '*/']], quotes: '\'"', url: true })
const SCSS = spec({ line: ['//'], block: [['/*', '*/']], quotes: '\'"', url: true })
const SQL = spec({ line: ['--'], block: [['/*', '*/']], quotes: '\'"', dollar: true })
const LUA = spec({ line: ['--'], quotes: '\'"', lua: true })
const HS = spec({ line: ['--'], block: [['{-', '-}']], quotes: '"' })
const LISP = spec({ line: [';'], block: [['#|', '|#']], quotes: '"', multiline: '"' })
const ERL = spec({ line: ['%'], quotes: '"' })
const PHP = spec({ line: ['//', '#'], block: [['/*', '*/']], quotes: '\'"', multiline: '\'"', php: true })
const HTML = spec({ block: [['<!--', '-->']], tags: true })

const group = (s: Spec | 'markup' | 'php', exts: string): [string, Spec | 'markup' | 'php'][] => exts.split(' ').map((x) => [x, s])

const BY_EXT = new Map<string, Spec | 'markup' | 'notebook' | 'php'>([
  ...group(JS, 'js mjs cjs ts mts cts json5 jsonc'),
  ...group(JSX, 'jsx tsx'),
  ...group(C, 'c h cc cpp cxx hpp hh hxx m mm cs proto zig v sol glsl hlsl wgsl metal cu fs fsx'),
  ...group(C3, 'java kt kts scala sc swift groovy gradle dart'),
  ...group(GO, 'go'),
  ...group(RUST, 'rs'),
  ...group(PY, 'py pyi pyw bzl bazel star'),
  ...group(SH, 'sh bash zsh ksh fish'),
  ...group(HASH, 'rb rake gemspec r ex exs coffee nim cr graphql gql'),
  ...group(PERL, 'pl pm'),
  ...group(TEXTY, 'mk cmake'),
  ...group(YAML, 'yaml yml'),
  ...group(TOML, 'toml'),
  ...group(HCL, 'tf tfvars hcl'),
  ...group(NIX, 'nix'),
  ...group(PS, 'ps1 psm1 psd1'),
  ...group(JULIA, 'jl'),
  ...group(CSS, 'css'),
  ...group(SCSS, 'scss less styl sass'),
  ...group(SQL, 'sql psql plsql'),
  ...group(LUA, 'lua'),
  ...group(HS, 'hs elm'),
  ...group(LISP, 'clj cljs cljc edn el lisp scm rkt'),
  ...group(ERL, 'erl hrl'),
  ...group('php', 'php'),
  ...group('markup', 'html htm xhtml xml svg vue svelte astro'),
])

const BY_NAME = new Map<string, Spec>([
  ...(['makefile', 'gnumakefile', 'dockerfile', 'containerfile', 'justfile', 'cmakelists.txt'].map((n) => [n, TEXTY]) as [string, Spec][]),
  ...(['rakefile', 'gemfile', 'brewfile', 'podfile', 'vagrantfile', 'fastfile'].map((n) => [n, HASH]) as [string, Spec][]),
  ...(['build', 'workspace'].map((n) => [n, PY]) as [string, Spec][]),
])

const SKIPPED = /(^|\/)(node_modules|vendor|third_party|\.venv|venv|__pycache__|\.git)\//

export function specFor(path: string, text: string): Spec | 'markup' | 'notebook' | 'php' | undefined {
  if (SKIPPED.test(path)) return undefined
  const name = (path.split('/').pop() ?? '').toLowerCase()
  if (/\.min\.[a-z]+$/.test(name)) return undefined
  if (name.endsWith('.ipynb')) return 'notebook'
  const named = BY_NAME.get(name)
  if (named) return named
  if (name.startsWith('dockerfile') || name.endsWith('.dockerfile') || name.startsWith('containerfile')) return TEXTY
  const dot = name.lastIndexOf('.')
  if (dot > 0) return BY_EXT.get(name.slice(dot + 1))
  const bang = /^#!\s*(\S+)([^\n]*)/.exec(text)
  if (!bang) return undefined
  const words = [bang[1], ...bang[2].trim().split(/\s+/).filter(Boolean)]
  const interpreter = (words[0].split('/').pop() ?? '') === 'env' ? (words.slice(1).find((w) => !w.startsWith('-') && !w.includes('=')) ?? '') : words[0]
  const runner = (interpreter.split('/').pop() ?? '').replace(/[\d.]+$/, '')
  if (runner === 'python' || runner === 'uv') return PY
  if (['sh', 'bash', 'zsh', 'ksh', 'dash', 'fish'].includes(runner)) return SH
  if (['node', 'deno', 'bun', 'tsx', 'ts-node'].includes(runner)) return JS
  if (['ruby', 'perl', 'Rscript'].includes(runner)) return HASH
  return undefined
}

export function isCode(path: string, text = ''): boolean {
  return specFor(path, text) !== undefined
}

const neverCode = (path: string) => !isCode(path, '#!/bin/sh')

const DIRECTIVES: RegExp[] = [
  /^-\*-.*-\*-$/,
  /^(-\*-\s*)?(en)?coding[:=]\s*[\w.-]+(\s*-\*-)?$/,
  /^vim?:(\s*set)?(\s+[\w-]+(=\S+)?)+:?$/,
  /^type:\s*ignore(\[[\w-]+(,\s*[\w-]+)*\])?$/,
  /^noqa(:\s*[A-Z]+\d+(,\s*[A-Z]+\d+)*)?$/,
  /^(ruff|flake8):\s*noqa(:\s*[A-Z]+\d+(,\s*[A-Z]+\d+)*)?$/,
  /^pyright:\s*(basic|strict|standard|ignore(\[[\w,\s]+\])?|\w+=\w+(,\s*\w+=\w+)*)$/,
  /^mypy:\s*(ignore-errors|strict|[\w-]+=[\w,-]+(,\s*[\w-]+=[\w,-]+)*)$/,
  /^pylint:\s*((disable|enable)=[\w-]+(,\s*[\w-]+)*|skip-file)$/,
  /^isort:\s*(skip|skip_file|off|on)$/,
  /^fmt:\s*(off|on|skip)$/,
  /^yapf:\s*(disable|enable)$/,
  /^pragma:\s*no\s+(cover|branch)$/,
  /^nosec(\s+B\d+(,\s*B\d+)*)?$/,
  /^pyre-(strict|unsafe|ignore-all-errors|ignore(\[\d+\])?|fixme(\[\d+\])?)$/,
  /^frozen_string_literal:\s*(true|false)$/,
  /^rubocop:(disable|enable|todo)\s+[\w/]+(,\s*[\w/]+)*$/,
  /^shellcheck(\s+(disable|enable|source|shell|external-sources|source-path)=\S+)+$/,
  /^@ts-(ignore|expect-error|nocheck|check)$/,
  /^eslint-(disable|enable)(-next-line|-line)?(\s+[\w@/-]+(,\s*[\w@/-]+)*)?$/,
  /^eslint-env\s+[\w-]+(,\s*[\w-]+)*$/,
  /^eslint\s+[\w@/-]+:\s*(off|warn|error|[012])(,\s*[\w@/-]+:\s*(off|warn|error|[012]))*$/,
  /^tslint:(disable|enable)(-next-line|-line)?(:[\w-]+)?(\s+[\w-]+)*$/,
  /^oxlint-(disable|enable)(-next-line|-line)?(\s+[\w@/-]+(,\s*[\w@/-]+)*)?$/,
  /^globals?\s+[\w$]+(:\s*(readonly|writable|true|false))?(,\s*[\w$]+(:\s*(readonly|writable|true|false))?)*$/,
  /^prettier-ignore(-start|-end)?$/,
  /^(istanbul|c8|v8)\s+ignore(\s+(next|if|else|file|start|stop))?$/,
  /^@(jsx|jsxFrag|jsxImportSource|jsxRuntime)\s+[\w./@-]+$/,
  /^@flow(\s+strict(-local)?)?$/,
  /^@(vitest|jest)-environment\s+[\w-]+$/,
  /^webpack[A-Z]\w*:\s*("[^"]*"|'[^']*'|true|false|\w+)(,\s*webpack[A-Z]\w*:\s*("[^"]*"|'[^']*'|true|false|\w+))*$/,
  /^@vite-ignore$/,
  /^[#@]__(PURE|NO_SIDE_EFFECTS|INLINE|NOINLINE|KEY)__$/,
  /^deno-lint-ignore(-file)?(\s+[\w-]+)*$/,
  /^deno-fmt-ignore(-file)?$/,
  /^biome-ignore(-all|-start|-end)?\s+[\w/]+:\s*\S.*$/,
  /^@refresh reset$/,
  /^NOLINT(NEXTLINE|BEGIN|END)?(\([\w,.*-]+\))?$/,
  /^clang-format\s+(off|on)$/,
  /^IWYU\s+pragma:\s+[\w-]+(\s+"?[\w./<>-]+"?)?$/,
  /^swift-tools-version:\s*\d+(\.\d+){0,2}$/,
  /^swiftlint:(disable|enable)(:next|:this|:previous)?(\s+\w+)+$/,
  /^noinspection\s+\w+(,\s*\w+)*$/,
  /^language=[\w-]+$/,
  /^SPDX-(License-Identifier|FileCopyrightText):\s*\S.*$/,
  /^@generated$/,
  /^Code generated .* DO NOT EDIT\.$/,
  /^yaml-language-server:\s*\$schema=\S+$/,
  /^yamllint\s+(disable|enable)(-line|-file)?(\s+rule:[\w-]+)*$/,
  /^:schema\s+\S+$/,
  /^(syntax|escape|check)=\S+$/,
  /^stylelint-(disable|enable)(-next-line|-line)?(\s+[\w/-]+(,\s*[\w/-]+)*)?$/,
  /^Requires(\s+-\w+(\s+[\w.:\\-]+)?)+$/,
  /^e2e-lint-allow\s+[\w-]+:\s*\S.*$/,
]

const GO_DIRECTIVE = /^(go:[a-z]+(\s.*)?|line \S+:\d+(:\d+)?|export [A-Za-z_]\w*|nolint(:[\w,-]+)?)$/
const REFERENCE = /^\/\s?<(reference|amd-module|amd-dependency)\b[^>]*\/>\s*$/

export function isDirective(body: string, line: number, marker: string): boolean {
  if (marker === '#' && line === 1 && body.startsWith('!')) return true
  if (marker === '//' && (GO_DIRECTIVE.test(body) || REFERENCE.test(body))) return true
  const text = body.replace(/^\*+/, '').replace(/\*+$/, '').trim()
  return DIRECTIVES.some((r) => r.test(text))
}

const KEYWORDS = new Set(['return', 'typeof', 'case', 'do', 'else', 'in', 'of', 'new', 'delete', 'void', 'throw', 'yield', 'await', 'instanceof'])
const REGEX_AFTER = '(,=:[!&|?{};+-*%<>~^'
const OBJECT_AFTER = '(,=:[!&|?+-*%<~^'
const CONDITIONS = new Set(['if', 'while', 'for', 'with'])
const isIdent = (c: string | undefined) => c !== undefined && /[\w$]/.test(c)
const newlines = (s: string, from: number, to: number) => {
  let n = 0
  for (let k = from; k < to; k++) if (s.charCodeAt(k) === 10) n++
  return n
}
const keyOf = (raw: string) => raw.replace(/\s+/g, ' ').trim()
const shown = (raw: string) => {
  const first = raw.split('\n')[0].trim()
  return first.length > 120 ? first.slice(0, 117) + '...' : first
}

const LONG_BRACKET = /(--)?\[(=*)\[/y

function longBracket(src: string, at: number): RegExpExecArray | null {
  LONG_BRACKET.lastIndex = at
  return LONG_BRACKET.exec(src)
}

function quoted(src: string, j: number, q: string, escapes: boolean, multiline: boolean): number {
  const n = src.length
  while (j < n) {
    const d = src[j]
    if (d === '\\' && escapes) {
      j += 2
      continue
    }
    if (src.startsWith(q, j)) return j + q.length
    if (d === '\n' && !multiline) return j
    j++
  }
  return n
}

function template(src: string, j: number, holes?: [number, number][]): number {
  const n = src.length
  while (j < n) {
    const d = src[j]
    if (d === '\\') {
      j += 2
      continue
    }
    if (d === String.fromCharCode(96)) return j + 1
    if (d === '$' && src[j + 1] === '{') {
      const end = interpolation(src, j + 2)
      holes?.push([j + 2, Math.max(j + 2, end - 1)])
      j = end
      continue
    }
    j++
  }
  return n
}

function interpolation(src: string, j: number): number {
  const n = src.length
  let depth = 1
  let prev = '('
  while (j < n) {
    const d = src[j]
    if (d === String.fromCharCode(96)) {
      j = template(src, j + 1)
      prev = 'a'
      continue
    }
    if (d === '"' || d === "'") {
      j = quoted(src, j + 1, d, true, false)
      prev = 'a'
      continue
    }
    if (d === '/' && src[j + 1] === '/') {
      const eol = src.indexOf('\n', j)
      j = eol < 0 ? n : eol
      continue
    }
    if (d === '/' && src[j + 1] === '*') {
      const close = src.indexOf('*/', j + 2)
      j = close < 0 ? n : close + 2
      continue
    }
    if (d === '/' && REGEX_AFTER.includes(prev)) {
      const end = regexEnd(src, j)
      if (end > 0) {
        j = end
        prev = 'a'
        continue
      }
    }
    if (d === '{') depth++
    if (d === '}' && --depth === 0) return j + 1
    if (!/\s/.test(d)) prev = isIdent(d) ? 'a' : d
    j++
  }
  return n
}

function nestedClose(src: string, j: number, open: string, close: string): number {
  let depth = 1
  while (j < src.length) {
    if (src.startsWith(open, j)) {
      depth++
      j += open.length
      continue
    }
    if (src.startsWith(close, j)) {
      if (--depth === 0) return j
      j += close.length
      continue
    }
    j++
  }
  return -1
}

function jsxEnd(src: string, i: number, holes: [number, number][]): number {
  const n = src.length
  let j = i + 1
  const hole = () => {
    const end = interpolation(src, j + 1)
    holes.push([j + 1, Math.max(j + 1, end - 1)])
    j = end
  }
  if (src[j] === '>') j++
  else {
    while (j < n && /[\w.:-]/.test(src[j])) j++
    for (;;) {
      while (j < n && /\s/.test(src[j])) j++
      if (j >= n) return -1
      if (src.startsWith('/>', j)) return j + 2
      if (src[j] === '>') {
        j++
        break
      }
      if (src[j] === '{') hole()
      else if (src[j] === '"' || src[j] === "'") j = quoted(src, j + 1, src[j], false, true)
      else j++
    }
  }
  while (j < n) {
    if (src.startsWith('</', j)) {
      const close = src.indexOf('>', j)
      return close < 0 ? -1 : close + 1
    }
    if (src[j] === '<' && /[A-Za-z>]/.test(src[j + 1] ?? '')) {
      j = jsxEnd(src, j, holes)
      if (j < 0) return -1
    } else if (src[j] === '{') hole()
    else j++
  }
  return -1
}

function regexEnd(src: string, i: number): number {
  let j = i + 1
  let inClass = false
  while (j < src.length) {
    const d = src[j]
    if (d === '\n') return -1
    if (d === '\\') {
      j += 2
      continue
    }
    if (d === '[') inClass = true
    else if (d === ']') inClass = false
    else if (d === '/' && !inClass) {
      j++
      while (isIdent(src[j])) j++
      return j
    }
    j++
  }
  return -1
}

export function lex(src: string, s: Spec): Found[] {
  const out: Found[] = []
  const n = src.length
  let i = 0
  let line = 1
  let lineStart = 0
  let depth = 0
  let prev = ''
  let prevWord = ''
  let heredocs: { word: string; strip: boolean; comment: boolean; line: number; raw: string }[] = []
  const stack: { cond: boolean; object: boolean; bare: boolean; value: boolean }[] = []
  let wordBefore = ''
  let fnValue = false
  let braceValue = false
  const add = (at: number, raw: string, key = raw) => out.push({ line: at, text: shown(raw), key: keyOf(key) })
  const restIsEmpty = (j: number, marker: string) => {
    const eol = src.indexOf('\n', j)
    const rest = src.slice(j, eol < 0 ? n : eol).trim()
    return rest === '' || rest === ';' || (marker !== '' && rest.startsWith(marker))
  }
  const quote = (at: number, from: number) => {
    const c = src[at]
    const triple = s.triple && src.startsWith(c.repeat(3), at)
    const q = triple ? c.repeat(3) : c
    const escapes = !((s.shell || s.toml) && c === "'") && !s.raw.includes(c)
    let end = quoted(src, at + q.length, q, escapes, triple || s.multiline.includes(c))
    if (s.toml && triple) for (let extra = 0; extra < 2 && src[end] === c; extra++) end++
    const lead = src.slice(lineStart, from)
    const continued = lineStart >= 2 && src[lineStart - 2] === '\\'
    if (s.py && !continued) {
      const wrapped = /^[ \t]*(\(+)[ \t]*$/.exec(lead)
      const k = wrapped ? wrapped[1].length : 0
      const eol = src.indexOf('\n', end)
      const rest = src.slice(end, eol < 0 ? n : eol)
      const closes = new RegExp('^[ \\t]*\\){' + k + '}[ \\t]*(;.*|#.*)?$')
      const opens = k > 0 || /^[ \t]*$/.test(lead) || /^[ \t]*(async[ \t]+)?(def|class)\b.*:[ \t]*$/.test(lead)
      const grouped = depth > 0 && stack.length === depth && stack.every((x) => x.bare) && /^[ \t]*\(*[ \t]*$/.test(lead) && /^\s*\)/.test(src.slice(end))
      if ((opens && depth === k && closes.test(rest)) || grouped) add(line, src.slice(from, end))
    }
    if (s.shell && /^[ \t]*:[ \t]+$/.test(lead) && restIsEmpty(end, '#')) add(line, src.slice(lineStart, end).trim())
    const lines = newlines(src, at, end)
    if (lines) {
      line += lines
      lineStart = src.lastIndexOf('\n', end - 1) + 1
    }
    prev = 'a'
    prevWord = ''
    i = end
  }
  while (i < n) {
    const c = src[i]
    if (c === '\n') {
      const ended = src.slice(lineStart, i)
      line++
      i++
      lineStart = i
      const scalar = s.yaml ? /(^|[\s:-])[|>]([-+]?)([1-9]?)[-+]?[ \t]*(#.*)?$/.exec(ended) : null
      if (scalar && /:|^[ \t]*-/.test(ended)) {
        const lead = /^([ \t]*)((?:-[ \t]+)*)/.exec(ended) as RegExpExecArray
        const keyCol = /^[|>]/.test(ended.slice(lead[0].length)) && lead[2] ? lead[1].length + lead[2].lastIndexOf('-') : lead[0].length
        let content = scalar[3] ? keyCol + Number(scalar[3]) : -1
        while (i < n) {
          const end = src.indexOf('\n', i)
          const row = src.slice(i, end < 0 ? n : end)
          const indent = (/^[ \t]*/.exec(row) as RegExpExecArray)[0].length
          if (row.trim() !== '' && content < 0) content = indent
          if (row.trim() !== '' && (indent <= keyCol || indent < content)) break
          if (end < 0) {
            i = n
            break
          }
          i = end + 1
          line++
          lineStart = i
        }
        continue
      }
      const queue = heredocs
      heredocs = []
      for (const h of queue) {
        const bodyStart = i
        let bodyEnd = n
        while (i < n) {
          const end = src.indexOf('\n', i)
          const row = src.slice(i, end < 0 ? n : end)
          const last = (h.strip ? row.replace(/^\t+/, '') : row) === h.word
          if (last) bodyEnd = i
          i = end < 0 ? n : end + 1
          if (end >= 0) line++
          lineStart = i
          if (last) break
        }
        if (h.comment) add(h.line, h.raw, h.raw + '\n' + src.slice(bodyStart, bodyEnd))
      }
      if (queue.length) continue
      continue
    }
    if (c === ' ' || c === '\t' || c === '\r') {
      i++
      continue
    }
    const long = s.lua ? longBracket(src, i) : null
    if (long) {
      const closer = ']' + long[2] + ']'
      const close = src.indexOf(closer, i + long[0].length)
      const end = close < 0 ? n : close + closer.length
      if (long[1]) add(line, src.slice(i, end))
      const lines = newlines(src, i, end)
      if (lines) {
        line += lines
        lineStart = src.lastIndexOf('\n', end - 1) + 1
      }
      i = end
      prev = 'a'
      continue
    }
    const block = s.block.find(([open]) => src.startsWith(open, i))
    if (block) {
      const close = s.nested ? nestedClose(src, i + block[0].length, block[0], block[1]) : src.indexOf(block[1], i + block[0].length)
      const end = close < 0 ? n : close + block[1].length
      const body = src.slice(i + block[0].length, close < 0 ? n : close)
      if (!isDirective(body, line, block[0])) add(line, src.slice(i, end))
      const lines = newlines(src, i, end)
      if (lines) {
        line += lines
        lineStart = src.lastIndexOf('\n', end - 1) + 1
      }
      i = end
      continue
    }
    const marker = s.line.find((m) => src.startsWith(m, i))
    const hashOk =
      marker !== '#' ||
      ((!s.wordHash || i === lineStart || /[\s;|&()]/.test(src[i - 1])) && !(s.php && src[i + 1] === '[') && !(i > lineStart && s.hashGuard.includes(src[i - 1])))
    if (marker && hashOk) {
      const eol = src.indexOf('\n', i)
      const stop = eol < 0 ? n : eol
      const body = src.slice(i + marker.length, stop)
      if (!isDirective(body, line, marker)) add(line, src.slice(i, stop).trimEnd())
      i = stop
      continue
    }
    if (s.url && /^url\(/i.test(src.slice(i, i + 4))) {
      const close = src.indexOf(')', i)
      i = close < 0 ? n : close + 1
      continue
    }
    if (s.rust && c === 'r' && !isIdent(src[i - 1]) && /^r#*"/.test(src.slice(i, i + 12))) {
      const m = /^r(#*)"/.exec(src.slice(i, i + 12)) as RegExpExecArray
      const closer = '"' + m[1]
      const close = src.indexOf(closer, i + m[0].length)
      const end = close < 0 ? n : close + closer.length
      line += newlines(src, i, end)
      i = end
      prev = 'a'
      continue
    }
    if (s.rust && c === "'") {
      const m = /^'(\\.[^'\n]{0,8}|[^'\\\n])'/.exec(src.slice(i, i + 12))
      i += m ? m[0].length : 1
      prev = m ? 'a' : "'"
      continue
    }
    if (s.template && c === String.fromCharCode(96)) {
      const holes: [number, number][] = []
      const end = template(src, i + 1, holes)
      for (const [a, b] of holes) for (const f of lex(src.slice(a, b), s)) out.push({ ...f, line: f.line + line - 1 + newlines(src, i, a) })
      const lines = newlines(src, i, end)
      if (lines) {
        line += lines
        lineStart = src.lastIndexOf('\n', end - 1) + 1
      }
      i = end
      prev = 'a'
      continue
    }
    const regexOk = prev === '' || REGEX_AFTER.includes(prev) || (prev === 'a' && KEYWORDS.has(prevWord))
    if (s.jsx && c === '<' && regexOk && /[A-Za-z>]/.test(src[i + 1] ?? '')) {
      const holes: [number, number][] = []
      const end = jsxEnd(src, i, holes)
      if (end >= 0) {
        for (const [a, b] of holes) for (const f of lex(src.slice(a, b), s)) out.push({ ...f, line: f.line + line - 1 + newlines(src, i, a) })
        const lines = newlines(src, i, end)
        if (lines) {
          line += lines
          lineStart = src.lastIndexOf('\n', end - 1) + 1
        }
        i = end
        prev = 'a'
        prevWord = ''
        continue
      }
    }
    if (s.tags && c === '<' && /[A-Za-z/!?]/.test(src[i + 1] ?? '')) {
      let j = i + 1
      while (j < n && src[j] !== '>') j = src[j] === '"' || src[j] === "'" ? quoted(src, j + 1, src[j], false, true) : j + 1
      const end = Math.min(n, j + 1)
      const lines = newlines(src, i, end)
      if (lines) {
        line += lines
        lineStart = src.lastIndexOf('\n', end - 1) + 1
      }
      i = end
      continue
    }
    if (s.dollar && c === '$') {
      const m = /^\$([A-Za-z_]\w*)?\$/.exec(src.slice(i, i + 64))
      if (m) {
        const close = src.indexOf(m[0], i + m[0].length)
        const end = close < 0 ? n : close + m[0].length
        const statement = src.slice(Math.max(src.lastIndexOf(';', i - 1), 0), i)
        if (/\b(function|procedure|do)\b/i.test(statement) || /\bas\s*$/i.test(statement)) {
          const body = i + m[0].length
          for (const f of lex(src.slice(body, close < 0 ? n : close), s)) out.push({ ...f, line: f.line + line - 1 + newlines(src, i, body) })
        }
        const lines = newlines(src, i, end)
        if (lines) {
          line += lines
          lineStart = src.lastIndexOf('\n', end - 1) + 1
        }
        i = end
        prev = 'a'
        continue
      }
    }
    if (s.quotes.includes(c) && (!s.tokenQuotes || i === lineStart || /[\s:[{,=(-]/.test(src[i - 1]))) {
      quote(i, i)
      continue
    }
    if (s.regex && c === '/' && regexOk) {
      const end = regexEnd(src, i)
      if (end > 0) {
        i = end
        prev = 'a'
        prevWord = ''
        continue
      }
    }
    if (s.shell && src.startsWith('<<', i) && src[i + 2] !== '<') {
      const m = /^<<(-?)[ \t]*(['"]?)([A-Za-z_][\w-]*)\2/.exec(src.slice(i, i + 80))
      if (m) {
        heredocs.push({ word: m[3], strip: m[1] === '-', comment: /^[ \t]*:[ \t]*$/.test(src.slice(lineStart, i)), line, raw: src.slice(lineStart, i + m[0].length).trim() })
        i += m[0].length
        continue
      }
    }
    if (isIdent(c)) {
      let j = i
      while (j < n && isIdent(src[j])) j++
      const word = src.slice(i, j)
      if (s.py && /^[rRbBuUfF]{1,2}$/.test(word) && (src[j] === '"' || src[j] === "'")) {
        quote(j, i)
        continue
      }
      if (word === 'function') fnValue = (prev !== '' && OBJECT_AFTER.includes(prev)) || (prev === 'a' && KEYWORDS.has(prevWord))
      wordBefore = prev === 'a' ? prevWord : ''
      prev = 'a'
      prevWord = word
      i = j
      continue
    }
    if (c === '(') {
      const cond = prev === 'a' && (CONDITIONS.has(prevWord) || (prevWord === 'await' && wordBefore === 'for'))
      const bare = s.py && stack.every((x) => x.bare) && /^[ \t]*\(*$/.test(src.slice(lineStart, i))
      stack.push({ cond, object: false, bare, value: fnValue })
      fnValue = false
    } else if (c === '{') {
      stack.push({ cond: false, object: braceValue || (prev !== '' && OBJECT_AFTER.includes(prev)) || (prev === 'a' && KEYWORDS.has(prevWord)), bare: false, value: false })
      braceValue = false
    } else if (c === '[') stack.push({ cond: false, object: false, bare: false, value: false })
    else if (c === ';') braceValue = false
    if ('([{'.includes(c)) depth++
    else if (')]}'.includes(c)) depth = Math.max(0, depth - 1)
    const top = ')]}'.includes(c) ? stack.pop() : undefined
    if (c === ')' && top?.value) braceValue = true
    prev = (c === ')' && top?.cond) || (c === '}' && top && !top.object) ? ';' : c === '}' ? ')' : c
    if ((c === '+' || c === '-') && src[i - 1] === c && /[\w$)\]]/.test(src[i - 2] ?? '')) prev = 'a'
    prevWord = ''
    i++
  }
  return out
}

function shifted(found: Found[], src: string, at: number): Found[] {
  const offset = newlines(src, 0, at)
  return found.map((f) => ({ ...f, line: f.line + offset }))
}

function markup(src: string): Found[] {
  const out: Found[] = []
  let last = 0
  if (src.startsWith('---\n')) {
    const close = src.indexOf('\n---', 4)
    if (close > 0) {
      out.push(...shifted(lex(src.slice(4, close), JS), src, 4))
      last = close + 4
    }
  }
  const n = src.length
  const tagEnd = (j: number) => {
    while (j < n && src[j] !== '>') j = src[j] === '"' || src[j] === "'" ? quoted(src, j + 1, src[j], false, true) : j + 1
    return Math.min(n, j + 1)
  }
  let i = last
  while (i < n) {
    if (src.startsWith('<!--', i)) {
      const close = src.indexOf('-->', i + 4)
      i = close < 0 ? n : close + 3
      continue
    }
    const m = /^<(script|style)\b/i.exec(src.slice(i, i + 8))
    if (m) {
      const inner = tagEnd(i + m[0].length)
      const close = new RegExp('</' + m[1] + '\\s*>', 'i').exec(src.slice(inner))
      const innerEnd = close ? inner + close.index : n
      out.push(...shifted(lex(src.slice(last, inner), HTML), src, last))
      out.push(...shifted(lex(src.slice(inner, innerEnd), m[1].toLowerCase() === 'style' ? SCSS : JS), src, inner))
      last = i = innerEnd
      continue
    }
    i = src[i] === '<' && /[A-Za-z]/.test(src[i + 1] ?? '') ? tagEnd(i + 1) : i + 1
  }
  out.push(...shifted(lex(src.slice(last), HTML), src, last))
  return out
}

function phpEnd(src: string, j: number, docs: [number, number][] = []): number {
  const n = src.length
  while (j < n) {
    if (src.startsWith('?>', j)) return j
    const c = src[j]
    if (c === "'" || c === '"' || c === String.fromCharCode(96)) {
      j = quoted(src, j + 1, c, true, true)
      continue
    }
    if (src.startsWith('/*', j)) {
      const k = src.indexOf('*/', j + 2)
      j = k < 0 ? n : k + 2
      continue
    }
    if (src.startsWith('//', j) || (c === '#' && src[j + 1] !== '[')) {
      const eol = src.indexOf('\n', j)
      const close = src.indexOf('?>', j)
      const stop = eol < 0 ? n : eol
      if (close >= 0 && close < stop) return close
      j = stop
      continue
    }
    const doc = src.startsWith('<<<', j) ? /^<<<[ \t]*(['"]?)([A-Za-z_]\w*)\1\r?\n/.exec(src.slice(j, j + 120)) : null
    if (doc) {
      const body = j + doc[0].length
      const close = new RegExp('(^|\\n)[ \\t]*' + doc[2] + '(?=[ \\t]*[;,)\\]]*[ \\t]*(\\r?\\n|$))').exec(src.slice(body))
      const end = close ? body + close.index + close[0].length : n
      docs.push([body, end])
      j = end
      continue
    }
    j++
  }
  return n
}

const PHP_OPEN = /<\?(php\b|=|(?=\s))/gi

function phpFile(src: string): Found[] {
  const out: Found[] = []
  let html = ''
  let i = 0
  for (;;) {
    PHP_OPEN.lastIndex = i
    const open = PHP_OPEN.exec(src)
    if (!open) break
    const start = open.index + open[0].length
    const docs: [number, number][] = []
    const close = phpEnd(src, start, docs)
    const end = Math.min(src.length, close + 2)
    html += src.slice(i, open.index) + src.slice(open.index, end).replace(/[^\n]/g, ' ')
    let code = src.slice(start, close)
    for (const [from, to] of docs) code = code.slice(0, from - start) + code.slice(from - start, to - start).replace(/[^\n]/g, ' ') + code.slice(to - start)
    out.push(...shifted(lex(code, PHP), src, start))
    i = end
  }
  html += src.slice(i)
  return [...markup(html), ...out].sort((x, y) => x.line - y.line)
}

function notebookComments(text: string): Found[] {
  let nb: { metadata?: { language_info?: { name?: string }; kernelspec?: { language?: string } }; cells?: { cell_type?: string; source?: string | string[] }[] }
  try {
    nb = JSON.parse(text)
  } catch {
    return []
  }
  const lang = String(nb.metadata?.language_info?.name ?? nb.metadata?.kernelspec?.language ?? 'python').toLowerCase()
  const s = BY_EXT.get(NOTEBOOK_EXT[lang] ?? 'py')
  if (!s || typeof s === 'string') return []
  const out: Found[] = []
  ;(nb.cells ?? []).forEach((cell, k) => {
    if (cell.cell_type !== 'code') return
    const source = Array.isArray(cell.source) ? cell.source.join('') : String(cell.source ?? '')
    for (const f of lex(source, s)) out.push({ ...f, text: 'cell ' + (k + 1) + ': ' + f.text })
  })
  return out
}

export function findComments(path: string, text: string): Found[] {
  const s = specFor(path, text)
  if (!s) return []
  if (s === 'notebook') return notebookComments(text)
  if (s === 'php') return phpFile(text)
  return s === 'markup' ? markup(text) : lex(text, s)
}

export function newComments(path: string, before: string, after: string): Found[] {
  return subtract(findComments(path, after), findComments(path, before).map((f) => f.key))
}

export function subtract(found: Found[], keys: string[]): Found[] {
  const left = new Map<string, number>()
  for (const k of keys) left.set(k, (left.get(k) ?? 0) + 1)
  return found.filter((f) => {
    const n = left.get(f.key) ?? 0
    if (n > 0) left.set(f.key, n - 1)
    return n === 0
  })
}

export function parseStatus(out: string): { path: string; from: string }[] {
  const parts = out.split('\0')
  const files: { path: string; from: string }[] = []
  for (let k = 0; k < parts.length - 1; ) {
    const status = parts[k]
    if (!status) {
      k++
      continue
    }
    if (status.startsWith('R') || status.startsWith('C')) {
      files.push({ from: parts[k + 1], path: parts[k + 2] })
      k += 3
    } else {
      if (!status.startsWith('D')) files.push({ from: parts[k + 1], path: parts[k + 1] })
      k += 2
    }
  }
  return files
}

export function report(found: Located[], cap = 25): string {
  const lines = found.slice(0, cap).map((f) => '- ' + f.path + ':' + f.line + ': ' + f.text)
  if (found.length > cap) lines.push('- ...and ' + (found.length - cap) + ' more')
  return lines.join('\n')
}

const RULE =
  'No code comments, ever: quality code explains itself. Delete each one (docstrings, JSDoc and commented-out code count too) and say it in the code instead: names, small functions, types. Moving an existing comment is fine; adding or rewording one is not. Tool directives with no free text (a shebang, # type: ignore, // @ts-expect-error, // eslint-disable-next-line rule) may stay. There is no override, and working around this (a string posing as a note, a directive with prose, writing the file through the shell) is the same violation.'

export function refusal(found: Located[]): string {
  return "Refused by fleet's no-comments rule: this write adds " + found.length + ' code comment(s):\n' + report(found).replace(/^- ([^:]+):(\d+): /gm, '- $1 L$2: ') + '\n' + RULE
}

export function holdOpen(found: Located[]): string {
  return "Fleet's no-comments rule: the work in this session adds " + found.length + ' code comment(s), and the turn cannot end until they are gone:\n' + report(found) + '\n' + RULE
}

type Dollar = {
  fs: { read: (p: string) => Promise<unknown>; write: (p: string, t: string) => Promise<void>; exists: (p: string) => Promise<boolean>; stat: (p: string) => Promise<{ mtimeMs: number }> }
  process: { run: (argv: readonly string[], init?: { cwd?: string; timeoutMs?: number }) => Promise<{ exitCode: number; stdout: string; stderr?: string }> }
  store: { get: (k: string) => Promise<unknown>; set: (k: string, v: unknown) => Promise<void>; delete: (k: string) => Promise<void> }
  session: { id: () => Promise<string> }
  env: { get: (k: string) => Promise<string | undefined> }
}

const parent = (p: string) => {
  const cut = p.replace(/\/+$/, '').lastIndexOf('/')
  return cut <= 0 ? '/' : p.slice(0, cut)
}
const join = (a: string, b: string) => (a.endsWith('/') ? a + b : a + '/' + b)

async function read($: Dollar, path: string): Promise<string | undefined> {
  try {
    const t = await $.fs.read(path)
    return typeof t === 'string' ? t : undefined
  } catch {
    return undefined
  }
}

const ABSENT = /: (?:No such file or directory|Not a directory)\s*$/

async function present($: Dollar, path: string): Promise<boolean> {
  if ((await $.fs.exists(path).catch(() => undefined)) === false) return false
  try {
    const r = await $.process.run(['sh', '-c', 'LC_ALL=C exec stat -- "$1"', 'stat', path], { timeoutMs: 20000 })
    return r.exitCode === 0 || !ABSENT.test(r.stderr ?? '')
  } catch {
    return true
  }
}
async function vanished($: Dollar, root: string): Promise<boolean> {
  return !(await present($, join(root, '.git')))
}

async function git($: Dollar, root: string, args: string[]): Promise<string | undefined> {
  try {
    const r = await $.process.run(['git', '-C', root, ...args], { timeoutMs: 20000 })
    return r.exitCode === 0 ? r.stdout : undefined
  } catch {
    return undefined
  }
}

const roots = new Map<string, string>()

async function rootOf($: Dollar, path: string): Promise<string | undefined> {
  let dir = path
  for (let k = 0; k < 64 && dir; k++) {
    const known = roots.get(dir)
    if (known !== undefined) return known || undefined
    if (await $.fs.exists(dir).catch(() => false)) {
      const out = (await git($, dir, ['rev-parse', '--show-toplevel']))?.trim()
      if (out) {
        if (roots.size > 2000) roots.clear()
        roots.set(dir, out)
        return out
      }
    }
    if (dir === '/') break
    dir = parent(dir)
  }
  return undefined
}

const EMPTY_TREE = '4b825dc642cb6eb9a060e54bf8d69288fbee4904'

async function mustGit($: Dollar, root: string, args: string[]): Promise<string> {
  let r: { exitCode: number; stdout: string; stderr?: string }
  try {
    r = await $.process.run(['git', '-C', root, ...args], { timeoutMs: 60000 })
  } catch (error) {
    throw new Error('git ' + args[0] + ' failed: ' + String((error as Error)?.message ?? error))
  }
  if (r.exitCode !== 0) throw new Error('git ' + args[0] + ' failed: ' + (r.stderr ?? '').trim().split('\n')[0])
  return r.stdout
}

async function changed($: Dollar, root: string, base: string): Promise<{ path: string; from: string }[]> {
  const tracked = await mustGit($, root, ['diff', '-z', '--name-status', '-M', '--diff-filter=ACMRT', base || EMPTY_TREE])
  const untracked = await mustGit($, root, ['ls-files', '-z', '--others', '--exclude-standard'])
  const files = [...parseStatus(tracked), ...untracked.split('\0').filter(Boolean).map((p) => ({ path: p, from: '' }))]
  const seen = new Set<string>()
  return files.filter((f) => !seen.has(f.path) && seen.add(f.path) && !SKIPPED.test(f.path))
}

async function baseline($: Dollar, root: string, since = 0): Promise<Baseline> {
  const head = (await git($, root, ['rev-parse', '--verify', '-q', 'HEAD']))?.trim() ?? ''
  const dirty: Record<string, string[]> = {}
  for (const f of await changed($, root, head)) {
    const text = await read($, join(root, f.path))
    if (text === undefined || !isCode(f.path, text)) continue
    if (since && (await $.fs.stat(join(root, f.path)).then((s) => s.mtimeMs, () => 0)) >= since) continue
    dirty[f.path] = findComments(f.path, text).map((x) => x.key)
  }
  return { head, dirty, at: Date.now() }
}

export function minus(a: string[], b: string[]): string[] {
  const left = new Map<string, number>()
  for (const k of b) left.set(k, (left.get(k) ?? 0) + 1)
  return a.filter((k) => {
    const n = left.get(k) ?? 0
    if (n > 0) left.set(k, n - 1)
    return n === 0
  })
}

export function most(a: string[], b: string[]): string[] {
  const count = (keys: string[]) => keys.reduce((m, k) => m.set(k, (m.get(k) ?? 0) + 1), new Map<string, number>())
  const ca = count(a)
  const cb = count(b)
  const out: string[] = []
  for (const k of new Set([...ca.keys(), ...cb.keys()])) for (let n = Math.max(ca.get(k) ?? 0, cb.get(k) ?? 0); n > 0; n--) out.push(k)
  return out
}

const MOVES_ONLY = /\(start\)|^(reset|checkout|fetch|clone|branch)\b|Fast-forward/

async function foreignTip($: Dollar, root: string, from: string, since: number): Promise<string> {
  if (!from) return ''
  const shas = ((await git($, root, ['log', '--format=%H', 'HEAD', '--not', from])) ?? '').split('\n').map((x) => x.trim()).filter(Boolean)
  if (!shas.length) return ''
  const floor = Math.floor(since / 1000) - 1
  const reflog = await git($, root, ['reflog', '--date=unix', '--format=%H %gd %gs'])
  if (reflog === undefined) return ''
  const ours = new Set<string>()
  let recorded = false
  for (const row of reflog.split('\n')) {
    const m = /^(\S+) \S+@\{(\d+)\} (.*)$/.exec(row.trim())
    if (!m || Number(m[2]) < floor) continue
    recorded = true
    if (!MOVES_ONLY.test(m[3])) ours.add(m[1])
  }
  if (recorded) return shas.find((sha) => !ours.has(sha)) ?? ''
  const remote = new Set(((await git($, root, ['log', '--format=%H', '--remotes', '--not', from])) ?? '').split('\n').map((x) => x.trim()).filter(Boolean))
  const older = new Set(
    ((await git($, root, ['log', '--format=%H %ct', 'HEAD', '--not', from])) ?? '')
      .split('\n')
      .map((x) => x.trim().split(' '))
      .filter(([sha, at]) => sha && Number(at) < floor)
      .map(([sha]) => sha),
  )
  return shas.find((sha) => remote.has(sha) || older.has(sha)) ?? ''
}

async function keysAt($: Dollar, root: string, rev: string, path: string): Promise<string[]> {
  if (!rev || !path) return []
  return findComments(path, (await git($, root, ['show', rev + ':' + path])) ?? '').map((x) => x.key)
}

async function surviving($: Dollar, root: string, head: string): Promise<string> {
  if (!head) return ''
  if ((await git($, root, ['cat-file', '-e', head + '^{commit}'])) !== undefined) return head
  return (await git($, root, ['rev-parse', '--git-dir'])) === undefined ? head : ''
}

async function scan($: Dollar, root: string, base: Baseline): Promise<Located[]> {
  const from = await surviving($, root, base.head)
  const upstream = await foreignTip($, root, from, base.at ?? 0)
  const files = await changed($, root, from)
  const listed = new Set(files.flatMap((f) => [f.path, f.from]))
  for (const path of Object.keys(base.dirty)) if (!listed.has(path)) files.push({ path, from: path })
  const released: string[] = []
  const gone = new Set(from ? (await mustGit($, root, ['diff', '-z', '--name-only', '--diff-filter=D', from])).split('\0').filter(Boolean) : [])
  const found: Located[] = []
  for (const f of files) {
    if (neverCode(f.path)) continue
    const after = await read($, join(root, f.path))
    if (after === undefined) {
      if (await $.fs.exists(join(root, f.path)).catch(() => true)) throw new Error('cannot read ' + f.path)
      gone.add(f.path)
      continue
    }
    if (!isCode(f.path, after)) continue
    const kept = base.dirty[f.path] ?? base.dirty[f.from]
    let before = kept ?? (await keysAt($, root, from, f.from))
    if (upstream) before = most(before, await keysAt($, root, upstream, f.path))
    const now = findComments(f.path, after)
    released.push(...minus(before, now.map((x) => x.key)))
    for (const x of subtract(now, before)) found.push({ ...x, path: f.path })
  }
  const orphans: string[] = [...released]
  for (const path of gone) orphans.push(...(base.dirty[path] ?? (await keysAt($, root, from, path))))
  return subtract(found, orphans)
}

const KEEP_DAYS = 3
const sessionKey = (id: string) => 's:' + id

const memory = new Map<string, Session>()
const unsaved = new Set<string>()
const queues = new Map<string, Promise<unknown>>()

function serial<T>(id: string, work: () => Promise<T>): Promise<T> {
  const run = (queues.get(id) ?? Promise.resolve()).then(work, work)
  queues.set(id, run.catch(() => undefined))
  return run
}

async function sessionOf($: Dollar, id: string): Promise<Session> {
  const kept = (await $.store.get(sessionKey(id)).catch(() => undefined)) as Session | undefined
  const live = memory.get(id)
  if (live && unsaved.has(id)) return live
  const session: Session = kept && typeof kept === 'object' && kept.roots ? kept : { at: Date.now(), roots: {} }
  if (!live) return session
  for (const [root, base] of Object.entries(live.roots)) session.roots[root] ??= base
  for (const [path, keys] of Object.entries(live.loose ?? {})) (session.loose ??= {})[path] ??= keys
  session.cwd ??= live.cwd
  session.shells ??= live.shells
  return session
}

async function persist($: Dollar, id: string, session: Session) {
  memory.set(id, session)
  session.at = Date.now()
  const stored = await $.store.set(sessionKey(id), session).then(
    () => true,
    () => false,
  )
  if (stored) unsaved.delete(id)
  else unsaved.add(id)
  const index = ((await $.store.get('sessions').catch(() => undefined)) as Record<string, number> | undefined) ?? {}
  index[id] = session.at
  const floor = Date.now() - KEEP_DAYS * 86400000
  for (const [old, at] of Object.entries(index)) {
    if (at >= floor) continue
    delete index[old]
    memory.delete(old)
    unsaved.delete(old)
    await $.store.delete(sessionKey(old)).catch(() => undefined)
  }
  await $.store.set('sessions', index).catch(() => undefined)
}

function track($: Dollar, id: string, roots: string[], since = 0): Promise<Session> {
  return serial(id, async () => {
    const session = await sessionOf($, id)
    let grew = false
    for (const root of roots) {
      if (session.roots[root]) continue
      session.roots[root] = await baseline($, root, since)
      grew = true
    }
    if (grew || unsaved.has(id)) await persist($, id, session)
    return session
  })
}

function forget($: Dollar, id: string, gone: string[]): Promise<void> {
  return serial(id, async () => {
    const session = await sessionOf($, id)
    const held = gone.filter((root) => root in session.roots)
    if (!held.length) return
    for (const root of held) delete session.roots[root]
    await persist($, id, session)
  })
}

type Planned = { path: string; before: string; after: string }

function replaced(text: string, oldText: string, newText: string, all: boolean): string | undefined {
  if (!oldText) return undefined
  const at = text.indexOf(oldText)
  if (at < 0) return undefined
  return all ? text.split(oldText).join(newText) : text.slice(0, at) + newText + text.slice(at + oldText.length)
}

const NOTEBOOK_EXT: Record<string, string> = { python: 'py', r: 'r', julia: 'jl', javascript: 'js', typescript: 'ts', bash: 'sh', sql: 'sql', scala: 'scala', rust: 'rs', go: 'go' }

async function plan($: Dollar, tool: string, e: Record<string, unknown>): Promise<Planned[] | 'post' | undefined> {
  const str = (v: unknown) => (typeof v === 'string' ? v : '')
  if (tool === 'Write') {
    const path = str(e.file_path)
    return path ? [{ path, before: (await read($, path)) ?? '', after: str(e.content) }] : undefined
  }
  if (tool === 'Edit' || tool === 'MultiEdit') {
    const path = str(e.file_path)
    const before = path ? await read($, path) : undefined
    if (before === undefined) return path && tool === 'Edit' && !str(e.old_string) ? [{ path, before: '', after: str(e.new_string) }] : undefined
    const edits = tool === 'Edit' ? [e] : Array.isArray(e.edits) ? (e.edits as Record<string, unknown>[]) : []
    let after = before
    for (const edit of edits) {
      const next = replaced(after, str(edit.old_string), str(edit.new_string), edit.replace_all === true)
      if (next === undefined) return undefined
      after = next
    }
    return [{ path, before, after }]
  }
  if (tool === 'NotebookEdit') {
    const path = str(e.notebook_path)
    if (!path || e.edit_mode === 'delete' || e.cell_type === 'markdown') return undefined
    const raw = await read($, path)
    let lang = 'python'
    let before = ''
    try {
      const nb = JSON.parse(raw ?? '{}') as { metadata?: { language_info?: { name?: string }; kernelspec?: { language?: string } }; cells?: { id?: string; cell_type?: string; source?: string | string[] }[] }
      lang = String(nb.metadata?.language_info?.name ?? nb.metadata?.kernelspec?.language ?? 'python').toLowerCase()
      const cell = e.edit_mode === 'insert' ? undefined : nb.cells?.find((c) => c.id === e.cell_id)
      if (cell?.cell_type === 'markdown' && e.cell_type !== 'code') return undefined
      before = Array.isArray(cell?.source) ? cell.source.join('') : (cell?.source ?? '')
    } catch {
      before = ''
    }
    return [{ path: 'cell.' + (NOTEBOOK_EXT[lang] ?? 'py'), before, after: str(e.new_source) }].map((p) => ({ ...p, shownAs: path })) as Planned[]
  }
  if (/^mcp__lean[-_]ctx__ctx_patch$/.test(tool)) {
    const path = str(e.path)
    if (e.op === 'create' && path && !Array.isArray(e.ops)) return [{ path, before: (await read($, path)) ?? '', after: str(e.new_text) || str(e.replace) }]
    return 'post'
  }
  return undefined
}

function patchPaths(e: Record<string, unknown>): string[] {
  const ops = Array.isArray(e.ops) ? (e.ops as Record<string, unknown>[]) : []
  const paths = [e.path, ...ops.map((o) => o.path)].filter((p): p is string => typeof p === 'string' && p !== '')
  return [...new Set(paths)]
}

const tracked = new Set<string>()
const LOADED = Date.now()

async function remember($: Dollar, paths: string[]) {
  const id = (await $.session.id().catch(() => '')) || 'unknown'
  const found = new Set<string>()
  for (const p of paths) {
    const root = await rootOf($, p)
    if (root && !tracked.has(id + '\n' + root)) found.add(root)
  }
  if (!found.size) return
  await track($, id, [...found])
  if (unsaved.has(id)) return
  for (const root of found) tracked.add(id + '\n' + root)
}

const PATHISH = /(?:^|[\s'"=:(<>])((?:\/|~\/|\.\.?\/)[^\s'"`;|&<>()]*)/g
const CD = /(?:^|[;&|(]|\s)cd\s+(['"]?)([^\s;&|'"]+)\1|(?:^|\s)-C\s+(['"]?)([^\s;&|'"]+)\3/g

export function touched(e: Record<string, unknown>, home = ''): string[] {
  const cwd = typeof e.cwd === 'string' ? e.cwd : ''
  const command = typeof e.command === 'string' ? e.command : ''
  const place = (p: string) => (p.startsWith('~/') ? (home ? join(home, p.slice(2)) : '') : p.startsWith('/') || !cwd ? p : join(cwd, p))
  const out = cwd ? [cwd] : []
  for (const m of command.matchAll(PATHISH)) out.push(place(m[1]))
  for (const m of command.matchAll(CD)) out.push(place(m[2] || m[4]))
  return [...new Set(out.filter(Boolean))].slice(0, 32)
}

const WORD = /(?:'[^'\n]*'|"[^"\n]*"|\\.|[^\s'"`;&|<>()\\])+/g
const SEGMENT = /'([^'\n]*)'|"([^"\n]*)"|\\(.)|([^'"\\]+)/g

export function named(command: string, cwd: string, home = ''): string[] {
  return shellWords(command, cwd ? [cwd] : [], home).paths
}

export function shellWords(command: string, cwds: string[], home = ''): { paths: string[]; dirs: string[]; moved: string[] } {
  const here = String.fromCharCode(0)
  const out: string[] = []
  const vars = new Map<string, string>()
  const scopes: string[][] = []
  const moved: string[] = []
  let current = [...new Set(cwds.filter(Boolean))]
  let changing = false
  let first = true
  let last = 0
  const expand = (text: string) => text.replace(/\$\{?([A-Za-z_]\w*)\}?/g, (all, name: string) => (name === 'HOME' && home ? home : name === 'PWD' ? here : (vars.get(name) ?? all)))
  const unquote = (raw: string) => {
    let text = ''
    for (const p of raw.matchAll(SEGMENT)) text += p[1] ?? (p[2] !== undefined ? expand(p[2]) : (p[3] ?? expand(p[4] ?? '')))
    return text
  }
  const tidy = (p: string) => p.replace(/\/\.$/, '')
  const resolve = (word: string): string[] => {
    const expanded = word.replace(/^~(?=\/|$)/, home ? home : '~')
    if (expanded.includes('$') || expanded.startsWith('~') || expanded.lastIndexOf(here) > 0) return []
    if (expanded.startsWith(here)) return current.map((d) => tidy(d + expanded.slice(1)))
    if (expanded.startsWith('/')) return [tidy(expanded)]
    return current.map((d) => tidy(join(d, expanded.replace(/^\.\//, ''))))
  }
  for (const m of command.matchAll(WORD)) {
    const at = m.index ?? 0
    const gap = command.slice(last, at)
    for (const c of gap) {
      if (c === '(') scopes.push(current)
      else if (c === ')') current = scopes.pop() ?? current
    }
    const starts = first || /[;&|(\n]/.test(gap)
    first = false
    last = at + m[0].length
    const word = unquote(m[0])
    if (changing) {
      changing = false
      const targets = resolve(word)
      current = [...new Set([...targets, ...current])].slice(0, 8)
      if (!scopes.length) moved.push(...targets)
      continue
    }
    if (starts && (word === 'cd' || word === 'pushd')) {
      changing = true
      continue
    }
    const assigned = starts ? /^([A-Za-z_]\w*)=([\s\S]*)$/.exec(word) : null
    if (assigned) {
      vars.set(assigned[1], assigned[2])
      continue
    }
    if (!word || word.includes('\\') || word.startsWith('-') || specFor(word, '') === undefined) continue
    out.push(...resolve(word))
  }
  return { paths: [...new Set(out)].slice(0, 64), dirs: current, moved: [...new Set(moved)] }
}

async function isLoose($: Dollar, path: string): Promise<boolean> {
  const root = await rootOf($, path)
  if (!root) return true
  return (await git($, root, ['check-ignore', '-q', '--', path])) !== undefined
}

function watchLoose($: Dollar, id: string, paths: string[]): Promise<void> {
  return serial(id, async () => {
    const session = await sessionOf($, id)
    let grew = false
    for (const path of paths) {
      if (session.loose?.[path] || !(await isLoose($, path))) continue
      const text = await read($, path)
      ;(session.loose ??= {})[path] = text === undefined ? [] : findComments(path, text).map((x) => x.key)
      grew = true
    }
    if (grew) await persist($, id, session)
  })
}

async function looseFound($: Dollar, loose: Record<string, string[]>, found: Located[], unchecked: string[]) {
  for (const [path, keys] of Object.entries(loose)) {
    if (neverCode(path)) continue
    const now = await read($, path)
    if (now === undefined) {
      if (await present($, path)) unchecked.push(path + ': cannot read it')
      continue
    }
    if (!isCode(path, now)) continue
    for (const x of subtract(findComments(path, now), keys)) found.push({ ...x, path })
  }
}

async function shellCwds($: Dollar, id: string): Promise<string[]> {
  const session = await sessionOf($, id).catch(() => undefined)
  return session?.shells ?? (session?.cwd ? [session.cwd] : [])
}

function moveShell($: Dollar, id: string, dirs: string[]): Promise<void> {
  return serial(id, async () => {
    const session = await sessionOf($, id)
    const kept = [...new Set([...[...dirs].reverse(), ...(session.shells ?? (session.cwd ? [session.cwd] : []))])]
    const next = session.cwd && !kept.slice(0, 4).includes(session.cwd) ? [...kept.slice(0, 3), session.cwd] : kept.slice(0, 4)
    if (JSON.stringify(next) === JSON.stringify(session.shells)) return
    session.shells = next
    await persist($, id, session)
  })
}

async function movedCredit($: Dollar, root: string, x: Located): Promise<number> {
  const line = x.text.replace(/\.\.\.$/, '').split('\n')[0].trim()
  if (!line) return 0
  const listed = async (options: string[], revs: string[]) => ((await git($, root, ['grep', '-F', '-l', ...options, '-e', line, ...revs])) ?? '').split('\n').map((h) => h.trim().replace(/^HEAD:/, '')).filter(Boolean)
  const count = (path: string, text: string) => findComments(path, text).filter((c) => c.key === x.key).length
  const was = await listed([], ['HEAD'])
  const now = await listed(['--untracked'], [])
  if (was.length > 200 || now.length > 200) return 0
  let credit = 0
  for (const rel of was) credit += (await keysAt($, root, 'HEAD', rel)).filter((k) => k === x.key).length
  for (const rel of now) credit -= count(rel, (await read($, join(root, rel))) ?? '')
  return credit
}

async function notMoved($: Dollar, found: Located[]): Promise<Located[]> {
  const credit = new Map<string, number>()
  const left: Located[] = []
  for (const x of found) {
    const root = await rootOf($, x.path)
    const slot = (root ?? '') + '\n' + x.key
    if (!credit.has(slot)) credit.set(slot, root ? await movedCredit($, root, x).catch(() => 0) : 0)
    const n = credit.get(slot) ?? 0
    if (n > 0) credit.set(slot, n - 1)
    else left.push(x)
  }
  return left
}

type Next = ((e: unknown) => Promise<unknown>) & { called?: boolean }

export async function guardTool($: Dollar, e: Record<string, unknown>, next: Next): Promise<unknown> {
  const tool = String(e.tool)
  const home = (await $.env.get('HOME').catch(() => undefined)) ?? ''
  const id = (await $.session.id().catch(() => '')) || 'unknown'
  const command = typeof e.command === 'string' ? e.command : ''
  const given = typeof e.cwd === 'string' && e.cwd ? e.cwd : ''
  const cwds = given ? [given] : command ? await shellCwds($, id) : []
  const shellMoves: string[] = []
  const places = cwds.length ? [...new Set(cwds.flatMap((cwd) => touched({ ...e, cwd }, home)))] : touched(e, home)
  if (places.length) await remember($, places).catch(() => undefined)
  if (command) {
    const parsed = shellWords(command, cwds, home)
    await watchLoose($, id, parsed.paths).catch(() => undefined)
    if (parsed.moved.length) {
      await remember($, parsed.moved).catch(() => undefined)
      if (!given) shellMoves.push(...parsed.moved)
    }
  }
  const settle = async (result: unknown) => {
    const real: string[] = []
    for (const d of shellMoves) if (await $.fs.exists(d).catch(() => false)) real.push(d)
    if (real.length) await moveShell($, id, real).catch(() => undefined)
    return result
  }
  let planned: Planned[] | 'post' | undefined
  try {
    planned = await plan($, tool, e)
  } catch {
    return settle(await next(e))
  }
  if (planned === undefined) return settle(await next(e))
  if (planned !== 'post') {
    const added: Located[] = planned.flatMap((p) => newComments(p.path, p.before, p.after).map((f) => ({ ...f, path: (p as Planned & { shownAs?: string }).shownAs ?? p.path })))
    const found = added.length && !planned.some((p) => (p as Planned & { shownAs?: string }).shownAs) ? await notMoved($, added) : added
    if (found.length) return { deny: refusal(found) }
    await remember($, planned.map((p) => (p as Planned & { shownAs?: string }).shownAs ?? p.path)).catch(() => undefined)
    return settle(await next(e))
  }
  const paths = patchPaths(e)
  await watchLoose($, (await $.session.id().catch(() => '')) || 'unknown', paths).catch(() => undefined)
  const before = new Map<string, string | undefined>()
  for (const p of paths) before.set(p, await read($, p))
  await remember($, paths).catch(() => undefined)
  const result = await next(e)
  const added: Located[] = []
  const touchedNow: string[] = []
  for (const p of paths) {
    const after = await read($, p)
    if (after !== before.get(p)) touchedNow.push(p)
    if (after === undefined) continue
    for (const f of newComments(p, before.get(p) ?? '', after)) added.push({ ...f, path: p })
  }
  const found = added.length ? await notMoved($, added) : added
  if (!found.length) return result
  return {
    deny:
      refusal(found) +
      '\nThe patch already ran, and these files stay exactly as it left them, so nothing anyone else wrote is lost: ' +
      touchedNow.join(', ') +
      ': delete the comments listed above from them now, before anything else; the turn cannot end while they remain.',
  }
}

export async function holdStop($: Dollar, e: { session_id?: string; cwd?: string }, next: Next): Promise<unknown> {
  const result = (await next(e)) as { block?: string } | undefined
  if (result?.block) return result
  try {
    const id = e.session_id || (await $.session.id().catch(() => '')) || 'unknown'
    const here = e.cwd ? await rootOf($, e.cwd) : undefined
    const unchecked: string[] = []
    const session = await track($, id, here ? [here] : [], LOADED).catch(async (error) => {
      unchecked.push((here ?? e.cwd ?? 'this session') + ': ' + String((error as Error)?.message ?? error))
      return sessionOf($, id)
    })
    const own = (await $.session.id().catch(() => '')) || 'unknown'
    if (own !== id) {
      const mine = await sessionOf($, own)
      for (const [root, base] of Object.entries(mine.roots)) session.roots[root] ??= base
      for (const [path, keys] of Object.entries(mine.loose ?? {})) (session.loose ??= {})[path] ??= keys
    }
    const found: Located[] = []
    await looseFound($, session.loose ?? {}, found, unchecked)
    const gone: string[] = []
    for (const [root, base] of Object.entries(session.roots)) {
      if (await vanished($, root)) {
        gone.push(root)
        continue
      }
      const prefix = root === here ? '' : root + '/'
      try {
        for (const f of await scan($, root, base)) found.push({ ...f, path: prefix + f.path })
      } catch (error) {
        unchecked.push(root + ': ' + String((error as Error)?.message ?? error))
      }
    }
    if (gone.length) for (const owner of new Set([id, own])) await forget($, owner, gone).catch(() => undefined)
    if (unchecked.length) {
      const message = "Fleet's no-comments rule could not check this session's work, so the turn cannot end yet:\n" + unchecked.map((u) => '- ' + u).join('\n') + '\nFix what stops the check there (a stale index.lock, a broken repository, a file it may not read) and end the turn again.' + (found.length ? '\n\n' + holdOpen(found) : '')
      return { ...(result ?? {}), block: message }
    }
    if (!found.length) return result
    return { ...(result ?? {}), block: holdOpen(found) }
  } catch {
    return result
  }
}

export async function startSession($: Dollar, e: { session_id?: string; cwd?: string }, next: Next): Promise<unknown> {
  try {
    const id = e.session_id || 'unknown'
    const root = e.cwd ? await rootOf($, e.cwd) : undefined
    if (root) await track($, id, [root])
    const cwd = e.cwd
    if (cwd)
      await serial(id, async () => {
        const session = await sessionOf($, id)
        if (session.cwd === cwd) return
        session.cwd = cwd
        await persist($, id, session)
      })
  } catch {
    return next(e)
  }
  return next(e)
}

export const register: Register = (on) => {
  on('tool.call', async ($, e, next) => (await guardTool($ as unknown as Dollar, e as unknown as Record<string, unknown>, next as unknown as Next)) as never)
  on('classic.SessionStart', async ($, e, next) => (await startSession($ as unknown as Dollar, e, next as unknown as Next)) as never)
  on('classic.Stop', async ($, e, next) => (await holdStop($ as unknown as Dollar, e, next as unknown as Next)) as never).catch(($, e, next) => next(e))
  on('classic.SubagentStop', async ($, e, next) => (await holdStop($ as unknown as Dollar, e, next as unknown as Next)) as never).catch(($, e, next) => next(e))
}
