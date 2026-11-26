import { For, Match, Show, Switch, createEffect, createMemo, createSignal, on, onCleanup, type JSX } from 'solid-js'
import ChevronRight from 'lucide-solid/icons/chevron-right'
import Check from 'lucide-solid/icons/check'
import Circle from 'lucide-solid/icons/circle'
import CircleDot from 'lucide-solid/icons/circle-dot'
import Bell from 'lucide-solid/icons/bell'
import KeyRound from 'lucide-solid/icons/key-round'
import Copy from 'lucide-solid/icons/copy'
import GitFork from 'lucide-solid/icons/git-fork'
import Undo2 from 'lucide-solid/icons/undo-2'
import { call, state } from '../lib/store'
import { highlightIn, renderMarkdown } from '../lib/markdown'
import { fileInCode, fileInHref, joinPath, type FileRef } from '../lib/filelinks'
import { folds, isWork, type Entry, type ItemView, type RequestView, type TurnView } from '../lib/timeline'
import type { InputPart } from '../proto/InputPart'
import type { RequestResponse } from '../proto/RequestResponse'
import { clock, duration } from '../lib/format'
import { openFile, setPanel } from '../lib/panel'
import { Btn, Thumbs, inputClass } from './bits'
import { ToolDetail, toolSummary } from './tools'
import { AgentRow, ThreadRow } from './Agents'
import { isAgentCall, startedThread } from '../lib/agents'

export function TurnBlock(props: { thread: string; turn: TurnView; last: boolean; cwd?: string }) {
  const top = createMemo(() => props.turn.entries.filter((e) => !(e.type === 'item' && e.item.parent)))
  const children = createMemo(() => {
    const m = new Map<string, ItemView[]>()
    for (const e of props.turn.entries) {
      if (e.type === 'item' && e.item.parent) {
        const l = m.get(e.item.parent) ?? []
        l.push(e.item)
        m.set(e.item.parent, l)
      }
    }
    return m
  })
  const done = () => !!props.turn.outcome
  const parts = createMemo(() => folds(props.turn.id, top()))
  const firstFold = () => parts().findIndex((p) => p.work.length > 0)
  const stepsIn = (es: Entry[]) => {
    let n = 0
    const walk = (items: ItemView[]) => {
      for (const it of items) {
        if (it.body.type === 'tool_call') n++
        walk(children().get(it.id) ?? [])
      }
    }
    walk(es.flatMap((e) => (e.type === 'item' ? [e.item] : [])))
    return n
  }
  const foldLabel = (i: number, lead?: Entry) => {
    if (i === firstFold()) return `${props.turn.outcome === 'interrupted' ? 'Stopped after' : 'Worked for'} ${took()}`
    const at = lead?.type === 'item' ? lead.item.at : undefined
    return at && props.turn.ended ? `Continued for ${duration(props.turn.ended - at)}` : 'Continued'
  }
  const took = () => (props.turn.ended && props.turn.started ? duration(props.turn.ended - props.turn.started) : '')
  const changes = createMemo(() => {
    const files = (props.turn.checkpoint ?? []).flatMap((r) => r.files.map((f) => ({ ...f, repo: r.repo })))
    if (!files.length) return undefined
    return { files, added: files.reduce((a, f) => a + f.added, 0), removed: files.reduce((a, f) => a + f.removed, 0) }
  })
  const secrets = createMemo(() =>
    props.turn.entries.flatMap((e) => {
      if (e.type !== 'request' || !e.request.response) return []
      const r = e.request.request
      if (r.type !== 'hub_action' || r.action !== 'secret.request' || e.request.response.type === 'cancelled') return []
      return [{ name: String((r.input as { name?: string })?.name ?? ''), given: e.request.response.type === 'allow' }]
    }),
  )
  const segments = createMemo(() => {
    const out: ({ steps: Entry[] } | { entry: Entry })[] = []
    for (const e of top()) {
      const last = out[out.length - 1]
      if (!isWork(e)) out.push({ entry: e })
      else if (last && 'steps' in last) last.steps.push(e)
      else out.push({ steps: [e] })
    }
    return out
  })
  const render = (e: Entry) => <EntryView thread={props.thread} entry={e} kids={children()} cwd={props.cwd} />
  return (
    <section
      class="group/turn pb-7"
      style={{ 'content-visibility': props.last ? 'visible' : 'auto', 'contain-intrinsic-size': 'auto 220px' }}
    >
      <Show
        when={props.turn.input.length > 0}
        fallback={
          <div class="mb-4">
            <HubNote text={props.turn.cause ?? 'Continued on its own.'} />
          </div>
        }
      >
        <UserMessage thread={props.thread} turn={props.turn} />
      </Show>
      <Show when={!done() || top().length > 0}>
        <div class="space-y-3">
          <Show
            when={done()}
            fallback={
              <>
                <For each={segments()}>
                  {(seg, i) =>
                    'steps' in seg ? (
                      <Steps entries={seg.steps} current={i() === segments().length - 1} render={render} />
                    ) : (
                      render(seg.entry)
                    )
                  }
                </For>
                <Show when={props.last && segments().length === 0}>
                  <Working />
                </Show>
              </>
            }
          >
            <For each={parts()}>
              {(p, i) => (
                <>
                  <Show when={p.lead}>{(l) => render(l())}</Show>
                  <Show when={p.work.length > 0}>
                    <Fold label={foldLabel(i(), p.lead)} steps={stepsIn(p.work)} entries={p.work} render={render} />
                  </Show>
                  <For each={p.rest}>{render}</For>
                </>
              )}
            </For>
          </Show>
        </div>
      </Show>
      <For each={secrets()}>
        {(s) => (
          <div class="mt-3 flex items-center gap-2 text-[12.5px] text-mute">
            <KeyRound size={13} class="shrink-0 text-faint" />
            <span>
              {s.given ? 'You gave the agent' : 'You declined to give'} <span class="font-mono text-[12px]">{s.name}</span>
            </span>
          </div>
        )}
      </For>
      <Show when={props.turn.error && !props.turn.entries.some((e) => e.type === 'notice' && e.text === props.turn.error)}>
        <div
          class="mt-3 rounded-xl border px-3.5 py-2.5 text-[13px]"
          classList={{
            'border-bad/30 bg-bad/5 text-bad': !isLimit(props.turn.error),
            'border-warn/40 bg-warn/10 text-ink': isLimit(props.turn.error),
          }}
        >
          {props.turn.error}
          <Show when={props.last && (state.live[props.thread]?.continue_at ?? 0) > Date.now()}>
            <div class="mt-1 text-[12.5px] text-mute">Corra continues this thread at {clock(state.live[props.thread]!.continue_at!)}.</div>
          </Show>
        </div>
      </Show>
      <Show when={changes()}>
        {(c) => (
          <div class="mt-4 overflow-hidden rounded-xl border border-line">
            <button
              class="flex w-full items-center gap-2 px-3.5 py-2 text-left text-[13px] hover:bg-ink/[0.03]"
              onClick={() => setPanel({ kind: 'diff', turn: props.turn.id })}
            >
              <span class="font-medium">
                {c().files.length} {c().files.length === 1 ? 'file' : 'files'} changed
              </span>
              <span class="num font-mono text-[12px] text-ok">+{c().added}</span>
              <span class="num font-mono text-[12px] text-bad">−{c().removed}</span>
              <span class="flex-1" />
              <span class="text-[12px] text-mute">Open diff</span>
            </button>
            <div class="border-t border-line py-1">
              <For each={c().files.slice(0, 6)}>
                {(f) => (
                  <button
                    class="flex w-full items-center gap-2 px-3.5 py-1 text-left font-mono text-[12px] hover:bg-ink/[0.03]"
                    onClick={() => setPanel({ kind: 'diff', turn: props.turn.id, file: f.path })}
                  >
                    <span class="min-w-0 flex-1 truncate text-mute" style={{ direction: 'rtl', 'text-align': 'left' }}>
                      <bdi>{f.path}</bdi>
                    </span>
                    <span class="num text-ok">+{f.added}</span>
                    <span class="num text-bad">−{f.removed}</span>
                  </button>
                )}
              </For>
              <Show when={c().files.length > 6}>
                <div class="px-3.5 py-1 text-[12px] text-faint">and {c().files.length - 6} more</div>
              </Show>
            </div>
          </div>
        )}
      </Show>
      <Show when={done() && firstFold() < 0 && top().length > 0 && took()}>
        <div class="mt-2 text-[12px] text-faint">
          {props.turn.outcome === 'interrupted' ? 'Stopped · ' : ''}
          {took()}
        </div>
      </Show>
    </section>
  )
}

