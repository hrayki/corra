import { For, Show, createEffect, createMemo, createResource, createSignal, on } from 'solid-js'
import ChevronRight from 'lucide-solid/icons/chevron-right'
import RotateCw from 'lucide-solid/icons/rotate-cw'
import { call, state } from '../../lib/store'
import { setPanel } from '../../lib/panel'
import { parseDiff, type FileDiff } from '../../lib/diff'
import { changesOf, refreshChanges, totals, type ChangedFile } from '../../lib/changes'
import { ago } from '../../lib/format'
import { IconBtn } from '../bits'
import { WakeNotice, machineOf } from '../Panel'

type Range = { repo: string; from: string; to: string }

function plainGit(stderr: string) {
  if (/cannot change to|not a git repository|No such file or directory/i.test(stderr))
    return "This thread's folder isn't on its machine yet, so there's nothing to compare."
  if (/unknown revision|bad revision|ambiguous argument/i.test(stderr))
    return "The commit this turn started from is gone, so its changes can't be shown."
  return stderr.trim().split('\n')[0] || "Couldn't read the changes."
}

export function DiffPanel(props: { thread: string; turn?: string; file?: string }) {
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
  const connected = () => machineOf(props.thread)?.status === 'connected'
  createEffect(
    on(connected, (ok) => {
      if (ok) void refreshChanges(props.thread)
    }),
  )
  return (
    <div>
      <WakeNotice thread={props.thread} compact />
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
