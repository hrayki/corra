import { Match, Show, Switch, createSignal, onMount } from 'solid-js'
import { Sidebar } from './ui/Sidebar'
import { ThreadPane } from './ui/ThreadPane'
import { NewThread } from './ui/NewThread'
import { Settings } from './ui/Settings'
import { Gate } from './ui/Gate'
import { ContextMenuHost, PictureViewer } from './ui/bits'
import { RenameDialog, ToastHost } from './ui/Overlays'
import { CommandPalette } from './ui/CommandPalette'
import { NewProject } from './ui/NewProject'
import { ImportSession } from './ui/ImportSession'
import { ProjectPage } from './ui/ProjectPage'
import { boot, state } from './lib/store'
import { route } from './lib/route'
import './lib/theme'
import PanelLeftOpen from 'lucide-solid/icons/panel-left-open'
import SquarePen from 'lucide-solid/icons/square-pen'
import SettingsIcon from 'lucide-solid/icons/settings'
import { setSidebarWidth, sidebarCollapsed, sidebarWidth, toggleSidebar } from './lib/layout'

const rail = 'flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/[0.06] hover:text-ink'
import './lib/typeto'
import './lib/tooltip'
import './lib/notify'

export default function App() {
  onMount(() => void boot())
  return (
    <Switch>
      <Match when={state.boot === 'loading'}>
        <div class="h-full" />
      </Match>
      <Match when={state.boot === 'unreachable'}>
        <div class="flex h-full items-center justify-center p-6 text-center">
          <div>
            <p class="text-[14px] font-medium">Can't reach corra.</p>
            <button class="mt-3 text-[13px] underline decoration-line underline-offset-4" onClick={() => location.reload()}>
              Try again
            </button>
          </div>
        </div>
      </Match>
      <Match when={state.boot === 'signin'}>
        <Gate onDone={() => location.reload()} />
      </Match>
      <Match when={state.boot === 'ready'}>
        <Main />
      </Match>
    </Switch>
  )
}

function Main() {
  const r = route
  const [dragging, setDragging] = createSignal(false)
  const offline = () => state.loaded && state.conn !== 'open'
  return (
    <div class="flex h-full">
      <PictureViewer />
      <ContextMenuHost />
      <ToastHost />
      <RenameDialog />
      <CommandPalette />
      <Show when={sidebarCollapsed()}>
        <div class="hidden h-full w-12 shrink-0 flex-col items-center border-r border-line bg-panel md:flex">
          <div class="flex flex-col items-center gap-1 py-2.5">
            <a href="#/" aria-label="Corra" class="flex size-8 items-center justify-center rounded-lg">
              <img src="/favicon.svg" alt="" class="size-[22px]" />
            </a>
            <a href="#/new" class={rail} aria-label="New thread" title="New thread">
              <SquarePen size={16} />
            </a>
          </div>
          <div class="flex-1" />
          <a href="#/settings" class={rail} aria-label="Settings" title="Settings">
            <SettingsIcon size={16} />
          </a>
          <div class="mt-1 flex h-[52px] w-full items-center justify-center border-t border-line">
            <button class={rail} aria-label="Show the sidebar" title="Show the sidebar (Ctrl+B)" onClick={() => toggleSidebar(false)}>
              <PanelLeftOpen size={16} />
            </button>
          </div>
        </div>
      </Show>
      <div
        class="relative h-full shrink-0 md:border-r md:border-line"
        style={{ '--sidebar': `${sidebarWidth()}px` }}
        classList={{
          'max-md:hidden': r().kind !== 'home',
          'max-md:w-full': r().kind === 'home',
          'md:w-[var(--sidebar)]': true,
          'md:hidden': sidebarCollapsed(),
        }}
      >
        <Sidebar />
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize the sidebar"
          title="Drag to resize"
          class="group absolute top-0 -right-[4px] z-10 flex h-full w-[8px] cursor-col-resize justify-center max-md:hidden"
          onPointerDown={(e) => {
            e.preventDefault()
            const start = e.clientX
            const from = sidebarWidth()
            const move = (ev: PointerEvent) => setSidebarWidth(from + ev.clientX - start)
            const up = () => {
              window.removeEventListener('pointermove', move)
              window.removeEventListener('pointerup', up)
              document.body.style.cursor = ''
              setDragging(false)
            }
            setDragging(true)
            document.body.style.cursor = 'col-resize'
            window.addEventListener('pointermove', move)
            window.addEventListener('pointerup', up)
          }}
          onDblClick={() => setSidebarWidth(268)}
        >
          <div
            class="h-full w-[2px] transition-colors duration-150 group-hover:bg-ink/20 group-hover:delay-300"
            classList={{ 'bg-ink/20': dragging() }}
          />
        </div>
      </div>
      <main class="flex min-w-0 flex-1 flex-col" classList={{ 'max-md:hidden': r().kind === 'home' }}>
        <Show when={offline()}>
          <div class="border-b border-line bg-warn/10 px-5 py-1.5 text-[12.5px]">Reconnecting to the hub…</div>
        </Show>
        <Switch>
          <Match when={r().kind === 'thread' && (r() as { id: string }).id} keyed>
            {(id) => <ThreadPane id={id} />}
          </Match>
          <Match when={r().kind === 'settings'}>
            <Settings />
          </Match>
          <Match when={r().kind === 'new-project'}>
            <NewProject />
          </Match>
          <Match when={r().kind === 'import'}>
            <ImportSession machine={(r() as { machine?: string }).machine} />
          </Match>
          <Match when={r().kind === 'project' && (r() as { id: string }).id} keyed>
            {(id) => <ProjectPage id={id} />}
          </Match>
          <Match when={true}>
            <NewThread project={(r() as { project?: string }).project} />
          </Match>
        </Switch>
      </main>
    </div>
  )
}