const agentsIn = (es: Entry[]) => es.filter((e) => e.type === 'item' && (isAgentCall(e.item) || !!startedThread(e.item)))

function Fold(props: { label: string; steps: number; entries: Entry[]; render: (e: Entry) => JSX.Element }) {
  const [open, setOpen] = createSignal(false)
  const agents = createMemo(() => agentsIn(props.entries))
  return (
    <div>
      <button
        class="group flex items-center gap-1 text-[13px] text-mute hover:text-ink"
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <span>
          {props.label}
          <Show when={props.steps > 0}>
            <span class="text-faint">
              {' '}
              · {props.steps} {props.steps === 1 ? 'step' : 'steps'}
            </span>
          </Show>
        </span>
        <ChevronRight size={14} class="transition-transform duration-100" classList={{ 'rotate-90': open() }} />
      </button>
      <Show when={open()}>
        <div class="mt-3 space-y-2.5 border-l border-line pl-4">
          <For each={props.entries}>{props.render}</For>
        </div>
      </Show>
      <Show when={!open() && agents().length > 0}>
        <div class="mt-3 space-y-3">
          <For each={agents()}>{props.render}</For>
        </div>
      </Show>
    </div>
  )
}

function Steps(props: { entries: Entry[]; current: boolean; render: (e: Entry) => JSX.Element }) {
  const [open, setOpen] = createSignal(false)
  const folded = () => (props.current ? props.entries.slice(0, -1) : props.entries)
  const count = () => folded().filter((e) => e.type === 'item' && e.item.body.type === 'tool_call').length
  return (
    <div class="space-y-2.5">
      <Show when={folded().length > 0}>
        <div>
          <button
            class="flex items-center gap-1 text-[13px] text-mute hover:text-ink"
            aria-expanded={open()}
            onClick={() => setOpen(!open())}
          >
            <span>{count() === 0 ? 'Thought' : `${count()} ${count() === 1 ? 'step' : 'steps'}`}</span>
            <ChevronRight size={14} class="transition-transform duration-100" classList={{ 'rotate-90': open() }} />
          </button>
          <Show when={open()}>
            <div class="mt-3 space-y-2.5 border-l border-line pl-4">
              <For each={folded()}>{props.render}</For>
            </div>
          </Show>
          <Show when={!open() && agentsIn(folded()).length > 0}>
            <div class="mt-3 space-y-3">
              <For each={agentsIn(folded())}>{props.render}</For>
            </div>
          </Show>
        </div>
      </Show>
      <Show when={props.current}>{props.render(props.entries[props.entries.length - 1])}</Show>
    </div>
  )
}

