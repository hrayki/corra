import { ErrorBoundary, Show, createSignal, lazy, type JSX } from 'solid-js'
import X from 'lucide-solid/icons/x'
import FileDiff from 'lucide-solid/icons/file-diff'
import FolderTree from 'lucide-solid/icons/folder-tree'
import SquareTerminal from 'lucide-solid/icons/square-terminal'
import Bot from 'lucide-solid/icons/bot'
import { call, state } from '../lib/store'
import { lazyPart } from '../lib/version'
import { panel, panelOverlays, panelWidth, setPanel, setPanelWidth } from '../lib/panel'
import { totals } from '../lib/changes'
import { providerOf } from '../lib/catalog'
import { AgentPanel } from './Agents'
import { DiffPanel } from './panel/DiffPanel'
import { FilesPanel } from './panel/FilesPanel'
import { Btn } from './bits'

const TerminalPanel = lazy(lazyPart(() => import('./Terminal').then((m) => ({ default: m.TerminalPanel }))))

function Part(props: { what: string; children: JSX.Element }) {
  return (
    <ErrorBoundary
      fallback={
        <div class="p-6 text-[13px] text-mute">
          Couldn't load {props.what}. Corra may have been updated since this page opened.{' '}
          <button class="underline decoration-line underline-offset-4 hover:text-ink" onClick={() => location.reload()}>
            Reload
          </button>
        </div>
      }
    >
      {props.children}
    </ErrorBoundary>
  )
}

export function projectOf(thread: string) {
  const t = state.threads[thread]
  return t ? state.projects[t.project] : undefined
}

export function machineOf(thread: string) {
  const t = state.threads[thread]
  if (!t) return undefined
  if (t.machine && state.machines[t.machine]) return state.machines[t.machine]
  return Object.values(state.machines)
    .filter((m) => m.project === t.project && m.status !== 'destroyed')
    .sort((a, b) => b.created_at - a.created_at)[0]
}

export function WakeNotice(props: { thread: string; compact?: boolean }) {
  const [waking, setWaking] = createSignal(false)
  const [err, setErr] = createSignal('')
  const p = () => projectOf(props.thread)
  const m = () => machineOf(props.thread)
  const makes = () => !!providerOf(p()?.provider)?.creates_machines
  const stage = () => (p() ? state.projectStatus[p()!.id]?.stage : undefined)
  const needsWake = () => !!p() && makes() && m()?.status !== 'connected'
  const wake = async () => {
    const proj = p()
    if (!proj) return
    setErr('')
    setWaking(true)
    try {
      await call({ op: 'wake_project', project: proj.id })
    } catch (e) {
      setErr((e as Error).message)
    } finally {
      setWaking(false)
    }
  }
  return (
    <Show when={needsWake()}>
      <div
        class={
          props.compact
            ? 'flex items-center gap-2 border-b border-line bg-panel px-3 py-2 text-[12px]'
            : 'm-4 rounded-xl border border-line bg-panel p-4 text-[13px]'
        }
      >
        <div class="min-w-0 flex-1">
          <Show
            when={stage() || waking()}
            fallback={
              <span class="text-mute">
                {m() ? 'This project’s machine is asleep.' : 'This project doesn’t have a running machine yet.'}
              </span>
            }
          >
            <span class="inline-flex items-center gap-2 text-ink">
              <span class="size-2 shrink-0 rounded-full bg-accent pulse" />
              {stage() ?? 'Waking machine…'}
            </span>
          </Show>
          <Show when={err()}>
            <div class="mt-1 text-[12px] text-bad">{err()}</div>
          </Show>
        </div>
        <Show when={!stage() && !waking()}>
          <Btn kind="primary" onClick={() => void wake()}>
            Wake machine
          </Btn>
        </Show>
      </div>
    </Show>
  )
}

export function SidePanel(props: { thread: string }) {
  const tab = () => panel()?.kind
  const changed = () => totals(props.thread).files
  const startDrag = (e: PointerEvent) => {
    e.preventDefault()
    const move = (ev: PointerEvent) => setPanelWidth(window.innerWidth - ev.clientX)
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  return (
    <aside
      class="relative flex min-w-0 flex-col border-line bg-bg max-md:flex-1 md:shrink-0 md:border-l"
      classList={{ 'md:absolute md:inset-y-0 md:right-0 md:z-20 md:shadow-[var(--shadow)]': panelOverlays() }}
      style={{ width: window.innerWidth >= 768 ? (panelOverlays() ? 'min(100%, 640px)' : `${panelWidth()}px`) : undefined }}
    >
      <div class="absolute top-0 bottom-0 -left-1 z-10 w-2 cursor-col-resize max-md:hidden" onPointerDown={startDrag} />
      <div class="flex h-[52px] shrink-0 items-center gap-1 border-b border-line px-3">
        <div class="flex min-w-0 flex-1 items-center gap-1 no-scrollbar overflow-x-auto">
          <Tab active={tab() === 'diff'} onClick={() => setPanel({ kind: 'diff' })}>
            <FileDiff size={14} />
            <span>Changes</span>
            <Show when={changed() > 0}>
              <span class="num text-[11.5px] text-faint">{changed()}</span>
            </Show>
          </Tab>
          <Tab active={tab() === 'files'} onClick={() => setPanel({ kind: 'files' })}>
            <FolderTree size={14} />
            <span>Files</span>
          </Tab>
          <Tab active={tab() === 'terminal'} onClick={() => setPanel({ kind: 'terminal' })}>
            <SquareTerminal size={14} />
            <span>Terminal</span>
          </Tab>
          <Show when={tab() === 'agent'}>
            <Tab active onClick={() => {}}>
              <Bot size={14} />
              <span>Agent</span>
            </Tab>
          </Show>
        </div>
        <button
          aria-label="Close panel"
          title="Close"
          class="flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 hover:text-ink"
          onClick={() => setPanel(null)}
        >
          <X size={16} />
        </button>
      </div>
      <div class="min-h-0 flex-1 overflow-auto">
        <Show when={tab() === 'diff'}>
          <DiffPanel thread={props.thread} turn={(panel() as { turn?: string }).turn} file={(panel() as { file?: string }).file} />
        </Show>
        <Show when={tab() === 'files'}>
          <FilesPanel thread={props.thread} />
        </Show>
        <Show when={tab() === 'terminal'}>
          <Part what="the terminal">
            <TerminalPanel thread={props.thread} />
          </Part>
        </Show>
        <Show when={panel()?.kind === 'agent' && (panel() as { tool: string }).tool} keyed>
          {(tool) => <AgentPanel thread={props.thread} tool={tool} />}
        </Show>
      </div>
    </aside>
  )
}

function Tab(props: { active: boolean; onClick: () => void; children: JSX.Element }) {
  return (
    <button
      ref={(el) => props.active && queueMicrotask(() => el.scrollIntoView({ block: 'nearest', inline: 'nearest' }))}
      class="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-[13px] whitespace-nowrap"
      classList={{ 'bg-ink/[0.06] font-medium text-ink': props.active, 'text-mute hover:text-ink': !props.active }}
      onClick={props.onClick}
    >
      {props.children}
    </button>
  )
}
