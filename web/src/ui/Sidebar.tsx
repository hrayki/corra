import { For, Show, createEffect, createMemo, createSignal, type JSX } from 'solid-js'
import Plus from 'lucide-solid/icons/plus'
import SquarePen from 'lucide-solid/icons/square-pen'
import PanelLeftClose from 'lucide-solid/icons/panel-left-close'
import Settings from 'lucide-solid/icons/settings'
import Search from 'lucide-solid/icons/search'
import ChevronRight from 'lucide-solid/icons/chevron-right'
import ChevronUp from 'lucide-solid/icons/chevron-up'
import Pin from 'lucide-solid/icons/pin'
import FolderPlus from 'lucide-solid/icons/folder-plus'
import SlidersHorizontal from 'lucide-solid/icons/sliders-horizontal'
import BellRing from 'lucide-solid/icons/bell-ring'
import { state } from '../lib/store'
import { pushToast, setPaletteOpen } from '../lib/toast'
import { attentionThreads, jumpToNextAttention } from './CommandPalette'
import type { Thread } from '../proto/Thread'
import { StatusMark, openContextMenu, statusWord } from './bits'
import { threadActions } from './actions'
import { ago } from '../lib/format'
import { route } from '../lib/route'
import { knowThreads, markSeen, unread } from '../lib/seen'
import { limitedNow } from '../lib/usage'
import { toggleSidebar } from '../lib/layout'
import { AGENT_NAMES } from '../lib/choices'
import { mostUrgent, needsYou, shownStatus, type Shown } from '../lib/status'

const FOLDED = 'corra:sidebar-folded'
const SHOWN = 5

const headBtn = 'flex size-8 shrink-0 items-center justify-center rounded-lg text-mute hover:bg-ink/[0.06] hover:text-ink max-md:size-9'