function Working() {
  return (
    <div class="flex items-center gap-2 text-[13px] text-mute">
      <span class="size-2 rounded-full bg-accent pulse" />
      Working
    </div>
  )
}

const FIELD_ORDER = ['title', 'repo', 'number', 'push', 'head', 'base', 'sha', 'method']

function hubFields(input: unknown): [string, unknown][] {
  const rank = (k: string) => {
    const i = FIELD_ORDER.indexOf(k)
    return i < 0 ? FIELD_ORDER.length : i
  }
  return Object.entries((input ?? {}) as Record<string, unknown>).sort(([a], [b]) => rank(a) - rank(b))
}

const NOTE = '[corra] '

const actionBtn =
  'inline-flex h-7 items-center gap-1.5 rounded-lg px-2 text-[12.5px] text-mute hover:bg-ink/[0.05] hover:text-ink disabled:pointer-events-none disabled:opacity-40'

function CopyButton(props: { text: () => string; label?: string }) {
  const [done, setDone] = createSignal(false)
  return (
    <button
      type="button"
      class={actionBtn}
      onClick={() => {
        void navigator.clipboard?.writeText(props.text())
        setDone(true)
        setTimeout(() => setDone(false), 1200)
      }}
    >
      <Show when={done()} fallback={<Copy size={13} />}>
        <Check size={13} />
      </Show>
      {done() ? 'Copied' : (props.label ?? 'Copy')}
    </button>
  )
}

const actionRow =
  'flex flex-wrap items-center gap-0.5 transition-opacity duration-100 md:opacity-0 md:group-hover/turn:opacity-100 md:focus-within:opacity-100'

function HubNote(props: { text: string }) {
  return (
    <div class="flex gap-2.5 rounded-xl bg-ink/[0.03] px-3.5 py-2.5 text-[13px] text-mute">
      <Bell size={14} class="mt-[3px] shrink-0 text-faint" />
      <div class="min-w-0 flex-1 [&_.md]:text-[13px]">
        <Markdown text={props.text} />
      </div>
    </div>
  )
}

function UserMessage(props: { thread: string; turn: TurnView }) {
  const text = () => props.turn.input.find((p) => p.type === 'text') as Extract<InputPart, { type: 'text' }> | undefined
  const texts = () => props.turn.input.filter((p) => p.type === 'text')
  const busy = () => {
    const s = state.threads[props.thread]?.status
    return s === 'running' || s === 'waiting' || s === 'starting'
  }
  const [whole, setWhole] = createSignal(false)
  const long = () => {
    const t = text()?.text ?? ''
    return t.length > 1400 || t.split('\n').length > 16
  }
  const note = () => {
    const t = text()?.text
    return props.turn.input.length === 1 && t?.startsWith(NOTE) ? t.slice(NOTE.length) : null
  }
  return (
    <Show
      when={note() === null}
      fallback={
        <div class="mb-4">
          <HubNote text={note()!} />
        </div>
      }
    >
      <div class="mb-2 flex flex-col items-end gap-1">
        <Thumbs parts={props.turn.input} class="mb-1" />
        <Show when={texts().length}>
          <div class="max-w-[85%] rounded-2xl rounded-br-md bg-panel px-4 py-2.5 text-[14px] leading-relaxed whitespace-pre-wrap">
            <div
              classList={{
                'max-h-[15.5rem] overflow-hidden [mask-image:linear-gradient(to_bottom,black_70%,transparent)]': long() && !whole(),
              }}
            >
              <For each={texts()}>{(p) => <span>{p.text}</span>}</For>
            </div>
            <Show when={long()}>
              <button class="mt-1 text-[12.5px] font-medium text-mute hover:text-ink" onClick={() => setWhole(!whole())}>
                {whole() ? 'Show less' : 'Show more'}
              </button>
            </Show>
          </div>
        </Show>
        <div class={`${actionRow} justify-end`}>
          <CopyButton text={() => text()?.text ?? ''} />
          <Show when={state.threads[props.thread]?.agent !== 'antigravity'}>
            <button
              type="button"
              class={actionBtn}
              disabled={!props.turn.outcome}
              title="Start a new thread from this point"
              onClick={() => void forkAt(props.thread, props.turn.id)}
            >
              <GitFork size={13} />
              Fork
            </button>
            <button
              type="button"
              class={actionBtn}
              disabled={busy()}
              title="Remove this message and everything after it, and undo its file changes"
              onClick={() => void rewindTo(props.thread, props.turn.id, text()?.text ?? '')}
            >
              <Undo2 size={13} />
              Rewind
            </button>
          </Show>
        </div>
      </div>
    </Show>
  )
}

