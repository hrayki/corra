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
import { AgentIcon, StatusMark, openContextMenu, statusWord } from './bits'
import { UsageMeter } from './UsagePopover'
import { threadActions } from './actions'
import { ago } from '../lib/format'
import { route } from '../lib/route'
import { knowThreads, markSeen, unread } from '../lib/seen'
import { limitedNow } from '../lib/usage'
import { toggleSidebar } from '../lib/layout'
import { AGENT_NAMES } from '../lib/choices'
import { mostUrgent, needsYou, shownStatus, type Shown } from '../lib/status'

const FOLDED = 'corra:sidebar-folded'
const SHOWN = 6

const headBtn = 'flex size-9 shrink-0 items-center justify-center rounded-lg text-mute hover:bg-ink/[0.06] hover:text-ink'

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
      .sort((a, b) => (b.pinned_at ?? 0) - (a.pinned_at ?? 0) || b.created_at - a.created_at),
  )
  const groups = createMemo(() => {
    const byProject = new Map<string, Thread[]>()
    for (const t of threads()) {
      if (t.pinned_at || t.settled_at) continue
      const name = projectName(t)
      const list = byProject.get(name)
      if (list) list.push(t)
      else byProject.set(name, [t])
    }
    return [...byProject.entries()]
      .map(([name, open]) => {
        open.sort((a, b) => b.created_at - a.created_at || a.id.localeCompare(b.id))
        const keep = (t: Thread) => needsYou(shown(t)) || t.id === current()
        return {
          name,
          keep,
          flag: mostUrgent(open.map(shown)),
          open,
          hasCurrent: open.some((t) => t.id === current()),
        }
      })
      .sort((a, b) => a.name.localeCompare(b.name))
  })
  const settled = createMemo(() =>
    threads()
      .filter((t) => t.settled_at && !t.pinned_at)
      .sort((a, b) => (b.settled_at ?? b.updated_at) - (a.settled_at ?? a.updated_at) || b.created_at - a.created_at),
  )
  const hubProject = (name: string) => Object.values(state.projects).find((p) => p.name === name)
  const empty = () => state.loaded && pinned().length === 0 && groups().length === 0
  const urgentCount = createMemo(() => attentionThreads().length)
  return (
    <aside class="flex h-full w-full flex-col bg-panel">
      <div class="flex h-14 shrink-0 items-center gap-1 px-2.5">
        <a href="#/" aria-label="Corra" class="flex h-9 items-center rounded-lg px-1.5">
          <img src="/favicon.svg" alt="" class="size-6" />
        </a>
        <UsageMeter />
        <div class="flex-1" />
        <Show when={urgentCount() > 0}>
          <button
            type="button"
            aria-label={`Next thread needing you (${urgentCount()})`}
            title={`Next thread needing you (${urgentCount()}) · Alt+N`}
            onClick={() => jumpToNextAttention()}
            class="inline-flex h-8 items-center gap-1 rounded-lg bg-warn/15 px-2 text-[12.5px] font-medium text-warn hover:bg-warn/25"
          >
            <BellRing size={13.5} />
            <span class="num">{urgentCount()}</span>
          </button>
        </Show>
        <a href="#/new-project" aria-label="New project" title="New project" class={headBtn}>
          <FolderPlus size={17} />
        </a>
        <a href="#/new" aria-label="New thread" title="New thread" class={headBtn}>
          <SquarePen size={17} />
        </a>
      </div>
      <div class="px-3 pb-2.5">
        <div class="flex h-9 items-center gap-2 rounded-lg bg-ink/[0.045] px-2.5 text-mute focus-within:bg-ink/[0.075]">
          <Search size={14} />
          <input
            aria-label="Find a thread"
            class="min-w-0 flex-1 bg-transparent text-[13.5px] text-ink placeholder:text-faint"
            placeholder="Find a thread"
            value={q()}
            onInput={(e) => setQ(e.currentTarget.value)}
          />
          <button
            type="button"
            aria-label="Open quick switcher"
            title="Quick switcher (Ctrl+K)"
            onClick={() => setPaletteOpen(true)}
            class="shrink-0 rounded border border-line/80 bg-raised/70 px-1.5 py-px font-mono text-[11px] text-faint hover:border-faint hover:text-ink max-md:hidden"
          >
            ⌘K
          </button>
        </div>
      </div>
      <nav class="min-h-0 overflow-y-auto px-2.5 pt-1 pb-4" classList={{ 'flex-1': !settledOpen(), 'shrink grow-0': settledOpen() }}>
        <Show when={pinned().length}>
          <Group title="Pinned" icon={<Pin size={13.5} />}>
            <For each={pinned()}>{(t) => <Card t={t} active={current() === t.id} project={projectName(t)} />}</For>
          </Group>
        </Show>
        <For each={groups()}>
          {(g) => {
            const open = () => !folded()[g.name]
            const listed = () => (more()[g.name] ? g.open : g.open.filter((t, i) => i < SHOWN || g.keep(t)))
            const hidden = () => g.open.length - listed().length
            return (
              <Group
                title={g.name}
                folded={!open()}
                activeCollapsed={!open() && g.hasCurrent}
                count={!open() ? g.open.length : undefined}
                flag={open() ? undefined : g.flag}
                onFold={() => setFolded((f) => ({ ...f, [g.name]: !f[g.name] }))}
                newHref={`#/new/${encodeURIComponent(g.name)}`}
                settingsHref={hubProject(g.name) ? `#/project/${hubProject(g.name)!.id}` : undefined}
                trusted={!!hubProject(g.name)?.trusted}
              >
                <Show when={open()}>
                  <div class="mt-0.5 space-y-0.5">
                    <For each={listed()}>{(t) => <Card t={t} active={current() === t.id} />}</For>
                  </div>
                  <Show when={hidden() > 0 || more()[g.name]}>
                    <button
                      class="mt-0.5 flex h-8 w-full items-center rounded-lg px-2.5 text-[12.5px] text-faint hover:text-ink max-md:h-10"
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
          <p class="px-2.5 py-6 text-[13.5px] leading-relaxed text-mute">
            {q().trim() ? 'No matching threads.' : 'No active threads yet. Start one and it shows up here.'}
          </p>
        </Show>
      </nav>
      <Show when={settledOpen()}>
        <div class="flex min-h-44 flex-1 flex-col justify-end overflow-hidden border-t border-line">
          <div class="min-h-0 space-y-0.5 overflow-y-auto px-2.5 py-2.5">
            <For each={settled()} fallback={<p class="px-2.5 py-1.5 text-[13px] text-faint">Nothing settled.</p>}>
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
          <PanelLeftClose size={17} />
        </button>
        <div class="flex flex-1 justify-center">
          <Show when={settled().length}>
            <button
              class="flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[13px] text-mute hover:bg-ink/[0.06] hover:text-ink"
              classList={{ 'bg-ink/[0.07] text-ink': settledOpen() }}
              aria-expanded={settledOpen()}
              onClick={() => setSettledOpen(!settledOpen())}
            >
              Settled <span class="num text-faint">{settled().length}</span>
              <ChevronUp size={13.5} class="transition-transform duration-100" classList={{ 'rotate-180': settledOpen() }} />
            </button>
          </Show>
        </div>
        <HubBadge />
        <a
          href="#/settings"
          aria-label="Settings"
          title="Settings"
          class={headBtn}
          classList={{ 'bg-ink/[0.07] text-ink': route().kind === 'settings' }}
        >
          <Settings size={17} />
        </a>
      </div>
    </aside>
  )
}

function HubBadge() {
  const up = () => state.conn === 'open'
  return (
    <Show when={!up()}>
      <a href="#/settings" class="flex h-8 items-center gap-1.5 rounded-lg px-2 text-[12.5px] text-faint hover:text-mute">
        <span class="size-1.5 rounded-full bg-warn" />
        Reconnecting
      </a>
    </Show>
  )
}

const hoverBtn =
  'flex size-7 items-center justify-center rounded-md text-faint opacity-0 group-hover:opacity-100 hover:bg-ink/[0.06] hover:text-ink focus-visible:opacity-100 max-md:opacity-100'

function Group(props: {
  title: string
  icon?: JSX.Element
  folded?: boolean
  activeCollapsed?: boolean
  count?: number
  onFold?: () => void
  newHref?: string
  settingsHref?: string
  trusted?: boolean
  flag?: Shown
  children: JSX.Element
}) {
  return (
    <section class="mt-3 first:mt-0">
      <div
        class="group flex h-9 items-center gap-1.5 rounded-lg pr-1 pl-2 transition-colors duration-100"
        classList={{ 'bg-ink/[0.075]': !!props.activeCollapsed }}
      >
        <button
          class="flex min-w-0 flex-1 items-center gap-2 text-left"
          aria-expanded={props.onFold ? !props.folded : undefined}
          disabled={!props.onFold}
          onClick={() => props.onFold?.()}
        >
          <Show
            when={props.onFold}
            fallback={
              <Show when={props.icon}>
                <span class="flex size-4 shrink-0 items-center justify-center text-faint">{props.icon}</span>
              </Show>
            }
          >
            <ChevronRight
              size={13}
              class="shrink-0 text-faint transition-transform duration-100 group-hover:text-mute"
              classList={{ 'rotate-90': !props.folded }}
            />
          </Show>
          <span
            class="min-w-0 truncate text-[13px] font-semibold tracking-[-0.005em] transition-colors duration-100"
            classList={{
              'text-ink': !!props.activeCollapsed,
              'text-mute group-hover:text-ink': !props.activeCollapsed,
            }}
          >
            {props.title}
          </span>
          <Show when={props.trusted}>
            <span
              class="shrink-0 text-[11px] font-medium text-warn"
              title="Manager: the hub's own keys, every repo, and the tools that manage corra"
            >
              manager
            </span>
          </Show>
          <Show when={props.flag && FLAG[props.flag]}>
            {(label) => (
              <span class="flex shrink-0" title={label()}>
                <StatusMark status={props.flag!} class="-mx-[2px]" />
                <span class="sr-only">{label()}</span>
              </span>
            )}
          </Show>
          <Show when={props.count !== undefined}>
            <span class="num ml-auto pr-1 text-[11.5px] text-faint">{props.count}</span>
          </Show>
        </button>
        <Show when={props.settingsHref}>
          <a href={props.settingsHref} aria-label={`${props.title}'s settings`} title={`${props.title}'s settings`} class={hoverBtn}>
            <SlidersHorizontal size={13} />
          </a>
        </Show>
        <Show when={props.newHref}>
          <a href={props.newHref} aria-label={`New thread in ${props.title}`} title={`New thread in ${props.title}`} class={hoverBtn}>
            <Plus size={14} />
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

function Card(props: { t: Thread; active: boolean; project?: string }) {
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
      class="group/card flex min-w-0 flex-col gap-1 rounded-lg px-2.5 py-2 transition-colors duration-100 max-md:py-2.5"
      classList={{ 'bg-ink/[0.075]': props.active }}
      title={props.project ? `${props.t.title} · ${props.project}` : undefined}
    >
      <span
        class="truncate text-[14px] leading-[20px] transition-colors duration-100"
        classList={{
          'text-faint group-hover/card:text-mute': !!props.t.settled_at && !props.active,
          'font-medium text-ink': strong() || props.active,
          'text-ink/80 group-hover/card:text-ink': !props.t.settled_at && !strong() && !props.active,
        }}
      >
        {props.t.title}
      </span>
      <span class="flex min-w-0 items-center gap-1.5 text-[12px] leading-4 text-faint">
        <AgentIcon agent={props.t.agent} size={13} title={AGENT_NAMES[props.t.agent]} class="text-faint group-hover/card:text-mute" />
        <Show when={word()}>
          <span class="inline-flex shrink-0 items-center gap-1">
            <StatusMark status={shown()} class="-mx-[2px]" />
            <span class={`font-medium ${TONE[shown()]}`}>{word()}</span>
          </span>
        </Show>
        <Show when={props.project}>
          <span class="min-w-0 truncate text-faint">{props.project}</span>
        </Show>
        <span class="num ml-auto shrink-0 text-[11.5px] text-faint">{ago(props.t.updated_at)}</span>
      </span>
    </a>
  )
}