export function Sidebar() {
  const [q, setQ] = createSignal('')
  const [settledOpen, setSettledOpen] = createSignal(false)
  const [more, setMore] = createSignal<Record<string, boolean>>({})
  const [folded, setFoldedRaw] = createSignal<Record<string, boolean>>(JSON.parse(localStorage.getItem(FOLDED) ?? '{}'))
  const setFolded = (f: (old: Record<string, boolean>) => Record<string, boolean>) => {
    const next = f(folded())
    setFoldedRaw(next)
    localStorage.setItem(FOLDED, JSON.stringify(next))
  }
  const current = () => {
    const r = route()
    return r.kind === 'thread' ? r.id : undefined
  }
  const threads = createMemo(() => {
    const needle = q().trim().toLowerCase()
    return Object.values(state.threads).filter((t) => !t.hidden_at && (!needle || t.title.toLowerCase().includes(needle)))
  })
  const projectName = (t: Thread) => state.projects[t.project]?.name ?? 'Other'
  createEffect(() => knowThreads(Object.values(state.threads)))
  createEffect(() => {
    const t = current() ? state.threads[current()!] : undefined
    if (t && t.status === 'idle') markSeen(t.id, t.last_seq)
  })
  const shown = (t: Thread) => shownStatus(t.status, unread(t), t.id === current())
  const pinned = createMemo(() =>
    threads()
      .filter((t) => t.pinned_at)
      .sort((a, b) => (b.pinned_at ?? 0) - (a.pinned_at ?? 0)),
  )
  const groups = createMemo(() => {
    const names = new Set<string>(Object.values(state.projects).map((p) => p.name))
    for (const t of threads()) names.add(projectName(t))
    const rest = threads().filter((t) => !t.pinned_at)
    return [...names]
      .map((name) => {
        const ts = rest.filter((t) => projectName(t) === name).sort((a, b) => b.updated_at - a.updated_at)
        const all = threads().filter((t) => projectName(t) === name)
        const open = ts.filter((t) => !t.settled_at)
        const here = new Set(open.map((t) => t.id))
        const kids = new Map<string, Thread[]>()
        for (const t of open)
          if (t.started_by && here.has(t.started_by) && t.started_by !== t.id)
            kids.set(t.started_by, [...(kids.get(t.started_by) ?? []), t])
        const latest = (t: Thread, seen = new Set<string>()): number =>
          seen.has(t.id) ? t.updated_at : Math.max(t.updated_at, ...(kids.get(t.id) ?? []).map((k) => latest(k, new Set([...seen, t.id]))))
        const keep = (t: Thread, seen = new Set<string>()): boolean =>
          !seen.has(t.id) &&
          (needsYou(shown(t)) || t.id === current() || (kids.get(t.id) ?? []).some((k) => keep(k, new Set([...seen, t.id]))))
        return {
          name,
          kids,
          keep,
          flag: mostUrgent(open.map(shown)),
          open: open.filter((t) => !t.started_by || !here.has(t.started_by)).sort((a, b) => latest(b) - latest(a)),
          settled: ts.filter((t) => t.settled_at),
          latest: all.reduce((m, t) => Math.max(m, t.updated_at), 0),
          any: all.length > 0,
        }
      })
      .filter((g) => !q().trim() || g.open.length > 0)
      .sort((a, b) => b.latest - a.latest || a.name.localeCompare(b.name))
  })
  const settled = createMemo(() =>
    threads()
      .filter((t) => t.settled_at && !t.pinned_at)
      .sort((a, b) => b.updated_at - a.updated_at),
  )
  const hubProject = (name: string) => Object.values(state.projects).find((p) => p.name === name)
  const empty = () => state.loaded && Object.keys(state.projects).length === 0 && threads().length === 0
  const urgentCount = createMemo(() => attentionThreads().length)
  return (
    <aside class="flex h-full w-full flex-col bg-panel">
      <div class="flex h-[52px] shrink-0 items-center gap-1 px-2.5">
        <a href="#/" aria-label="Corra" class="flex h-8 items-center rounded-lg px-1.5">
          <img src="/favicon.svg" alt="" class="size-[22px]" />
        </a>
        <div class="flex-1" />
        <Show when={urgentCount() > 0}>
          <button
            type="button"
            aria-label={`Next thread needing you (${urgentCount()})`}
            title={`Next thread needing you (${urgentCount()}) · Alt+N`}
            onClick={() => jumpToNextAttention()}
            class="inline-flex h-8 items-center gap-1 rounded-lg bg-warn/15 px-2 text-[12px] font-medium text-warn hover:bg-warn/25"
          >
            <BellRing size={13} />
            <span class="num">{urgentCount()}</span>
          </button>
        </Show>
        <a href="#/new-project" aria-label="New project" title="New project" class={headBtn}>
          <FolderPlus size={16} />
        </a>
        <a href="#/new" aria-label="New thread" title="New thread" class={headBtn}>
          <SquarePen size={16} />
        </a>
      </div>
      <div class="px-3 pb-2">
        <div class="flex h-8 items-center gap-2 rounded-lg bg-ink/[0.04] px-2.5 text-mute focus-within:bg-ink/[0.07]">
          <Search size={13} />
          <input
            aria-label="Find a thread"
            class="min-w-0 flex-1 bg-transparent text-[13px] text-ink placeholder:text-faint"
            placeholder="Find a thread"
            value={q()}
            onInput={(e) => setQ(e.currentTarget.value)}
          />
          <button
            type="button"
            aria-label="Open quick switcher"
            title="Quick switcher (Ctrl+K)"
            onClick={() => setPaletteOpen(true)}
            class="shrink-0 rounded border border-line/80 bg-raised/70 px-1.5 py-px font-mono text-[10.5px] text-faint hover:border-faint hover:text-ink max-md:hidden"
          >
            ⌘K
          </button>
        </div>
      </div>
      <nav class="min-h-0 overflow-y-auto px-2 pt-1 pb-3" classList={{ 'flex-1': !settledOpen(), 'shrink grow-0': settledOpen() }}>
        <Show when={pinned().length}>
          <Group title="Pinned" icon={<Pin size={13} />}>
            <For each={pinned()}>{(t) => <Card t={t} active={current() === t.id} project={projectName(t)} />}</For>
          </Group>
        </Show>
        <For each={groups()}>
          {(g) => {
            const open = () => !folded()[g.name] || g.open.some((t) => t.id === current())
            const listed = () => (more()[g.name] ? g.open : g.open.filter((t, i) => i < SHOWN || g.keep(t)))
            const hidden = () => g.open.length - listed().length
            return (
              <Group
                title={g.name}
                folded={!open()}
                flag={open() ? undefined : g.flag}
                onFold={() => setFolded((f) => ({ ...f, [g.name]: !f[g.name] }))}
                newHref={`#/new/${encodeURIComponent(g.name)}`}
                settingsHref={hubProject(g.name) ? `#/project/${hubProject(g.name)!.id}` : undefined}
                trusted={!!hubProject(g.name)?.trusted}
              >
                <Show when={open()}>
                  <For
                    each={listed()}
                    fallback={
                      <Show when={!g.any}>
                        <div class="flex h-[30px] items-center pl-2.5 text-[12.5px] text-faint">No threads yet</div>
                      </Show>
                    }
                  >
                    {(t) => <Tree t={t} kids={g.kids} current={current()} depth={0} />}
                  </For>
                  <Show when={hidden() > 0 || more()[g.name]}>
                    <button
                      class="flex h-[30px] w-full items-center rounded-lg pl-2.5 text-[12.5px] text-faint hover:text-mute max-md:h-10"
                      onClick={() => setMore((m) => ({ ...m, [g.name]: !m[g.name] }))}
                    >
                      {more()[g.name] ? 'Show fewer' : `Show ${hidden()} more`}
                    </button>
                  </Show>
                </Show>
              </Group>
            )
          }}
        </For>
        <Show when={empty()}>
          <p class="px-2 py-6 text-[13px] text-mute">Nothing here yet. Start a thread and it shows up in this list.</p>
        </Show>
      </nav>
      <Show when={settledOpen()}>
        <div class="flex min-h-40 flex-1 flex-col justify-end overflow-hidden border-t border-line">
          <div class="min-h-0 overflow-y-auto px-2 py-2">
            <For each={settled()} fallback={<p class="px-2 py-1.5 text-[12.5px] text-faint">Nothing settled.</p>}>
              {(t) => <Card t={t} active={current() === t.id} project={projectName(t)} />}
            </For>
          </div>
        </div>
      </Show>
      <div class="flex h-[52px] shrink-0 items-center gap-1 border-t border-line px-2.5">
        <button
          class={`${headBtn} max-md:hidden`}
          aria-label="Hide the sidebar"
          title="Hide the sidebar (Ctrl+B)"
          onClick={() => toggleSidebar(true)}
        >
          <PanelLeftClose size={16} />
        </button>
        <div class="flex flex-1 justify-center">
          <Show when={settled().length}>
            <button
              class="flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[12.5px] text-mute hover:bg-ink/[0.06] hover:text-ink"
              classList={{ 'bg-ink/[0.06] text-ink': settledOpen() }}
              aria-expanded={settledOpen()}
              onClick={() => setSettledOpen(!settledOpen())}
            >
              Settled <span class="num text-faint">{settled().length}</span>
              <ChevronUp size={13} class="transition-transform duration-100" classList={{ 'rotate-180': settledOpen() }} />
            </button>
          </Show>
        </div>
        <HubBadge />
        <a
          href="#/settings"
          aria-label="Settings"
          title="Settings"
          class={headBtn}
          classList={{ 'bg-ink/[0.06] text-ink': route().kind === 'settings' }}
        >
          <Settings size={16} />
        </a>
      </div>
    </aside>
  )
}