export const [draftFor, setDraftFor] = createSignal<{ thread: string; text: string } | null>(null)

async function forkAt(thread: string, turn: string) {
  try {
    const t = await call<{ id: string }>({ op: 'fork_thread', thread, turn })
    location.hash = `#/t/${t.id}`
  } catch (e) {
    alert((e as Error).message)
  }
}

async function rewindTo(thread: string, turn: string, text: string) {
  if (!confirm('Rewind to before this message? Everything after it is removed from the thread, and files go back to how they were.')) return
  try {
    await call({ op: 'rewind_thread', thread, turn })
    setDraftFor({ thread, text })
  } catch (e) {
    alert((e as Error).message)
  }
}

export function EntryView(props: { thread: string; entry: Entry; kids?: Map<string, ItemView[]>; cwd?: string }) {
  return (
    <Switch>
      <Match when={props.entry.type === 'item' && props.entry}>
        {(e) => <ItemBlock thread={props.thread} item={(e() as { item: ItemView }).item} kids={props.kids} cwd={props.cwd} />}
      </Match>
      <Match when={props.entry.type === 'request' && props.entry}>
        {(e) => <RequestCard thread={props.thread} r={(e() as { request: RequestView }).request} cwd={props.cwd} />}
      </Match>
      <Match when={props.entry.type === 'notice' && props.entry}>
        {(e) => {
          const n = e() as { level: string; text: string }
          return (
            <div
              class="rounded-xl px-3.5 py-2 text-[13px]"
              classList={{
                'bg-bad/5 text-bad': n.level === 'error' && !isLimit(n.text),
                'bg-warn/10 text-ink': n.level === 'warning' || (n.level === 'error' && isLimit(n.text)),
                'text-mute bg-ink/[0.03]': n.level === 'info',
              }}
            >
              {n.text}
            </div>
          )
        }}
      </Match>
      <Match when={props.entry.type === 'compacted'}>
        <div class="flex items-center gap-3 text-[12px] text-faint">
          <div class="h-px flex-1 bg-line" />
          Conversation compacted
          <div class="h-px flex-1 bg-line" />
        </div>
      </Match>
    </Switch>
  )
}

function ItemBlock(props: { thread?: string; item: ItemView; kids?: Map<string, ItemView[]>; cwd?: string }) {
  const b = () => props.item.body
  return (
    <Switch>
      <Match when={b().type === 'agent_message'}>
        <div>
          <Markdown text={props.item.live ?? (b() as { text: string }).text} />
          <Show when={props.item.done && ((b() as { text: string }).text?.length ?? 0) > 200}>
            <div class={`${actionRow} mt-1 -ml-2`}>
              <CopyButton text={() => (b() as { text: string }).text} label="Copy reply" />
            </div>
          </Show>
        </div>
      </Match>
      <Match when={b().type === 'reasoning'}>
        <Reasoning item={props.item} />
      </Match>
      <Match when={b().type === 'tool_call' && startedThread(props.item)}>
        <ThreadRow item={props.item} />
      </Match>
      <Match when={b().type === 'tool_call' && isAgentCall(props.item)}>
        <AgentRow thread={props.thread} item={props.item} kids={props.kids} cwd={props.cwd} />
      </Match>
      <Match when={b().type === 'tool_call'}>
        <ToolRow thread={props.thread} item={props.item} kids={props.kids} cwd={props.cwd} />
      </Match>
      <Match when={b().type === 'todos'}>
        <Todos item={props.item} />
      </Match>
      <Match when={b().type === 'user_message'}>
        <div class="flex justify-end">
          <div class="max-w-[85%] rounded-2xl rounded-br-md bg-panel px-4 py-2.5 text-[14px] whitespace-pre-wrap">
            <For each={(b() as { content: InputPart[] }).content}>{(p) => (p.type === 'text' ? <span>{p.text}</span> : null)}</For>
          </div>
        </div>
      </Match>
    </Switch>
  )
}

function copyButton(text: () => string) {
  const b = document.createElement('button')
  b.className = 'copy'
  b.type = 'button'
  b.textContent = 'Copy'
  b.onclick = () => {
    void navigator.clipboard?.writeText(text())
    b.textContent = 'Copied'
    setTimeout(() => (b.textContent = 'Copy'), 1200)
  }
  return b
}

const isLimit = (text?: string) => !!text && /limit/i.test(text) && /reset/i.test(text)

