import { For, Show, createEffect, createMemo, createSignal, on, onCleanup } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import FileDiff from 'lucide-solid/icons/file-diff'
import FolderTree from 'lucide-solid/icons/folder-tree'
import SquareTerminal from 'lucide-solid/icons/square-terminal'
import Ellipsis from 'lucide-solid/icons/ellipsis'
import Laptop from 'lucide-solid/icons/laptop'
import Cloud from 'lucide-solid/icons/cloud'
import { call, closeThread, loadEarlier, openThread, state } from '../lib/store'
import { panel, setPanel } from '../lib/panel'
import { tokens } from '../lib/format'
import { Composer, UsageRing } from './Composer'
import ArrowDown from 'lucide-solid/icons/arrow-down'
import { EntryView, TurnBlock, setDraftFor } from './Timeline'
import type { Queued } from '../proto/Queued'
import type { InputPart } from '../proto/InputPart'
import type { TurnView } from '../lib/timeline'
import { IconBtn, Menu, StatusMark, statusWord, type MenuItem } from './bits'
import { SidePanel } from './Panel'
import { RunningTasks } from './Agents'
import { threadActions } from './actions'
import { refreshChanges, totals as changeTotals } from '../lib/changes'
import { limitedNow } from '../lib/seen'
import { paired } from '../lib/catalog'

function repeatsError(turns: TurnView[], i: number) {
  const t = turns[i]
  const before = turns[i - 1]
  if (!before || !t.error || t.error !== before.error || t.input.length) return false
  return t.entries.every((e) => (e.type === 'notice' && e.text === t.error) || (e.type === 'item' && !!e.item.parent))
}

function Outgoing(props: { parts: InputPart[]; note: string; queued?: boolean; actions?: { label: string; run: () => void }[] }) {
  return (
    <div class="flex flex-col items-end gap-1 pb-4">
      <div
        class="max-w-[85%] rounded-2xl rounded-br-md px-4 py-2.5 text-[14px] leading-relaxed whitespace-pre-wrap"
        classList={{ 'border border-dashed border-faint/70 text-mute': !!props.queued, 'bg-panel opacity-60': !props.queued }}
      >
        <For each={props.parts}>
          {(x) =>
            x.type === 'text' ? (
              <span>{x.text}</span>
            ) : (
              <img class="my-1 max-h-40 rounded-lg" src={`data:${x.media_type};base64,${x.data}`} />
            )
          }
        </For>
      </div>
      <div class="flex items-center gap-2 text-[12px] text-faint">
        <span>{props.note}</span>
        <For each={props.actions ?? []}>
          {(a) => (
            <button class="rounded-md px-1 text-mute hover:bg-ink/5 hover:text-ink max-md:py-1.5" onClick={a.run}>
              {a.label}
            </button>
          )}
        </For>
      </div>
    </div>
  )
}

const [clock, setClock] = createSignal(Date.now())
setInterval(() => setClock(Date.now()), 5000)

