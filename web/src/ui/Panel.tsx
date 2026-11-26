import { ErrorBoundary, For, Show, createEffect, createMemo, createResource, createSignal, lazy, on, type JSX } from 'solid-js'
import X from 'lucide-solid/icons/x'
import ChevronRight from 'lucide-solid/icons/chevron-right'
import FileIcon from 'lucide-solid/icons/file'
import FolderIcon from 'lucide-solid/icons/folder'
import { call, state } from '../lib/store'
import { lazyPart } from '../lib/version'
import { panel, panelOverlays, panelWidth, setPanel, setPanelWidth } from '../lib/panel'
import { parseDiff, type FileDiff } from '../lib/diff'
import { Markdown } from './Timeline'
import { AgentPanel } from './Agents'
import { CodeBlock } from './tools'
import RotateCw from 'lucide-solid/icons/rotate-cw'
import { changesOf, refreshChanges, totals, type ChangedFile } from '../lib/changes'
import { ago } from '../lib/format'
import { IconBtn } from './bits'
import { inside } from '../lib/filelinks'

const TerminalPanel = lazy(lazyPart(() => import('./Terminal').then((m) => ({ default: m.TerminalPanel }))))
const BrowserPanel = lazy(lazyPart(() => import('./Browser').then((m) => ({ default: m.BrowserPanel }))))
const PhonePanel = lazy(lazyPart(() => import('./Phone').then((m) => ({ default: m.PhonePanel }))))

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

function plainGit(stderr: string) {
  if (/cannot change to|not a git repository|No such file or directory/i.test(stderr))
    return "This thread's folder isn't on its machine yet, so there's nothing to compare."
  if (/unknown revision|bad revision|ambiguous argument/i.test(stderr))
    return "The commit this turn started from is gone, so its changes can't be shown."
  return stderr.trim().split('\n')[0] || "Couldn't read the changes."
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
            Changes
            <Show when={changed() > 0}>
              <span class="num ml-1.5 text-[11.5px] text-faint">{changed()}</span>
            </Show>
          </Tab>
          <Tab active={tab() === 'files'} onClick={() => setPanel({ kind: 'files' })}>
            Files
          </Tab>
          <Tab active={tab() === 'terminal'} onClick={() => setPanel({ kind: 'terminal' })}>
            Terminal
          </Tab>
          <Tab active={tab() === 'browser'} onClick={() => setPanel({ kind: 'browser' })}>
            Browser
          </Tab>
          <Tab active={tab() === 'phone'} onClick={() => setPanel({ kind: 'phone' })}>
            Phone
          </Tab>
          <Show when={tab() === 'agent'}>
            <Tab active onClick={() => {}}>
              Agent
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
        <Show when={tab() === 'browser'}>
          <Part what="the browser">
            <BrowserPanel thread={props.thread} />
          </Part>
        </Show>
        <Show when={tab() === 'phone'}>
          <Part what="the phone">
            <PhonePanel />
          </Part>
        </Show>
        <Show when={panel()?.kind === 'agent' && (panel() as { tool: string }).tool} keyed>
          {(tool) => <AgentPanel thread={props.thread} tool={tool} />}
        </Show>
      </div>
    </aside>
  )
}

function Tab(props: { active: boolean; onClick: () => void; children: any }) {
  return (
    <button
      ref={(el) => props.active && queueMicrotask(() => el.scrollIntoView({ block: 'nearest', inline: 'nearest' }))}
      class="h-8 shrink-0 rounded-lg px-2.5 text-[13px] whitespace-nowrap"
      classList={{ 'bg-ink/[0.06] font-medium text-ink': props.active, 'text-mute hover:text-ink': !props.active }}
      onClick={props.onClick}
    >
      {props.children}
    </button>
  )
}

type Range = { repo: string; from: string; to: string }

function DiffPanel(props: { thread: string; turn?: string; file?: string }) {
  createEffect(
    on(
      () => props.thread,
      (t) => void refreshChanges(t),
    ),
  )
  return (
    <Show when={props.turn} fallback={<Uncommitted thread={props.thread} focus={props.file} />} keyed>
      {(turn) => <TurnDiff thread={props.thread} turn={turn} file={props.file} />}
    </Show>
  )
}