export function Markdown(props: { text: string; base?: string }) {
  let el!: HTMLDivElement
  const html = createMemo(() => renderMarkdown(props.text))
  createEffect(
    on(html, () => {
      queueMicrotask(() => {
        if (!el) return
        void highlightIn(el)
        markFiles(el, props.base)
        for (const pre of el.querySelectorAll('pre')) {
          if (pre.parentElement?.classList.contains('codewrap')) continue
          const wrap = document.createElement('div')
          wrap.className = 'codewrap'
          pre.replaceWith(wrap)
          wrap.append(
            pre,
            copyButton(() => pre.innerText),
          )
        }
      })
    }),
  )
  return <div ref={el} class="md text-[14px] leading-[1.65]" innerHTML={html()} onClick={clickFile} onKeyDown={clickFile} />
}

function markFiles(root: HTMLElement, base?: string) {
  const at = (ref: FileRef | null) => (ref && base !== undefined ? { ...ref, path: joinPath(base, ref.path) } : ref)
  for (const a of root.querySelectorAll<HTMLAnchorElement>('a[href]')) {
    const ref = at(fileInHref(a.getAttribute('href')!))
    if (ref) {
      a.dataset.file = ref.path
      if (ref.line) a.dataset.line = String(ref.line)
      a.title = `Open ${ref.path}`
    } else {
      a.target = '_blank'
      a.rel = 'noopener noreferrer'
    }
  }
  for (const code of root.querySelectorAll<HTMLElement>('code')) {
    if (code.closest('pre, a')) continue
    const ref = at(fileInCode(code.textContent ?? ''))
    if (!ref) continue
    code.dataset.file = ref.path
    if (ref.line) code.dataset.line = String(ref.line)
    code.classList.add('filelink')
    code.role = 'link'
    code.tabIndex = 0
    code.title = `Open ${ref.path}`
  }
}

function clickFile(e: MouseEvent | KeyboardEvent) {
  if (e instanceof KeyboardEvent && e.key !== 'Enter') return
  if (e instanceof MouseEvent && e.button !== 0) return
  const t = (e.target as HTMLElement).closest<HTMLElement>('[data-file]')
  const sel = window.getSelection()
  if (!t || (sel && !sel.isCollapsed && sel.containsNode(t, true))) return
  e.preventDefault()
  openFile({ path: t.dataset.file!, line: t.dataset.line ? Number(t.dataset.line) : undefined })
}

function Reasoning(props: { item: ItemView }) {
  const [open, setOpen] = createSignal(false)
  const text = () => props.item.live ?? (props.item.body as { text: string }).text
  const streaming = () => props.item.live !== undefined && !props.item.done
  return (
    <Show when={streaming() || text().trim()}>
      <div class="text-[13px] text-mute">
        <button class="flex items-center gap-1 hover:text-ink" aria-expanded={open()} onClick={() => setOpen(!open())}>
          <span classList={{ pulse: streaming() }}>{streaming() ? 'Thinking' : 'Thought'}</span>
          <ChevronRight size={13} class="transition-transform duration-100" classList={{ 'rotate-90': open() }} />
        </button>
        <Show when={open() || streaming()}>
          <div class="mt-1 whitespace-pre-wrap text-mute" classList={{ 'line-clamp-2': !open() }}>
            {text()}
          </div>
        </Show>
      </div>
    </Show>
  )
}

function Todos(props: { item: ItemView }) {
  const todos = () => (props.item.body as { todos: { content: string; status: string; active_form?: string }[] }).todos
  return (
    <div class="space-y-1 py-0.5">
      <For each={todos()}>
        {(t) => (
          <div class="flex items-start gap-2 text-[13px]">
            <span
              class="mt-[3px] shrink-0"
              classList={{
                'text-ok': t.status === 'completed',
                'text-accent': t.status === 'in_progress',
                'text-faint': t.status === 'pending',
              }}
            >
              {t.status === 'completed' ? <Check size={13} /> : t.status === 'in_progress' ? <CircleDot size={13} /> : <Circle size={13} />}
            </span>
            <span classList={{ 'text-mute line-through decoration-faint': t.status === 'completed', 'text-mute': t.status === 'pending' }}>
              {t.status === 'in_progress' && t.active_form ? t.active_form : t.content}
            </span>
          </div>
        )}
      </For>
    </div>
  )
}

function ToolRow(props: { thread?: string; item: ItemView; kids?: Map<string, ItemView[]>; cwd?: string }) {
  const [open, setOpen] = createSignal(false)
  const body = () => props.item.body as Extract<ItemView['body'], { type: 'tool_call' }>
  const summary = createMemo(() => toolSummary(body().name, body().input, props.item.liveInput, props.cwd))
  const status = () => body().status
  const kids = () => props.kids?.get(props.item.id) ?? []
  return (
    <div>
      <button
        class="flex w-full items-baseline gap-2 text-left text-[13px] leading-[1.5]"
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <span
          class="shrink-0 font-medium"
          classList={{ 'text-mute': status() !== 'running', 'text-ink': status() === 'running', pulse: status() === 'running' }}
        >
          {summary().verb}
        </span>
        <span
          class="min-w-0 truncate text-faint"
          classList={{ 'font-mono text-[12px]': !summary().plain, 'text-[13px]': !!summary().plain }}
        >
          {summary().detail}
        </span>
        <Show when={status() === 'failed'}>
          <span class="shrink-0 text-[11.5px] text-bad">failed</span>
        </Show>
        <Show when={status() === 'denied'}>
          <span class="shrink-0 text-[11.5px] text-warn">denied</span>
        </Show>
        <Show when={kids().length > 0}>
          <span class="shrink-0 text-[11.5px] text-faint">{kids().filter((k) => k.body.type === 'tool_call').length} steps</span>
        </Show>
      </button>
      <Show when={open()}>
        <div class="mt-1.5 mb-1">
          <ToolDetail name={body().name} input={body().input} output={body().output} />
          <Show when={kids().length > 0}>
            <div class="mt-2 space-y-2 border-l border-line pl-3">
              <For each={kids()}>{(c) => <ItemBlock thread={props.thread} item={c} kids={props.kids} cwd={props.cwd} />}</For>
            </div>
          </Show>
        </div>
      </Show>
    </div>
  )
}

