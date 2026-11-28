import { For, Match, Show, Switch, createEffect, createMemo, createSignal, type JSX } from 'solid-js'
import ChevronRight from 'lucide-solid/icons/chevron-right'
import Check from 'lucide-solid/icons/check'
import RotateCcw from 'lucide-solid/icons/rotate-ccw'
import Circle from 'lucide-solid/icons/circle'
import CircleDot from 'lucide-solid/icons/circle-dot'
import Bell from 'lucide-solid/icons/bell'
import KeyRound from 'lucide-solid/icons/key-round'
import Copy from 'lucide-solid/icons/copy'
import GitFork from 'lucide-solid/icons/git-fork'
import Undo2 from 'lucide-solid/icons/undo-2'
import { call, restoreDraft, state } from '../lib/store'
import { pushToast } from '../lib/toast'
import { folds, isWork, type Entry, type ItemView, type RequestView, type TurnView } from '../lib/timeline'
import type { InputPart } from '../proto/InputPart'
import { clock, duration } from '../lib/format'
import { setPanel } from '../lib/panel'
import { Btn, Thumbs } from './bits'
import { ToolDetail, toolSummary } from './tools'
import { AgentRow, ThreadRow } from './Agents'
import { isAgentCall, startedThread } from '../lib/agents'
import { holds } from '../lib/find'
import { findTarget } from './Find'
import { Markdown } from './Markdown'
import { RequestCard } from './Requests'

export { Markdown }