export function ThreadPane(props: { id: string }) {
  void openThread(props.id)
  onCleanup(() => closeThread(props.id))
  const thread = () => state.threads[props.id]
  const view = () => state.views[props.id]
  const machine = () => (thread()?.machine ? state.machines[thread()!.machine!] : undefined)
  const project = () => (thread() ? state.projects[thread()!.project] : undefined)
  const here = () => !!thread()?.machine && thread()!.machine === state.thisMachine
  const where = () => (here() ? 'This computer' : (machine()?.name ?? 'No machine yet'))
  const cwd = () => {
    const c = thread()?.cwd ?? ''
    const home = machine()?.info?.home
    return home && c.startsWith('~/') ? home + c.slice(1) : c
  }

  let scroller!: HTMLDivElement
  const [stick, setStick] = createSignal(true)
  let byHand = 0
  const touched = () => (byHand = Date.now())
  const fromBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight
  const toBottom = () => {
    if (!scroller) return
    scroller.scrollTop = scroller.scrollHeight
    requestAnimationFrame(() => scroller && stick() && (scroller.scrollTop = scroller.scrollHeight))
  }
  const pin = (content: HTMLDivElement) => {
    const ro = new ResizeObserver(() => {
      if (scroller && stick()) scroller.scrollTop = scroller.scrollHeight
    })
    queueMicrotask(() => {
      ro.observe(content)
      if (scroller) ro.observe(scroller)
    })
    onCleanup(() => ro.disconnect())
  }
  const [fetching, setFetching] = createSignal(false)
  const earlier = () => (state.earlier[props.id] ?? null) !== null
  const nearTop = () => !!scroller && scroller.scrollTop < 800 && (!stick() || scroller.scrollHeight <= scroller.clientHeight + 1)
  const more = async () => {
    if (!earlier() || fetching()) return
    setFetching(true)
    const below = scroller.scrollHeight - scroller.scrollTop
    try {
      await loadEarlier(props.id)
    } finally {
      setFetching(false)
      requestAnimationFrame(() => {
        if (!scroller) return
        if (!stick()) scroller.scrollTop = scroller.scrollHeight - below
        if (nearTop()) void more()
      })
    }
  }
  const onScroll = () => {
    if (fromBottom() < 8) setStick(true)
    else if (Date.now() - byHand < 400) setStick(false)
    if (nearTop()) void more()
  }
  createEffect(
    on(
      () => [props.id, earlier(), view()?.turns.length] as const,
      () => requestAnimationFrame(() => nearTop() && void more()),
    ),
  )
  createEffect(
    on(
      () => props.id,
      () => {
        setStick(true)
        requestAnimationFrame(toBottom)
      },
    ),
  )
  const contentKey = createMemo(() => {
    const v = view()
    if (!v) return 0
    const t = v.turns.at(-1)
    const e = t?.entries.at(-1)
    const live = e?.type === 'item' ? (e.item.live?.length ?? 0) + (e.item.liveInput?.length ?? 0) : 0
    return (
      v.lastSeq * 1e6 +
      (t?.entries.length ?? 0) * 1e3 +
      live +
      (state.pending[props.id]?.length ?? 0) +
      (state.queues[props.id]?.length ?? 0)
    )
  })
  createEffect(
    on(contentKey, () => {
      if (stick()) requestAnimationFrame(toBottom)
    }),
  )

  const context = () => {
    const c = state.live[props.id]?.context
    if (!c?.used_tokens) return undefined
    return `${tokens(c.used_tokens)} tokens`
  }
  const contextHint = () => {
    const c = state.live[props.id]?.context
    if (!c?.used_tokens) return undefined
    return c.window_tokens
      ? `${c.used_tokens.toLocaleString()} of ${c.window_tokens.toLocaleString()} tokens of context (${Math.round((c.used_tokens / c.window_tokens) * 100)}%)`
      : `${c.used_tokens.toLocaleString()} tokens of context`
  }
  const totals = () => {
    const t = changeTotals(props.id)
    return { n: t.files, a: t.added, r: t.removed }
  }
  createEffect(
    on(
      () => [props.id, view()?.turns.filter((t) => t.outcome).length ?? 0] as const,
      ([id]) => void refreshChanges(id),
    ),
  )

  const [error, setError] = createSignal('')
  const run = (p: Promise<unknown>) => void p.catch((e) => setError((e as Error).message))
  const editQueued = (q: Queued) => {
    const t = q.input.find((p) => p.type === 'text')
    run(
      call({ op: 'unqueue', thread: props.id, id: q.id, send: false }).then(() =>
        setDraftFor({ thread: props.id, text: t && t.type === 'text' ? t.text : '' }),
      ),
    )
  }
  const menu = createMemo<MenuItem[]>(() => (thread() ? threadActions(thread()!, setError) : []))

  const background = () => {
    const at = state.live[props.id]?.background ?? 0
    return at > 0 && clock() - at < 45_000
  }
  return (
    <Show when={thread()} fallback={<Missing id={props.id} />}>
      <div class="relative flex min-h-0 flex-1">
        <div class="flex min-w-0 flex-1 flex-col" classList={{ 'max-md:hidden': !!panel() }}>
          <header class="flex h-[52px] shrink-0 items-center gap-2 border-b border-line px-3 sm:px-5">
            <a
              href="#/"
              aria-label="Back to threads"
              class="-ml-1 flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 md:hidden"
            >
              <ArrowLeft size={18} />
            </a>
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <StatusMark status={thread()!.status} />
                <h1 class="truncate text-[14px] font-semibold">{thread()!.title}</h1>
              </div>
              <div class="flex items-center gap-1.5 truncate pl-[22px] text-[12px] text-mute">
                <span>{project()?.name}</span>
                <span class="text-faint">·</span>
                <span class="max-sm:hidden">{thread()!.agent === 'antigravity' ? 'Antigravity' : 'Claude Code'}</span>
                <span class="text-faint max-sm:hidden">·</span>
                <span class="inline-flex items-center gap-1">
                  {here() || (machine() && paired(machine()!.provider)) ? <Laptop size={12} /> : <Cloud size={12} />}
                  {where()}
                </span>
                <Show when={statusWord[thread()!.status]}>
                  <span class="text-faint">·</span>
                  <span classList={{ 'text-warn': thread()!.status === 'waiting', 'text-bad': thread()!.status === 'error' }}>
                    {thread()!.status === 'error' && limitedNow(state.usage) ? 'Limited' : statusWord[thread()!.status]}
                  </span>
                </Show>
                <Show when={context()}>
                  <span class="text-faint max-sm:hidden">·</span>
                  <span class="num max-sm:hidden" title={contextHint()}>
                    {context()}
                  </span>
                </Show>
              </div>
            </div>
            <UsageRing />
            <button
              class="flex h-8 items-center gap-1.5 rounded-lg px-2 text-[12.5px] text-mute hover:bg-ink/5 hover:text-ink"
              classList={{ 'bg-ink/5 text-ink': panel()?.kind === 'diff' }}
              title={totals().n ? `${totals().n} uncommitted ${totals().n === 1 ? 'file' : 'files'}` : 'Changes'}
              onClick={() => setPanel(panel()?.kind === 'diff' ? null : { kind: 'diff' })}
            >
              <FileDiff size={16} />
              <Show when={totals().n > 0}>
                <span class="num font-mono text-[11.5px] text-ok">+{totals().a}</span>
                <span class="num font-mono text-[11.5px] text-bad">−{totals().r}</span>
              </Show>
            </button>
            <IconBtn
              label="Files"
              active={panel()?.kind === 'files'}
              onClick={() => setPanel(panel()?.kind === 'files' ? null : { kind: 'files' })}
            >
              <FolderTree size={16} />
            </IconBtn>
            <IconBtn
              label="Terminal"
              class="max-sm:hidden"
              active={panel()?.kind === 'terminal'}
              onClick={() => setPanel(panel()?.kind === 'terminal' ? null : { kind: 'terminal' })}
            >
              <SquareTerminal size={16} />
            </IconBtn>
            <Menu
              label="More"
              align="end"
              items={menu()}
              class="inline-flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 hover:text-ink md:hidden"
            >
              <Ellipsis size={16} />
            </Menu>
          </header>
          <Show when={error()}>
            <button class="border-b border-line bg-bad/5 px-5 py-1.5 text-left text-[12.5px] text-bad" onClick={() => setError('')}>
              {error()}
            </button>
          </Show>
          <div
            ref={scroller}
            onScroll={onScroll}
            onWheel={touched}
            onTouchMove={touched}
            onPointerDown={touched}
            onKeyDown={touched}
            class="min-h-0 flex-1 overflow-y-auto"
          >
            <div ref={pin} class="mx-auto max-w-3xl px-4 pt-7 pb-2 sm:px-6">
              <Show when={earlier()}>
                <div class="pb-7 text-center">
                  <button
                    class="text-[12.5px] text-mute hover:text-ink disabled:pointer-events-none"
                    disabled={fetching()}
                    onClick={() => void more()}
                  >
                    {fetching() ? 'Loading earlier turns…' : 'Show earlier turns'}
                  </button>
                </div>
              </Show>
              <For each={view()?.preface ?? []}>
                {(e) => (
                  <div class="pb-4">
                    <EntryView thread={props.id} entry={e} />
                  </div>
                )}
              </For>
              <For each={view()?.turns ?? []}>
                {(t, i) => (
                  <>
                    <Show when={!repeatsError(view()!.turns, i())}>
                      <TurnBlock thread={props.id} turn={t} last={i() === (view()?.turns.length ?? 0) - 1} cwd={cwd()} />
                    </Show>
                    <For each={t.after}>
                      {(e) => (
                        <div class="pb-4">
                          <EntryView thread={props.id} entry={e} />
                        </div>
                      )}
                    </For>
                  </>
                )}
              </For>
              <For each={(state.pending[props.id] ?? []).filter((p) => !p.queued)}>
                {(p) => <Outgoing parts={p.parts} note="Sending" />}
              </For>
              <For each={state.queues[props.id] ?? []}>
                {(q) => (
                  <Outgoing
                    parts={q.input}
                    queued
                    note="Queued for when the agent is done"
                    actions={[
                      { label: 'Send now', run: () => run(call({ op: 'unqueue', thread: props.id, id: q.id, send: true })) },
                      { label: 'Edit', run: () => editQueued(q) },
                      { label: 'Remove', run: () => run(call({ op: 'unqueue', thread: props.id, id: q.id, send: false })) },
                    ]}
                  />
                )}
              </For>
              <For each={(state.pending[props.id] ?? []).filter((p) => p.queued)}>
                {(p) => <Outgoing parts={p.parts} queued note="Queued for when the agent is done" />}
              </For>
              <Show when={thread()!.status === 'starting'}>
                <div class="flex items-center gap-2 pb-4 text-[13px] text-mute">
                  <span class="size-2 rounded-full bg-accent pulse" />
                  {here()
                    ? 'Starting'
                    : machine()?.status === 'connected'
                      ? 'Starting the agent'
                      : machine()
                        ? 'Waking the machine'
                        : 'Starting a machine'}
                </div>
              </Show>
              <Show
                when={state.live[props.id]?.agents !== undefined}
                fallback={
                  <Show when={background()}>
                    <div class="flex items-center gap-2 pb-4 text-[13px] text-mute">
                      <span class="size-2 rounded-full bg-faint pulse" />A background agent is still working
                    </div>
                  </Show>
                }
              >
                <RunningTasks thread={props.id} cwd={cwd()} />
              </Show>
            </div>
          </div>
          <Show when={!stick()}>
            <div class="pointer-events-none relative h-0">
              <button
                class="pointer-events-auto absolute -top-11 left-1/2 flex h-8 -translate-x-1/2 items-center gap-1.5 rounded-full border border-line bg-raised px-3 text-[12.5px] text-mute shadow-[var(--shadow)] hover:text-ink"
                onClick={() => {
                  setStick(true)
                  toBottom()
                }}
              >
                <ArrowDown size={13} />
                Jump to latest
              </button>
            </div>
          </Show>
          <Composer thread={thread()!} />
        </div>
        <Show when={panel()}>
          <SidePanel thread={props.id} />
        </Show>
      </div>
    </Show>
  )
}

function Missing(props: { id: string }) {
  return (
    <div class="flex flex-1 items-center justify-center p-8 text-[13px] text-mute">
      <Show when={state.loaded.hub} fallback="Loading">
        <div class="text-center">
          <div>This thread isn't here.</div>
          <a class="mt-2 inline-block text-ink underline decoration-line underline-offset-4" href="#/">
            Start a new one
          </a>
          <span class="hidden">{props.id}</span>
        </div>
      </Show>
    </div>
  )
}