function approvalTitle(tool: string): string {
  switch (tool) {
    case 'Bash':
      return 'Run this command?'
    case 'Write':
      return 'Write this file?'
    case 'Edit':
    case 'MultiEdit':
    case 'NotebookEdit':
      return 'Edit this file?'
    case 'WebFetch':
      return 'Fetch this page?'
    case 'Read':
    case 'acp:read':
      return 'Read this file?'
    case 'acp:execute':
      return 'Run this command?'
    case 'acp:edit':
      return 'Edit this file?'
    case 'acp:delete':
      return 'Delete this?'
    case 'acp:move':
      return 'Move this?'
    case 'acp:fetch':
      return 'Fetch this page?'
    default: {
      if (tool.startsWith('acp:')) return 'Allow this?'
      const m = tool.match(/^mcp__(.+?)__(.+)$/)
      return m ? `Use ${m[2].replace(/_/g, ' ')} from ${m[1]}?` : `Use ${tool}?`
    }
  }
}

function resolvedLabel(r?: RequestResponse) {
  if (!r) return ''
  if (r.type === 'allow') return r.always ? 'Allowed for this session' : 'Allowed'
  if (r.type === 'deny') return 'Denied'
  if (r.type === 'answers') return 'Answered'
  return 'Cancelled'
}