const NOTE = '[corra] '
const isLimit = (text?: string) => !!text && /limit/i.test(text) && /reset/i.test(text)

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
  const sought = () => {
    const t = findTarget()
    return !!t && t.thread === props.thread && (t.item === `${props.turn.id}:input` || holds(props.turn.entries, t.item))
  }
  const limitedTurn = () => props.turn.resetsAt !== undefined || isLimit(props.turn.error)
  return (
    <section
      data-turn={props.turn.id}
      class="group/turn pb-7"
      style={{ 'content-visibility': props.last || sought() ? 'visible' : 'auto', 'contain-intrinsic-size': 'auto 220px' }}
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
                    <Fold thread={props.thread} label={foldLabel(i(), p.lead)} steps={stepsIn(p.work)} entries={p.work} render={render} />
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
            'border-bad/30 bg-bad/5 text-bad': !limitedTurn(),
            'border-warn/40 bg-warn/10 text-ink': limitedTurn(),
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

function Fold(props: { thread: string; label: string; steps: number; entries: Entry[]; render: (e: Entry) => JSX.Element }) {
  const [open, setOpen] = createSignal(false)
  const agents = createMemo(() => agentsIn(props.entries))
  createEffect(() => {
    const t = findTarget()
    if (t && t.thread === props.thread && holds(props.entries, t.item)) setOpen(true)
  })
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

function NotRead(props: { thread: string; input: InputPart[] }) {
  const [error, setError] = createSignal('')
  const [sent, setSent] = createSignal(false)
  const again = () => {
    const s = state.threads[props.thread]?.status
    setError('')
    call({
      op: 'send',
      thread: props.thread,
      input: props.input,
      queue: s === 'running' || s === 'waiting' || s === 'starting',
      client_id: crypto.randomUUID(),
    })
      .then(() => setSent(true))
      .catch((e) => setError((e as Error).message))
  }
  return (
    <div class="flex items-center justify-end gap-1 text-[12.5px] text-faint">
      <span>{error() || 'Not read'}</span>
      <button type="button" class={actionBtn} disabled={sent()} onClick={again}>
        <RotateCcw size={13} />
        {sent() ? 'Sent again' : 'Send again'}
      </button>
    </div>
  )
}

function HubNote(props: { text: string; find?: string }) {
  return (
    <div class="flex gap-2.5 rounded-xl bg-ink/[0.03] px-3.5 py-2.5 text-[13px] text-mute">
      <Bell size={14} class="mt-[3px] shrink-0 text-faint" />
      <div class="min-w-0 flex-1 [&_.md]:text-[13px]">
        <Markdown text={props.text} find={props.find} />
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
  const [confirmRewind, setConfirmRewind] = createSignal(false)
  const [rewinding, setRewinding] = createSignal(false)
  const long = () => {
    const t = text()?.text ?? ''
    return t.length > 1400 || t.split('\n').length > 16
  }
  const note = () => {
    const t = text()?.text
    return props.turn.input.length === 1 && t?.startsWith(NOTE) ? t.slice(NOTE.length) : null
  }
  const doRewind = async () => {
    setRewinding(true)
    try {
      await call({ op: 'rewind_thread', thread: props.thread, turn: props.turn.id })
      const t = text()?.text ?? ''
      if (t) restoreDraft(props.thread, [t], true)
      setConfirmRewind(false)
    } catch (e) {
      pushToast((e as Error).message)
    } finally {
      setRewinding(false)
    }
  }
  return (
    <Show
      when={note() === null}
      fallback={
        <div class="mb-4">
          <HubNote text={note()!} find={`${props.turn.id}:input`} />
        </div>
      }
    >
      <div class="mb-2 flex flex-col items-end gap-1">
        <Thumbs parts={props.turn.input} class="mb-1" />
        <Show when={texts().length}>
          <div class="max-w-[85%] rounded-2xl rounded-br-md bg-panel px-4 py-2.5 text-[14px] leading-relaxed whitespace-pre-wrap">
            <div
              ref={(el) => el.addEventListener('unclamp', () => setWhole(true))}
              data-find={`${props.turn.id}:input`}
              data-clamp={long() && !whole() ? '' : undefined}
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
        <Show when={props.turn.unread}>
          <NotRead thread={props.thread} input={props.turn.input} />
        </Show>
        <Show
          when={!confirmRewind()}
          fallback={
            <div class="mt-1 flex flex-wrap items-center justify-end gap-2 rounded-xl border border-warn/40 bg-warn/[0.06] px-3 py-1.5 text-[12.5px]">
              <span class="text-mute">Rewind thread and undo files after this?</span>
              <Btn small kind="danger" disabled={rewinding()} onClick={() => void doRewind()}>
                {rewinding() ? 'Rewinding…' : 'Rewind'}
              </Btn>
              <Btn small kind="ghost" disabled={rewinding()} onClick={() => setConfirmRewind(false)}>
                Cancel
              </Btn>
            </div>
          }
        >
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
                onClick={() => setConfirmRewind(true)}
              >
                <Undo2 size={13} />
                Rewind
              </button>
            </Show>
          </div>
        </Show>
      </div>
    </Show>
  )
}

async function forkAt(thread: string, turn: string) {
  try {
    const t = await call<{ id: string }>({ op: 'fork_thread', thread, turn })
    location.hash = `#/t/${t.id}`
  } catch (e) {
    pushToast((e as Error).message)
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
    </Switch>
  )
}

function ItemBlock(props: { thread?: string; item: ItemView; kids?: Map<string, ItemView[]>; cwd?: string }) {
  const b = () => props.item.body
  const found = () => (!props.item.parent ? props.item.id : undefined)
  return (
    <Switch>
      <Match when={b().type === 'agent_message'}>
        <div>
          <Markdown
            text={props.item.live ?? (b() as { text: string }).text}
            find={found()}
            streaming={!props.item.done && props.item.live !== undefined}
          />
          <Show when={props.item.done && ((b() as { text: string }).text?.length ?? 0) > 200}>
            <div class={`${actionRow} mt-1 -ml-2`}>
              <CopyButton text={() => (b() as { text: string }).text} label="Copy reply" />
            </div>
          </Show>
        </div>
      </Match>
      <Match when={b().type === 'task_result'}>
        <div class="text-[13px] text-mute">{(b() as { text: string }).text}</div>
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
        <div class="flex flex-col items-end gap-1">
          <div data-find={found()} class="max-w-[85%] rounded-2xl rounded-br-md bg-panel px-4 py-2.5 text-[14px] whitespace-pre-wrap">
            <For each={(b() as { content: InputPart[] }).content}>{(p) => (p.type === 'text' ? <span>{p.text}</span> : null)}</For>
          </div>
          <Show when={props.item.unread && props.thread}>
            {(t) => <NotRead thread={t()} input={(b() as { content: InputPart[] }).content} />}
          </Show>
        </div>
      </Match>
    </Switch>
  )
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