function Uncommitted(props: { thread: string; focus?: string }) {
  const c = () => changesOf(props.thread)
  const count = () => totals(props.thread).files
  return (
    <div>
      <div class="flex items-center gap-2 border-b border-line px-3 py-2 text-[12px] text-mute">
        <span class="min-w-0 flex-1 truncate">
          <Show when={c()?.error && c()!.at} fallback={count() ? 'Uncommitted changes, against the last commit' : ''}>
            Its machine is asleep. These are the changes it showed {ago(c()!.at)} ago.
          </Show>
        </span>
        <IconBtn label="Check again" onClick={() => void refreshChanges(props.thread)}>
          <RotateCw size={14} />
        </IconBtn>
      </div>
      <Show
        when={count()}
        fallback={
          <p class="p-5 text-[13px] text-mute">
            {c()?.error && !c()!.at ? plainGit(c()!.error!) : c() ? 'Nothing uncommitted. Everything is in a commit.' : 'Loading'}
          </p>
        }
      >
        <For each={c()!.repos.filter((r) => r.files.length)}>
          {(r) => (
            <div>
              <Show when={r.repo !== '.'}>
                <div class="border-b border-line bg-panel px-3 py-1.5 text-[12px] font-medium text-mute">{r.repo}</div>
              </Show>
              <For each={r.files}>
                {(f) => <LazyFile thread={props.thread} repo={r.repo} base={r.base} f={f} focus={props.focus === f.path} />}
              </For>
            </div>
          )}
        </For>
      </Show>
    </div>
  )
}

function LazyFile(props: { thread: string; repo: string; base: string; f: ChangedFile; focus: boolean }) {
  const [open, setOpen] = createSignal(false)
  const [diff] = createResource(
    () => (open() ? { ...props.f, base: props.base } : null),
    async (f) => {
      const args = f.untracked
        ? ['diff', '--no-color', '--no-index', '--', '/dev/null', f.path]
        : ['diff', '--no-color', '--find-renames', f.base, '--', f.path]
      const res = await call<{ stdout: string; stderr: string; code: number }>({ op: 'git', thread: props.thread, repo: props.repo, args })
      if (res.code > 1 || (res.code === 1 && !f.untracked)) throw new Error(plainGit(res.stderr))
      return parseDiff(res.stdout)[0]
    },
  )
  let el!: HTMLDivElement
  createEffect(
    on(
      () => props.focus,
      (f) => {
        if (!f) return
        setOpen(true)
        queueMicrotask(() => el?.scrollIntoView({ block: 'start' }))
      },
    ),
  )
  return (
    <div ref={el} class="border-b border-line">
      <button
        class="sticky top-0 z-10 flex w-full items-center gap-2 bg-bg px-3 py-2 text-left text-[12px] hover:bg-panel"
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <ChevronRight size={13} class="shrink-0 text-faint transition-transform duration-100" classList={{ 'rotate-90': open() }} />
        <span class="min-w-0 flex-1 truncate font-mono" style={{ direction: 'rtl', 'text-align': 'left' }}>
          <bdi>{props.f.path}</bdi>
        </span>
        <Show when={props.f.untracked}>
          <span class="text-[11.5px] text-mute">new</span>
        </Show>
        <span class="num font-mono text-ok">+{props.f.added}</span>
        <span class="num font-mono text-bad">−{props.f.removed}</span>
      </button>
      <Show when={open()}>
        <Show when={!diff.error} fallback={<p class="px-3 pb-2 text-[12px] text-bad">{String(diff.error?.message ?? diff.error)}</p>}>
          <Show when={diff()} fallback={<p class="px-3 pb-2 text-[12px] text-faint">{diff.loading ? 'Loading' : 'No difference'}</p>}>
            {(d) => <Hunks f={d()} />}
          </Show>
        </Show>
      </Show>
    </div>
  )
}