function RequestCard(props: { thread: string; r: RequestView; cwd?: string }) {
  const req = () => props.r.request
  const pending = () => !props.r.response
  const isSecret = () => req().type === 'hub_action' && (req() as { action: string }).action === 'secret.request'
  const isSignIn = () => req().type === 'hub_action' && (req() as { action: string }).action === 'agent.sign_in'
  const [error, setError] = createSignal('')
  const [sending, setSending] = createSignal(false)
  let stuck: number | undefined
  onCleanup(() => clearTimeout(stuck))
  const respond = (response: RequestResponse) => {
    if (sending()) return
    setSending(true)
    setError('')
    stuck = window.setTimeout(() => setSending(false), 15_000)
    void call({ op: 'respond', thread: props.thread, request: props.r.id, response }).catch((e) => {
      clearTimeout(stuck)
      setSending(false)
      setError((e as Error).message)
    })
  }
  let card: HTMLDivElement | undefined
  const byKey = () => pending() && (req().type === 'tool_approval' || (req().type === 'hub_action' && !isSecret() && !isSignIn()))
  createEffect(() => {
    if (!byKey()) return
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (el && (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT' || el.isContentEditable)) return
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat || sending()) return
      if (card !== document.querySelector('[data-approval]')) return
      if (e.key === 'Enter') {
        e.preventDefault()
        respond({ type: 'allow', always: false })
      } else if (e.key === 'Escape') {
        e.preventDefault()
        respond({ type: 'deny' })
      }
    }
    window.addEventListener('keydown', onKey)
    onCleanup(() => window.removeEventListener('keydown', onKey))
  })
  return (
    <Show
      when={pending()}
      fallback={
        <div class="text-[13px] text-mute">
          <span class="font-medium">
            {isSignIn()
              ? props.r.response?.type === 'allow'
                ? 'Sent a sign-in code'
                : 'Sign-in cancelled'
              : isSecret()
                ? secretLabel(props.r.response)
                : resolvedLabel(props.r.response)}
          </span>{' '}
          <span class="font-mono text-[12px] text-faint">
            {req().type === 'tool_approval'
              ? toolSummary((req() as { tool: string }).tool, (req() as unknown as { input: null }).input, undefined, props.cwd).detail
              : req().type === 'hub_action'
                ? isSignIn()
                  ? ''
                  : isSecret()
                    ? String((req() as { input: { name?: string } }).input?.name ?? '')
                    : (req() as { action: string }).action
                : req().type === 'plan_approval'
                  ? 'the plan'
                  : ''}
          </span>
        </div>
      }
    >
      <div ref={card} data-approval={byKey() ? '' : undefined} class="rounded-2xl border border-warn/50 bg-warn/[0.06] p-4">
        <Switch>
          <Match when={req().type === 'tool_approval' && req()}>
            {(r) => {
              const x = r() as Extract<RequestView['request'], { type: 'tool_approval' }>
              const s = toolSummary(x.tool, x.input, undefined, props.cwd)
              return (
                <>
                  <div class="text-[14px] font-medium">{approvalTitle(x.tool)}</div>
                  <Show
                    when={x.tool === 'Edit' || x.tool === 'MultiEdit' || x.tool === 'Write'}
                    fallback={
                      <div class="mt-2 max-h-40 overflow-auto rounded-lg bg-bg/70 px-3 py-2 font-mono text-[12px] break-all whitespace-pre-wrap">
                        {s.full ?? s.detail}
                      </div>
                    }
                  >
                    <div class="mt-1 mb-2 font-mono text-[12px] text-mute">{s.detail}</div>
                    <ToolDetail name={x.tool} input={x.input} />
                  </Show>
                  <Show when={x.reason}>
                    <div class="mt-1.5 text-[12.5px] text-mute">{x.reason}</div>
                  </Show>
                  <div class="mt-3 flex flex-wrap gap-2">
                    <Btn kind="primary" disabled={sending()} onClick={() => respond({ type: 'allow', always: false })}>
                      Allow once
                    </Btn>
                    <Btn disabled={sending()} onClick={() => respond({ type: 'allow', always: true })}>
                      Allow for this session
                    </Btn>
                    <Btn kind="ghost" disabled={sending()} onClick={() => respond({ type: 'deny' })}>
                      Deny
                    </Btn>
                    <span class="self-center text-[12px] text-faint max-md:hidden">Enter allows once, Esc denies</span>
                  </div>
                </>
              )
            }}
          </Match>
          <Match when={req().type === 'question' && req()}>
            {(r) => (
              <QuestionForm
                r={r() as Extract<RequestView['request'], { type: 'question' }>}
                disabled={sending()}
                onAnswer={(a) => respond({ type: 'answers', answers: a })}
              />
            )}
          </Match>
          <Match when={req().type === 'plan_approval' && req()}>
            {(r) => (
              <>
                <div class="mb-2 text-[12.5px] font-medium text-mute">Plan</div>
                <Markdown text={(r() as { plan: string }).plan} />
                <div class="mt-4 flex flex-wrap gap-2">
                  <Btn kind="primary" disabled={sending()} onClick={() => respond({ type: 'allow', always: false })}>
                    Approve and start
                  </Btn>
                  <Btn
                    kind="ghost"
                    disabled={sending()}
                    onClick={() => respond({ type: 'deny', message: 'The user wants to keep planning. Wait for their feedback.' })}
                  >
                    Keep planning
                  </Btn>
                </div>
              </>
            )}
          </Match>
          <Match when={isSignIn() && req()}>
            {(r) => {
              const x = r() as { input: { agent?: string; url?: string } }
              return (
                <SignInForm
                  agent={String(x.input?.agent ?? 'the agent')}
                  url={String(x.input?.url ?? '')}
                  onSend={(code) =>
                    call({ op: 'answer_secret', thread: props.thread, request: props.r.id, value: code }).catch((e) =>
                      setError((e as Error).message),
                    )
                  }
                />
              )
            }}
          </Match>
          <Match when={isSecret() && req()}>
            {(r) => {
              const x = r() as { input: { name?: string; why?: string } }
              return (
                <SecretForm
                  name={String(x.input?.name ?? '')}
                  why={String(x.input?.why ?? '')}
                  onSend={(value) =>
                    call({ op: 'answer_secret', thread: props.thread, request: props.r.id, value }).catch((e) =>
                      setError((e as Error).message),
                    )
                  }
                  disabled={sending()}
                  onDecline={() => respond({ type: 'deny' })}
                />
              )
            }}
          </Match>
          <Match when={req().type === 'hub_action' && req()}>
            {(r) => {
              const x = r() as Extract<RequestView['request'], { type: 'hub_action' }>
              return (
                <>
                  <div class="text-[14px] font-medium">Allow {x.action.replace(/\./g, ' ')}?</div>
                  <div class="mt-2 max-h-80 overflow-auto rounded-lg bg-bg/70 px-3 py-2 font-mono text-[12px]">
                    <For each={hubFields(x.input)}>
                      {([k, v]) => (
                        <div class="flex gap-3">
                          <span class="w-14 shrink-0 text-mute">{k}</span>
                          <span class="min-w-0 break-words whitespace-pre-wrap">
                            {Array.isArray(v) ? v.join('\n') : typeof v === 'object' ? JSON.stringify(v) : String(v)}
                          </span>
                        </div>
                      )}
                    </For>
                  </div>
                  <div class="mt-3 flex gap-2">
                    <Btn kind="primary" disabled={sending()} onClick={() => respond({ type: 'allow', always: false })}>
                      Allow
                    </Btn>
                    <Btn kind="ghost" disabled={sending()} onClick={() => respond({ type: 'deny' })}>
                      Deny
                    </Btn>
                  </div>
                </>
              )
            }}
          </Match>
        </Switch>
        <Show when={error()}>
          <div class="mt-2 text-[12.5px] text-bad">{error()}</div>
        </Show>
      </div>
    </Show>
  )
}

