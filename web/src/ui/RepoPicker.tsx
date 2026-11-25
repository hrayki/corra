import { For, Show, createMemo, createSignal, onCleanup, onMount } from 'solid-js'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import Lock from 'lucide-solid/icons/lock'
import Search from 'lucide-solid/icons/search'
import { callOn } from '../lib/store'
import { ago } from '../lib/format'

export type RepoRow = { full_name: string; private: boolean; description: string | null; pushed_at: string }

let cached: Promise<RepoRow[]> | null = null

export function listRepos(): Promise<RepoRow[]> {
  cached ??= callOn<RepoRow[]>('hub', { op: 'list_repos' })
    .then((l) => (Array.isArray(l) ? l : []))
    .catch((e) => {
      cached = null
      throw e
    })
  return cached
}

export function RepoPicker(props: { value?: string; placeholder?: string; exclude?: string[]; onPick: (repo: string) => void }) {
  const [open, setOpen] = createSignal(false)
  const [q, setQ] = createSignal('')
  const [repos, setRepos] = createSignal<RepoRow[] | null>(null)
  const [error, setError] = createSignal('')
  let box!: HTMLDivElement
  const load = () => {
    setError('')
    listRepos().then(setRepos, (e) => setError(e instanceof Error ? e.message : String(e)))
  }
  const typed = () => {
    const t = q()
      .trim()
      .replace(/^https:\/\/github\.com\//, '')
      .replace(/\.git$/, '')
    return /^[\w.-]+\/[\w.-]+$/.test(t) ? t : ''
  }
  const shown = createMemo(() => {
    const needle = q().trim().toLowerCase()
    return (repos() ?? [])
      .filter((r) => !props.exclude?.includes(r.full_name))
      .filter((r) => !needle || r.full_name.toLowerCase().includes(needle) || (r.description ?? '').toLowerCase().includes(needle))
      .slice(0, 100)
  })
  const pick = (r: string) => {
    props.onPick(r)
    setOpen(false)
    setQ('')
  }
  const onDoc = (e: PointerEvent) => {
    if (open() && !box.contains(e.target as Node)) setOpen(false)
  }
  onMount(() => document.addEventListener('pointerdown', onDoc, true))
  onCleanup(() => document.removeEventListener('pointerdown', onDoc, true))
  return (
    <div ref={box} class="relative w-full sm:max-w-md">
      <button
        type="button"
        class="flex h-8 w-full items-center gap-2 rounded-lg border border-line bg-raised px-2.5 text-left text-[13px] hover:border-faint"
        aria-expanded={open()}
        onClick={() => {
          setOpen(!open())
          if (!repos()) load()
        }}
      >
        <span class="min-w-0 flex-1 truncate" classList={{ 'text-faint': !props.value, 'font-mono text-[12.5px]': !!props.value }}>
          {props.value || props.placeholder || 'Pick a repo'}
        </span>
        <ChevronDown size={14} class="shrink-0 text-faint" />
      </button>
      <Show when={open()}>
        <div class="absolute top-9 right-0 left-0 z-30 overflow-hidden rounded-xl border border-line bg-raised shadow-[var(--shadow)]">
          <label class="flex h-10 items-center gap-2 border-b border-line px-3 text-mute">
            <Search size={14} />
            <input
              class="min-w-0 flex-1 bg-transparent text-[13.5px] text-ink placeholder:text-faint"
              placeholder="Find a repo, or type owner/name"
              autofocus
              value={q()}
              onInput={(e) => setQ(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === 'Escape') setOpen(false)
                if (e.key === 'Enter') {
                  const first = shown()[0]?.full_name
                  if (first) pick(first)
                  else if (typed()) pick(typed())
                }
              }}
            />
          </label>
          <div class="max-h-64 overflow-y-auto py-1">
            <Show when={error()}>
              <p class="px-3 py-2 text-[13px] text-bad">{error()}</p>
            </Show>
            <Show when={repos() || error()} fallback={<p class="px-3 py-2 text-[13px] text-faint">Loading your repos…</p>}>
              <For each={shown()}>
                {(r) => (
                  <button
                    type="button"
                    class="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-ink/[0.04]"
                    onClick={() => pick(r.full_name)}
                  >
                    <span class="min-w-0 flex-1">
                      <span class="flex items-center gap-1.5 text-[13.5px]">
                        <span class="truncate">{r.full_name}</span>
                        <Show when={r.private}>
                          <Lock size={11} class="shrink-0 text-faint" />
                        </Show>
                      </span>
                      <Show when={r.description}>
                        <span class="block truncate text-[12px] text-mute">{r.description}</span>
                      </Show>
                    </span>
                    <span class="num shrink-0 text-[11.5px] text-faint">{ago(Date.parse(r.pushed_at))}</span>
                  </button>
                )}
              </For>
              <Show when={!shown().length && typed()}>
                <button type="button" class="w-full px-3 py-2 text-left text-[13px] hover:bg-ink/[0.04]" onClick={() => pick(typed())}>
                  Use <span class="font-mono text-[12.5px]">{typed()}</span>
                </button>
              </Show>
              <Show when={!shown().length && !typed() && repos()}>
                <p class="px-3 py-2 text-[13px] text-faint">No repos match.</p>
              </Show>
            </Show>
          </div>
        </div>
      </Show>
    </div>
  )
}