function TurnDiff(props: { thread: string; turn: string; file?: string }) {
  const turns = () => state.views[props.thread]?.turns ?? []
  const n = () =>
    turns()
      .filter((t) => t.checkpoint?.some((r) => r.files.length))
      .findIndex((t) => t.id === props.turn) + 1
  const ranges = createMemo<Range[]>(() => {
    const t = turns().find((x) => x.id === props.turn)
    return (t?.checkpoint ?? []).filter((r) => r.before && r.files.length).map((r) => ({ repo: r.repo, from: r.before!, to: r.after }))
  })
  return (
    <div>
      <div class="flex items-center gap-2 border-b border-line px-3 py-2 text-[12px] text-mute">
        <span class="min-w-0 flex-1">{state.views[props.thread]?.partial ? 'What this turn changed' : `What turn ${n()} changed`}</span>
        <button class="rounded-md px-1.5 py-0.5 text-ink hover:bg-ink/5" onClick={() => setPanel({ kind: 'diff' })}>
          Show uncommitted changes
        </button>
      </div>
      <Show when={ranges().length} fallback={<p class="p-5 text-[13px] text-mute">This turn didn't change any files.</p>}>
        <For each={ranges()}>{(r) => <RepoDiff thread={props.thread} range={r} focus={props.file} />}</For>
      </Show>
    </div>
  )
}

function RepoDiff(props: { thread: string; range: Range; focus?: string }) {
  const [diff] = createResource(
    () => ({ ...props.range }),
    async (r) => {
      const res = await call<{ stdout: string; stderr: string; code: number }>({
        op: 'git',
        thread: props.thread,
        repo: r.repo,
        args: ['diff', '--no-color', '--find-renames', r.from, r.to],
      })
      if (res.code !== 0) throw new Error(plainGit(res.stderr))
      return parseDiff(res.stdout)
    },
  )
  return (
    <div>
      <Show when={props.range.repo !== '.'}>
        <div class="border-b border-line bg-panel px-3 py-1.5 text-[12px] font-medium text-mute">{props.range.repo}</div>
      </Show>
      <Show when={!diff.error} fallback={<p class="p-4 text-[13px] text-bad">{String(diff.error?.message ?? diff.error)}</p>}>
        <Show when={!diff.loading} fallback={<p class="p-4 text-[13px] text-faint">Loading</p>}>
          <For each={diff() ?? []}>{(f) => <FileDiffView f={f} focus={props.focus === f.path} />}</For>
        </Show>
      </Show>
    </div>
  )
}

function FileDiffView(props: { f: FileDiff; focus: boolean }) {
  const [open, setOpen] = createSignal(true)
  let el!: HTMLDivElement
  createEffect(
    on(
      () => props.focus,
      (f) => f && queueMicrotask(() => el?.scrollIntoView({ block: 'start' })),
    ),
  )
  return (
    <div ref={el} class="border-b border-line">
      <button
        class="sticky top-0 z-10 flex w-full items-center gap-2 bg-bg px-3 py-2 text-left text-[12px] hover:bg-panel"
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <ChevronRight size={13} class="shrink-0 text-faint transition-transform duration-100" classList={{ 'rotate-90': open() }} />
        <span class="min-w-0 flex-1 truncate font-mono" style={{ direction: 'rtl', 'text-align': 'left' }}>
          <bdi>{props.f.path}</bdi>
        </span>
        <Show when={props.f.status !== 'modified'}>
          <span class="text-[11.5px] text-mute">{props.f.status}</span>
        </Show>
        <span class="num font-mono text-ok">+{props.f.added}</span>
        <span class="num font-mono text-bad">−{props.f.removed}</span>
      </button>
      <Show when={open()}>
        <Hunks f={props.f} />
      </Show>
    </div>
  )
}

function Hunks(props: { f: FileDiff }) {
  return (
    <Show when={!props.f.binary} fallback={<p class="px-3 pb-2 text-[12px] text-mute">Binary file</p>}>
      <div class="overflow-x-auto pb-1 font-mono text-[12px] leading-[1.5]">
        <For each={props.f.hunks}>
          {(h, hi) => (
            <>
              <Show when={hi() > 0 || h.header}>
                <div class="bg-panel px-3 py-0.5 text-[11px] text-faint">{h.header || '⋯'}</div>
              </Show>
              <For each={h.lines}>
                {(l) => (
                  <div class="flex min-w-max" classList={{ 'bg-ok/10': l.kind === '+', 'bg-bad/10': l.kind === '-' }}>
                    <span class="num w-10 shrink-0 pr-2 text-right text-faint select-none">{l.old ?? ''}</span>
                    <span class="num w-10 shrink-0 pr-2 text-right text-faint select-none">{l.new ?? ''}</span>
                    <span class="w-4 shrink-0 select-none" classList={{ 'text-ok': l.kind === '+', 'text-bad': l.kind === '-' }}>
                      {l.kind === ' ' ? '' : l.kind}
                    </span>
                    <span class="pr-4 whitespace-pre">{l.text}</span>
                  </div>
                )}
              </For>
            </>
          )}
        </For>
      </div>
    </Show>
  )
}

type Entry = { name: string; dir: boolean; size: number }

function FilesPanel(props: { thread: string }) {
  const [open, setOpen] = createSignal<{ path: string; line?: number } | null>(null)
  createEffect(
    on(panel, (p) => {
      if (p?.kind === 'files' && p.path) setOpen({ path: inside(state.threads[props.thread]?.cwd ?? '', p.path), line: p.line })
    }),
  )
  return (
    <div class="flex h-full max-sm:flex-col">
      <div class="w-56 shrink-0 overflow-auto border-line py-1.5 max-sm:max-h-48 max-sm:w-full max-sm:border-b sm:border-r">
        <Dir thread={props.thread} path="" depth={0} onOpen={(path) => setOpen({ path })} selected={open()?.path ?? null} />
      </div>
      <div class="min-w-0 flex-1 overflow-auto">
        <Show when={open()} fallback={<p class="p-5 text-[13px] text-mute">Pick a file to read it.</p>} keyed>
          {(o) => <FileView thread={props.thread} path={o.path} line={o.line} />}
        </Show>
      </div>
    </div>
  )
}

function Dir(props: { thread: string; path: string; depth: number; onOpen: (p: string) => void; selected: string | null }) {
  const [list] = createResource(
    () => props.path,
    async (p) => (await call<{ entries: Entry[] }>({ op: 'list_dir', thread: props.thread, path: p || '.' })).entries,
  )
  return (
    <Show when={!list.error} fallback={<p class="px-3 py-1 text-[12px] text-bad">{String(list.error?.message ?? '')}</p>}>
      <For each={(list() ?? []).filter((e) => e.name !== '.git')}>
        {(e) => (
          <Node
            thread={props.thread}
            e={e}
            path={props.path ? `${props.path}/${e.name}` : e.name}
            depth={props.depth}
            onOpen={props.onOpen}
            selected={props.selected}
          />
        )}
      </For>
    </Show>
  )
}

function Node(props: { thread: string; e: Entry; path: string; depth: number; onOpen: (p: string) => void; selected: string | null }) {
  const [expanded, setExpanded] = createSignal(false)
  createEffect(() => {
    if (props.e.dir && props.selected?.startsWith(props.path + '/')) setExpanded(true)
  })
  return (
    <>
      <button
        ref={(b) =>
          createEffect(() => {
            if (props.selected === props.path) queueMicrotask(() => b.scrollIntoView({ block: 'nearest' }))
          })
        }
        class="flex w-full items-center gap-1.5 py-[3px] pr-2 text-left text-[12.5px] hover:bg-ink/5"
        classList={{ 'bg-ink/[0.06] font-medium': props.selected === props.path }}
        style={{ 'padding-left': `${10 + props.depth * 12}px` }}
        onClick={() => (props.e.dir ? setExpanded(!expanded()) : props.onOpen(props.path))}
      >
        {props.e.dir ? <FolderIcon size={13} class="shrink-0 text-mute" /> : <FileIcon size={13} class="shrink-0 text-faint" />}
        <span class="truncate" classList={{ 'text-mute': props.e.name.startsWith('.') }}>
          {props.e.name}
        </span>
      </button>
      <Show when={props.e.dir && expanded()}>
        <Dir thread={props.thread} path={props.path} depth={props.depth + 1} onOpen={props.onOpen} selected={props.selected} />
      </Show>
    </>
  )
}

const IMAGE = /\.(png|jpe?g|gif|webp|svg)$/i

function FileView(props: { thread: string; path: string; line?: number }) {
  const [file] = createResource(
    () => props.path,
    (p) => call<{ text?: string; base64?: string; size: number; truncated: boolean }>({ op: 'read_file', thread: props.thread, path: p }),
  )
  const mime = () => {
    const ext = props.path.split('.').pop()?.toLowerCase()
    return ext === 'svg' ? 'image/svg+xml' : `image/${ext === 'jpg' ? 'jpeg' : ext}`
  }
  return (
    <div>
      <div class="sticky top-0 z-10 border-b border-line bg-bg px-4 py-2 font-mono text-[12px] text-mute">{props.path}</div>
      <Show when={file()} fallback={<p class="p-4 text-[13px] text-mute">{file.error ? String(file.error.message) : 'Loading'}</p>}>
        {(f) => (
          <Show
            when={IMAGE.test(props.path) && (f().base64 || f().text)}
            fallback={
              <Show
                when={typeof f().text === 'string'}
                fallback={<p class="p-4 text-[13px] text-mute">A binary file, {f().size.toLocaleString()} bytes.</p>}
              >
                <Show
                  when={/\.mdx?$/i.test(props.path)}
                  fallback={
                    <div class="[&>div]:max-h-none [&>div]:rounded-none [&>div]:border-0 [&>div]:bg-transparent">
                      <CodeBlock code={f().text!} path={props.path} line={props.line} />
                    </div>
                  }
                >
                  <div class="p-5">
                    <Markdown text={f().text!} base={props.path.split('/').slice(0, -1).join('/')} />
                  </div>
                </Show>
              </Show>
            }
          >
            <img
              class="m-4 max-w-[90%] rounded-lg border border-line"
              src={f().base64 ? `data:${mime()};base64,${f().base64}` : `data:image/svg+xml;utf8,${encodeURIComponent(f().text ?? '')}`}
            />
          </Show>
        )}
      </Show>
    </div>
  )
}