function secretLabel(r?: RequestResponse) {
  if (!r) return ''
  if (r.type === 'allow') return 'Gave the secret'
  if (r.type === 'deny') return 'Declined to give'
  return 'Cancelled'
}

function SignInForm(props: { agent: string; url: string; onSend: (code: string) => Promise<unknown> }) {
  const [code, setCode] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const send = async () => {
    if (!code().trim() || busy()) return
    setBusy(true)
    await props.onSend(code().trim())
    setBusy(false)
  }
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      <div class="text-[14px] font-medium">Sign in to {props.agent} on this machine</div>
      <p class="mt-1 text-[13px] text-mute">Sign in with Google, then paste the code the page shows you, or the address it lands on.</p>
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <a
          href={props.url}
          target="_blank"
          rel="noopener noreferrer"
          class="inline-flex h-8 items-center rounded-lg border border-line bg-raised px-3 text-[13px] font-medium hover:border-faint"
        >
          Open Google sign-in
        </a>
      </div>
      <input
        class={`${inputClass} mt-3 w-full font-mono`}
        autocomplete="off"
        spellcheck={false}
        placeholder="Paste the code here"
        value={code()}
        onInput={(e) => setCode(e.currentTarget.value)}
      />
      <p class="mt-2 text-[12.5px] text-faint">The code goes straight to this machine and isn't kept in the conversation.</p>
      <div class="mt-3 flex gap-2">
        <Btn kind="primary" type="submit" disabled={!code().trim() || busy()}>
          {busy() ? 'Signing in…' : 'Sign in'}
        </Btn>
      </div>
    </form>
  )
}

function SecretForm(props: {
  name: string
  why: string
  disabled?: boolean
  onSend: (value: string) => Promise<unknown>
  onDecline: () => void
}) {
  const [value, setValue] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const send = async () => {
    if (!value() || busy()) return
    setBusy(true)
    await props.onSend(value())
    setValue('')
    setBusy(false)
  }
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      <div class="text-[14px] font-medium">
        The agent asks for a secret: <span class="font-mono text-[13px]">{props.name}</span>
      </div>
      <p class="mt-1 text-[13px] break-words whitespace-pre-wrap text-mute">{props.why}</p>
      <input
        class={`${inputClass} mt-3 w-full font-mono`}
        type="password"
        autocomplete="off"
        spellcheck={false}
        placeholder="Paste it here"
        value={value()}
        onInput={(e) => setValue(e.currentTarget.value)}
      />
      <p class="mt-2 text-[12.5px] text-faint">
        It goes to a file on the thread's machine that only the agent reads, for a day. It isn't kept in the conversation.
      </p>
      <div class="mt-3 flex gap-2">
        <Btn kind="primary" type="submit" disabled={!value() || busy() || props.disabled}>
          Send
        </Btn>
        <Btn kind="ghost" disabled={busy() || props.disabled} onClick={props.onDecline}>
          Decline
        </Btn>
      </div>
    </form>
  )
}

function QuestionForm(props: {
  r: Extract<RequestView['request'], { type: 'question' }>
  disabled?: boolean
  onAnswer: (a: string[][]) => void
}) {
  const [picked, setPicked] = createSignal<string[][]>(props.r.questions.map(() => []))
  const [other, setOther] = createSignal<string[]>(props.r.questions.map(() => ''))
  const toggle = (qi: number, label: string, multi: boolean) =>
    setPicked((p) => p.map((x, i) => (i !== qi ? x : multi ? (x.includes(label) ? x.filter((l) => l !== label) : [...x, label]) : [label])))
  const answers = () => picked().map((p, i) => (other()[i].trim() ? [...p, other()[i].trim()] : p))
  const ready = () => answers().every((a) => a.length > 0)
  return (
    <div class="space-y-4">
      <For each={props.r.questions}>
        {(q, qi) => (
          <div>
            <div class="text-[14px] font-medium">{q.question}</div>
            <div class="mt-2 grid gap-1.5">
              <For each={q.options}>
                {(o) => {
                  const on = () => picked()[qi()].includes(o.label)
                  return (
                    <button
                      class="rounded-xl border bg-bg/60 px-3 py-2 text-left text-[13px] transition-colors duration-100"
                      classList={{ 'border-accent': on(), 'border-line hover:border-faint': !on() }}
                      aria-pressed={on()}
                      onClick={() => toggle(qi(), o.label, q.multi_select)}
                    >
                      <div class="font-medium">{o.label}</div>
                      <Show when={o.description}>
                        <div class="text-[12.5px] text-mute">{o.description}</div>
                      </Show>
                    </button>
                  )
                }}
              </For>
              <input
                class="h-9 rounded-xl border border-line bg-bg/60 px-3 text-[13px] placeholder:text-faint focus:border-faint"
                placeholder="Something else"
                value={other()[qi()]}
                onInput={(e) => setOther((o) => o.map((x, i) => (i === qi() ? e.currentTarget.value : x)))}
              />
            </div>
          </div>
        )}
      </For>
      <Btn kind="primary" disabled={!ready() || props.disabled} onClick={() => props.onAnswer(answers())}>
        Answer
      </Btn>
    </div>
  )
}