function HubBadge() {
  const up = () => state.conn === 'open'
  return (
    <Show when={!up()}>
      <a href="#/settings" class="flex h-8 items-center gap-1.5 rounded-lg px-2 text-[12px] text-faint hover:text-mute">
        <span class="size-1.5 rounded-full bg-warn" />
        Reconnecting
      </a>
    </Show>
  )
}

const hoverBtn =
  'flex size-6 items-center justify-center rounded-md text-faint opacity-0 group-hover:opacity-100 hover:bg-ink/5 hover:text-ink focus-visible:opacity-100 max-md:opacity-100'

function Group(props: {
  title: string
  icon?: JSX.Element
  folded?: boolean
  onFold?: () => void
  newHref?: string
  settingsHref?: string
  trusted?: boolean
  flag?: Shown
  children: JSX.Element
}) {
  return (
    <section class="mt-1 rounded-xl py-0.5 transition-colors duration-100 first:mt-0 hover:bg-ink/[0.035]">
      <div class="group flex h-[30px] items-center gap-2 rounded-lg pr-1 pl-2">
        <button
          class="flex min-w-0 flex-1 items-center gap-2 text-left"
          aria-expanded={props.onFold ? !props.folded : undefined}
          disabled={!props.onFold}
          onClick={() => props.onFold?.()}
        >
          <Show when={props.icon}>
            <span class="flex size-4 shrink-0 items-center justify-center text-faint">{props.icon}</span>
          </Show>
          <span class="min-w-0 truncate text-[12.5px] font-medium text-mute">{props.title}</span>
          <Show when={props.trusted}>
            <span class="shrink-0 text-[11px] text-warn" title="Manager: the hub's own keys, every repo, and the tools that manage corra">
              manager
            </span>
          </Show>
          <Show when={props.flag && FLAG[props.flag]}>
            {(label) => (
              <span class="flex shrink-0" title={label()}>
                <StatusMark status={props.flag!} class="-mx-[3px]" />
                <span class="sr-only">{label()}</span>
              </span>
            )}
          </Show>
          <Show when={props.onFold}>
            <ChevronRight
              size={12}
              class="shrink-0 text-faint transition-transform duration-100"
              classList={{ 'rotate-90': !props.folded, 'opacity-0 group-hover:opacity-100': !props.folded }}
            />
          </Show>
        </button>
        <Show when={props.settingsHref}>
          <a href={props.settingsHref} aria-label={`${props.title}'s settings`} title={`${props.title}'s settings`} class={hoverBtn}>
            <SlidersHorizontal size={12} />
          </a>
        </Show>
        <Show when={props.newHref}>
          <a href={props.newHref} aria-label={`New thread in ${props.title}`} title={`New thread in ${props.title}`} class={hoverBtn}>
            <Plus size={13} />
          </a>
        </Show>
      </div>
      {props.children}
    </section>
  )
}

