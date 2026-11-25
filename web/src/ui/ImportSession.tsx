import { For, Show, createMemo, createResource, createSignal } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import Search from 'lucide-solid/icons/search'
import { state } from '../lib/store'
import { claudeSessions, importSession, type ClaudeSession } from '../lib/machines'
import { ago } from '../lib/format'
import { Menu } from './bits'

const pill = 'inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-raised px-2.5 text-[13px] hover:border-faint'

function folder(path: string, home: string) {
  return home && path.startsWith(home) ? '~' + path.slice(home.length) : path
}

function size(n: number) {
  return n >= 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`
}

export function ImportSession(props: { machine?: string }) {
  const machine = () =>
    props.machine ??
    state.thisMachine ??
    Object.values(state.machines).find((m) => m.provider === 'existing' && m.status === 'connected')?.id
  const [sessions] = createResource(machine, (m) => claudeSessions(m))
  const home = () => (machine() ? (state.machines[machine()!]?.info?.home ?? '') : '')
  const where = () => (machine() === state.thisMachine ? 'this computer' : (state.machines[machine() ?? '']?.name ?? 'the machine'))
  const [q, setQ] = createSignal('')
  const [into, setInto] = createSignal<string | null>(null)
  const [busy, setBusy] = createSignal<string | null>(null)
  const [error, setError] = createSignal('')
  const projects = createMemo(() =>
    Object.values(state.projects)
      .filter((p) => p.default_machine === machine() && p.path)
      .sort((a, b) => a.name.localeCompare(b.name)),
  )
  const filtered = createMemo(() => {
    const needle = q().trim().toLowerCase()
    const list = sessions()
    return (Array.isArray(list) ? list : []).filter(
      (s) => !needle || s.title.toLowerCase().includes(needle) || s.cwd.toLowerCase().includes(needle),
    )
  })
  const bring = async (s: ClaudeSession) => {
    setError('')
    setBusy(s.id)
    try {
      const r = await importSession(machine()!, s.id, into() ?? undefined)
      location.hash = `#/t/${r.thread}`
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(null)
    }
  }

  return (
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto max-w-2xl px-4 pt-4 pb-16 sm:px-6 md:pt-10">
        <div class="mb-2 flex items-center gap-2">
          <a
            href="#/"
            aria-label="Back"
            class="-ml-1 flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 md:hidden"
          >
            <ArrowLeft size={18} />
          </a>
          <h1 class="text-[20px] font-semibold tracking-tight">Bring in a Claude Code session</h1>
        </div>
        <p class="mb-6 text-[13.5px] text-mute">
          Sessions Claude Code has on {where()}. The thread keeps its history and carries on the same conversation.
        </p>

        <div class="mb-3 flex flex-wrap items-center gap-2 text-[13px]">
          <span class="text-mute">Into</span>
          <Menu
            label="Into which project"
            class={pill}
            items={[
              { label: 'Its own folder', checked: into() === null, onSelect: () => setInto(null) },
              ...(projects().length ? (['sep'] as const) : []),
              ...projects().map((p) => ({ label: p.name, checked: into() === p.id, onSelect: () => setInto(p.id) })),
            ]}
          >
            <span>{projects().find((p) => p.id === into())?.name ?? 'Its own folder'}</span>
            <ChevronDown size={13} class="text-faint" />
          </Menu>
        </div>

        <div class="overflow-hidden rounded-xl border border-line">
          <label class="flex h-10 items-center gap-2 border-b border-line px-3 text-mute">
            <Search size={14} />
            <input
              class="min-w-0 flex-1 bg-transparent text-[13.5px] text-ink placeholder:text-faint"
              placeholder="Find a session"
              autofocus
              value={q()}
              onInput={(e) => setQ(e.currentTarget.value)}
            />
          </label>
          <div class="max-h-[60vh] overflow-y-auto">
            <Show when={!sessions.loading} fallback={<p class="px-3 py-3 text-[13px] text-faint">Looking for sessions…</p>}>
              <Show when={!sessions.error} fallback={<p class="px-3 py-3 text-[13px] text-bad">{(sessions.error as Error)?.message}</p>}>
                <For each={filtered()} fallback={<p class="px-3 py-3 text-[13px] text-faint">No sessions to bring in.</p>}>
                  {(s) => (
                    <button
                      type="button"
                      class="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-ink/[0.03] disabled:opacity-60"
                      disabled={!!busy()}
                      onClick={() => void bring(s)}
                    >
                      <span class="min-w-0 flex-1">
                        <span class="block truncate text-[13.5px]">{s.title}</span>
                        <span class="block truncate font-mono text-[11.5px] text-mute">{folder(s.cwd, home() ?? '')}</span>
                      </span>
                      <span class="num shrink-0 text-right text-[11.5px] text-faint">
                        {busy() === s.id ? 'Bringing in…' : `${ago(s.updated_at)} · ${size(s.size)}`}
                      </span>
                    </button>
                  )}
                </For>
              </Show>
            </Show>
          </div>
        </div>
        <p class="mt-3 text-[12.5px] text-bad">{error()}</p>
      </div>
    </div>
  )
}
