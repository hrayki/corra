import { For, Show, createEffect, createMemo, createResource } from 'solid-js'
import type { ToolOutput } from '../proto/ToolOutput'
import type { JsonValue } from '../proto/serde_json/JsonValue'
import { shortPath } from '../lib/format'
import { Picture } from './bits'
import { highlightCode, langOfPath } from '../lib/markdown'

export type Summary = { verb: string; detail: string; full?: string; plain?: boolean }

function partialField(json: string | undefined, key: string): string | undefined {
  if (!json) return undefined
  const m = json.match(new RegExp(`"${key}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)`))
  if (!m) return undefined
  try {
    return JSON.parse(`"${m[1]}"`)
  } catch {
    return m[1]
  }
}

const LEGACY_ACP: Record<string, string> = {
  'acp:read': 'Read',
  'acp:edit': 'Edit',
  'acp:delete': 'Delete',
  'acp:move': 'Move',
  'acp:search': 'Grep',
  'acp:execute': 'Bash',
  'acp:think': 'Think',
  'acp:fetch': 'WebFetch',
  'acp:switch_mode': 'SwitchMode',
}

export function canonicalToolName(name: string): string {
  return LEGACY_ACP[name] ?? name
}

export function toolSummary(rawName: string, input: JsonValue | null | undefined, live?: string, cwd?: string): Summary {
  const name = canonicalToolName(rawName)
  const i = (input && typeof input === 'object' && !Array.isArray(input) ? input : {}) as Record<string, any>
  const f = (k: string) => i[k] ?? partialField(live, k)
  const path = (k: string) => shortPath(f(k) ?? f('path') ?? f('title'), cwd)
  switch (name) {
    case 'Bash': {
      const cmd = f('command') ?? f('CommandLine')
      const desc = f('description') ?? (f('title') && f('title') !== cmd ? f('title') : undefined)
      return desc
        ? { verb: 'Ran', detail: String(desc), full: cmd ?? desc, plain: true }
        : { verb: 'Ran', detail: String(cmd ?? f('title') ?? '').split('\n')[0], full: cmd }
    }
    case 'Read':
      return { verb: 'Read', detail: path('file_path') }
    case 'Write':
      return { verb: 'Wrote', detail: path('file_path') }
    case 'Edit':
    case 'MultiEdit':
      return { verb: 'Edited', detail: path('file_path') }
    case 'Delete':
      return { verb: 'Deleted', detail: path('file_path'), plain: true }
    case 'Move':
      return { verb: 'Moved', detail: path('file_path'), plain: true }
    case 'NotebookEdit':
      return { verb: 'Edited', detail: path('notebook_path') }
    case 'Grep':
      return {
        verb: 'Searched',
        detail: `${f('pattern') ?? f('title') ?? ''}${i.path ? `  in ${shortPath(i.path, cwd)}` : ''}`,
      }
    case 'Glob':
      return { verb: 'Listed', detail: String(f('pattern') ?? '') }
    case 'WebFetch':
      return { verb: 'Fetched', detail: String(f('url') ?? f('title') ?? '') }
    case 'WebSearch':
      return { verb: 'Searched the web', detail: String(f('query') ?? '') }
    case 'Think':
      return { verb: 'Thought', detail: String(f('title') ?? ''), plain: true }
    case 'SwitchMode':
      return { verb: 'Switched mode', detail: String(f('title') ?? ''), plain: true }
    case 'Agent':
    case 'Task':
      return { verb: 'Agent', detail: String(f('description') ?? '') }
    case 'TaskCreate':
      return { verb: 'Added task', detail: String(f('subject') ?? '') }
    case 'TaskUpdate':
      return { verb: 'Updated task', detail: [f('taskId'), f('status')].filter(Boolean).join(' → ') }
    case 'TaskList':
    case 'TaskGet':
      return { verb: 'Checked tasks', detail: '' }
    case 'TodoWrite':
      return { verb: 'Updated tasks', detail: '' }
    case 'ToolSearch':
      return {
        verb: 'Loaded tools',
        detail: String(f('query') ?? '')
          .replace(/^select:/, '')
          .replace(/mcp__corra__/g, ''),
      }
    case 'Skill':
      return { verb: 'Skill', detail: String(f('skill') ?? '') }
    case 'AskUserQuestion':
      return { verb: 'Asked you', detail: String(i.questions?.[0]?.question ?? '') }
    case 'ExitPlanMode':
      return { verb: 'Proposed a plan', detail: '' }
    case '':
      return { verb: 'Working', detail: '' }
    case 'acp:other':
      return { verb: String(f('title') ?? 'Tool'), detail: '', plain: true }
    default: {
      const m = name.match(/^mcp__(.+?)__(.+)$/)
      if (m) {
        const what = m[2].replace(/_/g, ' ')
        const arg = Object.values(i).find((v) => typeof v === 'string') as string | undefined
        return {
          verb: m[1] === 'corra' ? what[0].toUpperCase() + what.slice(1) : `${m[1]}: ${what}`,
          detail: arg ? arg.split('\n')[0].slice(0, 120) : '',
        }
      }
      return { verb: name, detail: String(f('title') && f('title') !== name ? f('title') : '') }
    }
  }
}