const TONE: Record<Shown, string> = {
  waiting: 'text-warn',
  error: 'text-bad',
  running: 'text-accent',
  starting: 'text-accent',
  done: 'text-done',
  idle: '',
}

const FLAG: Partial<Record<Shown, string>> = {
  waiting: 'A thread here needs you',
  error: 'A thread here failed',
  done: 'A thread here is done',
}

function Tree(props: { t: Thread; kids: Map<string, Thread[]>; current?: string; depth: number }) {
  return (
    <>
      <Card t={props.t} active={props.current === props.t.id} depth={props.depth} />
      <Show when={props.depth < 4}>
        <For each={props.kids.get(props.t.id) ?? []}>
          {(k) => <Tree t={k} kids={props.kids} current={props.current} depth={props.depth + 1} />}
        </For>
      </Show>
    </>
  )
}

function Card(props: { t: Thread; active: boolean; project?: string; depth?: number }) {
  const shown = () => shownStatus(props.t.status, unread(props.t), props.active)
  const word = () => (shown() === 'error' && limitedNow(state.usage[props.t.agent]) ? 'Limited' : statusWord[shown()])
  const strong = () => shown() === 'waiting' || shown() === 'done'
  return (
    <a
      href={`#/t/${props.t.id}`}
      onContextMenu={(e) =>
        openContextMenu(
          e,
          threadActions(props.t, (m) => pushToast(m)),
        )
      }
      aria-current={props.active ? 'page' : undefined}
      class="flex min-w-0 flex-col gap-px rounded-lg py-1.5 pr-2 max-md:py-2"
      style={{ 'padding-left': `${10 + (props.depth ?? 0) * 14}px` }}
      classList={{ 'bg-ink/[0.06]': props.active, 'hover:bg-ink/[0.03]': !props.active }}
      title={props.project ? `${props.t.title} · ${props.project}` : undefined}
    >
      <span
        class="truncate text-[13px] leading-[18px]"
        classList={{
          'text-faint': !!props.t.settled_at && !props.active,
          'font-medium text-ink': strong() || props.active,
          'text-ink/80': !props.t.settled_at && !strong() && !props.active,
        }}
      >
        {props.t.title}
      </span>
      <span class="flex min-w-0 items-center text-[11.5px] leading-4 text-faint">
        <Show when={word()}>
          <StatusMark status={shown()} class="-ml-[3px] mr-px" />
          <span class={`shrink-0 font-medium ${TONE[shown()]}`}>{word()}</span>
          <span class="shrink-0 px-1">·</span>
        </Show>
        <span class="min-w-0 truncate">
          {AGENT_NAMES[props.t.agent]}
          {props.project ? ` · ${props.project}` : ''}
        </span>
        <span class="shrink-0 px-1">·</span>
        <span class="num shrink-0">{ago(props.t.updated_at)}</span>
      </span>
    </a>
  )
}