export function ToolDetail(props: { name: string; input: JsonValue | null | undefined; output?: ToolOutput | null }) {
  const name = () => canonicalToolName(props.name)
  const i = () => (props.input && typeof props.input === 'object' ? props.input : {}) as Record<string, any>
  const out = () => props.output?.text ?? ''
  const hasDiff = () =>
    (name() === 'Edit' && (i().old_string !== undefined || i().new_string !== undefined)) ||
    (name() === 'MultiEdit' && Array.isArray(i().edits))
  const hasWrite = () => name() === 'Write' && i().content !== undefined
  const hasBash = () => name() === 'Bash' && Boolean(i().command ?? i().CommandLine)
  const shown = () => {
    const obj = i()
    if (!props.input || Object.keys(obj).length === 0) return undefined
    const { title: _t, path: _p, ...rest } = obj
    return Object.keys(rest).length > 0 ? rest : undefined
  }
  return (
    <div class="space-y-2">
      <Show when={hasDiff()}>
        <For each={name() === 'MultiEdit' ? ((i().edits as any[]) ?? []) : [i()]}>
          {(e) => <MiniDiff before={String(e.old_string ?? '')} after={String(e.new_string ?? '')} />}
        </For>
      </Show>
      <Show when={hasWrite()}>
        <CodeBlock code={String(i().content ?? '')} path={String(i().file_path ?? '')} />
      </Show>
      <Show when={hasBash()}>
        <Pre text={`$ ${i().command ?? i().CommandLine ?? ''}`} />
      </Show>
      <Show when={!hasDiff() && !hasWrite() && !hasBash() && shown()}>
        <Pre text={JSON.stringify(shown(), null, 2)} />
      </Show>
      <Show when={out() && (!(hasDiff() || hasWrite()) || props.output?.is_error)}>
        <Pre text={out()} error={props.output?.is_error} />
      </Show>
      <For each={props.output?.images ?? []}>
        {(im) =>
          im.type === 'image' ? (
            <Picture class="max-h-80 rounded-lg border border-line" src={`data:${im.media_type};base64,${im.data}`} />
          ) : null
        }
      </For>
    </div>
  )
}

export function Pre(props: { text: string; error?: boolean }) {
  const text = () => (props.text.length > 30000 ? props.text.slice(0, 30000) + '\n…' : props.text)
  return (
    <pre
      class="max-h-72 overflow-auto rounded-lg border border-line bg-panel px-3 py-2 font-mono text-[12px] leading-[1.5] whitespace-pre-wrap"
      classList={{ 'text-bad': props.error }}
    >
      {text()}
    </pre>
  )
}

export function CodeBlock(props: { code: string; path?: string; line?: number }) {
  let el!: HTMLDivElement
  const lang = () => (props.path ? langOfPath(props.path) : undefined)
  const [html] = createResource(
    () => [props.code, lang()] as const,
    ([code, l]) => (l ? highlightCode(code, l) : undefined),
  )
  createEffect(() => {
    const n = props.line
    if (!n || html.loading) return
    html()
    queueMicrotask(() => {
      const row = el?.querySelectorAll<HTMLElement>('.line')[n - 1]
      if (!row) return
      row.classList.add('jumped')
      const left = el.scrollLeft
      row.scrollIntoView({ block: 'center' })
      el.scrollLeft = left
    })
  })
  const plain = () => (props.code.length > 60000 ? props.code.slice(0, 60000) + '\n…' : props.code)
  return (
    <div ref={el} class="max-h-96 overflow-auto rounded-lg border border-line bg-panel font-mono text-[12px] leading-[1.55]">
      <Show
        when={html()}
        fallback={
          <pre class="px-3 py-2 whitespace-pre">
            <Show when={props.line} fallback={plain()}>
              <For each={plain().split('\n')}>{(l) => [<span class="line">{l}</span>, '\n']}</For>
            </Show>
          </pre>
        }
      >
        <div class="[&_pre]:!bg-transparent [&_pre]:px-3 [&_pre]:py-2" innerHTML={html()} />
      </Show>
    </div>
  )
}

type Line = { t: ' ' | '+' | '-'; s: string }

export function lineDiff(a: string, b: string): Line[] {
  const A = a === '' ? [] : a.split('\n')
  const B = b === '' ? [] : b.split('\n')
  if (A.length * B.length > 250000) return [...A.map((s) => ({ t: '-' as const, s })), ...B.map((s) => ({ t: '+' as const, s }))]
  const n = A.length
  const m = B.length
  const dp = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1))
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--) dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
  const out: Line[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (A[i] === B[j]) {
      out.push({ t: ' ', s: A[i] })
      i++
      j++
    } else if (dp[i + 1][j] >= dp[i][j + 1]) out.push({ t: '-', s: A[i++] })
    else out.push({ t: '+', s: B[j++] })
  }
  while (i < n) out.push({ t: '-', s: A[i++] })
  while (j < m) out.push({ t: '+', s: B[j++] })
  return out
}

export function MiniDiff(props: { before: string; after: string }) {
  const lines = createMemo(() => lineDiff(props.before, props.after))
  return (
    <div class="max-h-80 overflow-auto rounded-lg border border-line bg-panel py-1 font-mono text-[12px] leading-[1.5]">
      <For each={lines()}>
        {(l) => (
          <div class="flex px-2" classList={{ 'bg-ok/10': l.t === '+', 'bg-bad/10': l.t === '-' }}>
            <span
              class="w-4 shrink-0 select-none"
              classList={{ 'text-ok': l.t === '+', 'text-bad': l.t === '-', 'text-faint': l.t === ' ' }}
            >
              {l.t === ' ' ? '' : l.t}
            </span>
            <span class="min-w-0 whitespace-pre-wrap">{l.s}</span>
          </div>
        )}
      </For>
    </div>
  )
}
