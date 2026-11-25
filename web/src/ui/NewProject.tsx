import { For, Show, createEffect, createMemo, createResource, createSignal } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import Lock from 'lucide-solid/icons/lock'
import Search from 'lucide-solid/icons/search'
import { callOn, state, type ProjectX } from '../lib/store'
import { ago } from '../lib/format'
import { makers, paired, providerOf, tools as toolInfo } from '../lib/catalog'
import { Btn, Menu, inputClass } from './bits'
import { FolderPicker } from './NewThread'

type RepoRow = { full_name: string; private: boolean; description: string | null; pushed_at: string }
type SetupRow = { name: string; repos: string[]; env: string[] }

const MAIN = [
  { value: false, label: 'Ask first', hint: 'Pushing to the default branch or a tag, and merging, wait for your yes.' },
  { value: true, label: 'Allow', hint: 'It pushes to the default branch and tags, and merges, on its own.' },
]

const pill = 'inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-raised px-2.5 text-[13px] hover:border-faint'

export function NewProject() {
  const hubUp = () => state.conn.hub === 'open'
  const [source, setSource] = createSignal<'github' | 'folder'>('github')
  const [reposError, setReposError] = createSignal('')
  const [repos] = createResource(hubUp, async (up) => {
    if (!up) return []
    try {
      const list = await callOn<RepoRow[]>('hub', { op: 'list_repos' })
      setReposError('')
      return Array.isArray(list) ? list : []
    } catch (e) {
      const m = (e as Error).message
      setReposError(
        /401|Bad credentials/i.test(m)
          ? 'GitHub refused the hub’s key. Replace the GitHub key in Settings, under Keys.'
          : /no GitHub credential|github\.token/i.test(m)
            ? 'The hub has no GitHub key yet. Add one in Settings, under Keys.'
            : `Couldn't list your GitHub repos: ${m}`,
      )
      return []
    }
  })
  const [setups] = createResource(hubUp, (up) => (up ? callOn<SetupRow[]>('hub', { op: 'list_setups' }) : Promise.resolve([])))
  const [envNames, { refetch: refetchEnv }] = createResource(hubUp, (up) =>
    up ? callOn<string[]>('hub', { op: 'list_env' }) : Promise.resolve([]),
  )
  const phoneReady = () => !!toolInfo().find((t) => t.name === 'phone')?.ready

  const [q, setQ] = createSignal('')
  const [picked, setPicked] = createSignal<string[]>([])
  const [setup, setSetup] = createSignal<string | null>(null)
  const [setupTouched, setSetupTouched] = createSignal(false)
  const [where, setWhere] = createSignal<string>('')
  const [allowMain, setAllowMain] = createSignal(false)
  const [browser, setBrowser] = createSignal(true)
  const [phone, setPhone] = createSignal(true)
  const [extraEnv, setExtraEnv] = createSignal<string[]>([])
  const [values, setValues] = createSignal<Record<string, string>>({})
  const [name, setName] = createSignal<string | null>(null)
  const [error, setError] = createSignal('')
  const [busy, setBusy] = createSignal(false)

  const machines = createMemo(() =>
    Object.values(state.machines).filter((m) => m.src === 'hub' && paired(m.provider) && m.status !== 'destroyed'),
  )
  const places = createMemo(() => {
    const out = makers().map((p) => ({ key: p.name, label: p.label }))
    for (const m of machines()) out.push({ key: `machine:${m.id}`, label: m.name })
    return out
  })
  const place = () => places().find((p) => p.key === where()) ?? places()[0]
  const [kindName, setKindName] = createSignal<string>('')
  const kinds = () => providerOf(place()?.key)?.kinds ?? []
  const kind = () => kinds().find((k) => k.name === kindName()) ?? kinds()[0]

  const filtered = createMemo(() => {
    const needle = q().trim().toLowerCase()
    return (repos() ?? []).filter((r) => !needle || r.full_name.toLowerCase().includes(needle))
  })
  const toggle = (full: string) => {
    const on = picked().includes(full)
    setPicked(on ? picked().filter((r) => r !== full) : [...picked(), full])
  }
  const matched = (first: string | undefined) => {
    if (!first) return null
    const short = first.split('/').pop()!.toLowerCase()
    const list = setups() ?? []
    return (
      list.find((s) => s.repos.some((r) => r.toLowerCase() === first.toLowerCase()))?.name ??
      list.find((s) => s.name.toLowerCase() === short)?.name ??
      null
    )
  }
  const tickRepos = (name: string | null) => {
    const s = (setups() ?? []).find((x) => x.name === name)
    if (!s) return
    const extra = s.repos.filter((r) => !picked().some((p) => p.toLowerCase() === r.toLowerCase()))
    if (extra.length) setPicked([...picked(), ...extra])
  }
  createEffect(() => {
    const first = picked()[0]
    if (setupTouched()) return
    const m = matched(first)
    setSetup(m)
    tickRepos(m)
  })
  const setupRow = () => (setups() ?? []).find((s) => s.name === setup())
  const env = createMemo(() => [...new Set([...(setupRow()?.env ?? []), ...extraEnv()])])
  const missing = () => env().filter((n) => !envNames()?.includes(n) && !values()[n]?.trim())
  const projectName = () => name() ?? picked()[0]?.split('/').pop() ?? ''

  const create = async () => {
    const p = place()
    if (!picked().length || !p || busy()) return
    setError('')
    setBusy(true)
    try {
      for (const [k, v] of Object.entries(values())) if (v.trim()) await callOn('hub', { op: 'set_env', name: k, value: v })
      await refetchEnv()
      const tools = [...(browser() ? ['browser'] : []), ...(phone() && phoneReady() ? ['phone'] : [])]
      const made = await callOn<ProjectX>('hub', {
        op: 'create_project',
        name: projectName(),
        repos: picked(),
        provider: p.key.startsWith('machine:') ? undefined : p.key,
        kind: p.key.startsWith('machine:') ? undefined : kind()?.name,
        machine: p.key.startsWith('machine:') ? p.key.slice(8) : undefined,
        setup: setup() ?? undefined,
        env: env(),
        tools,
        allow_main: allowMain(),
      })
      location.hash = `#/new/${encodeURIComponent(made.name)}`
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const openFolder = async (path: string) => {
    setError('')
    try {
      const machine = state.thisMachine!
      const existing = Object.values(state.projects).find((p) => p.default_machine === machine && p.path === path)
      let p = existing
      if (!p) {
        let base = (path.split('/').filter(Boolean).pop() ?? 'project').replace(/[^A-Za-z0-9._-]/g, '-')
        if (Object.values(state.projects).some((x) => x.name === base)) base = `${base}-${Math.random().toString(36).slice(2, 5)}`
        p = await callOn<ProjectX>('hub', { op: 'create_project', name: base, path, machine, repos: [], env: [] })
      }
      location.hash = `#/new/${encodeURIComponent(p.name)}`
    } catch (e) {
      setError((e as Error).message)
    }
  }

  return (
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto max-w-2xl px-4 pt-4 pb-16 sm:px-6 md:pt-10">
        <div class="mb-6 flex items-center gap-2">
          <a
            href="#/"
            aria-label="Back"
            class="-ml-1 flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 md:hidden"
          >
            <ArrowLeft size={18} />
          </a>
          <h1 class="text-[20px] font-semibold tracking-tight">New project</h1>
        </div>
        <Show when={state.thisMachine && hubUp()}>
          <div class="mb-5 inline-flex rounded-lg bg-ink/[0.05] p-0.5 text-[13px]">
            <button
              class="h-7 rounded-md px-3"
              classList={{
                'bg-raised font-medium shadow-[0_1px_2px_rgb(0_0_0/0.08)]': source() === 'github',
                'text-mute': source() !== 'github',
              }}
              onClick={() => setSource('github')}
            >
              From GitHub
            </button>
            <button
              class="h-7 rounded-md px-3"
              classList={{
                'bg-raised font-medium shadow-[0_1px_2px_rgb(0_0_0/0.08)]': source() === 'folder',
                'text-mute': source() !== 'folder',
              }}
              onClick={() => setSource('folder')}
            >
              A folder here
            </button>
          </div>
        </Show>
        <Show
          when={source() === 'github'}
          fallback={
            <FolderPicker machine={state.thisMachine!} onPick={(p) => void openFolder(p)} onCancel={() => (location.hash = '#/')} />
          }
        >
          <Show when={hubUp()} fallback={<p class="text-[13.5px] text-mute">Reconnecting to the hub…</p>}>
            <div class="overflow-hidden rounded-xl border border-line">
              <label class="flex h-10 items-center gap-2 border-b border-line px-3 text-mute">
                <Search size={14} />
                <input
                  class="min-w-0 flex-1 bg-transparent text-[13.5px] text-ink placeholder:text-faint"
                  placeholder="Find a repo"
                  autofocus
                  value={q()}
                  onInput={(e) => setQ(e.currentTarget.value)}
                />
                <Show when={picked().length}>
                  <span class="num shrink-0 text-[12px] text-mute">{picked().length} picked</span>
                </Show>
              </label>
              <div class="max-h-72 overflow-y-auto">
                <Show when={!repos.loading} fallback={<p class="px-3 py-3 text-[13px] text-faint">Loading your repos…</p>}>
                  <Show when={!reposError()} fallback={<p class="px-3 py-3 text-[13px] text-bad">{reposError()}</p>}>
                    <For each={filtered()} fallback={<p class="px-3 py-3 text-[13px] text-faint">No repos match.</p>}>
                      {(r) => (
                        <label class="flex cursor-pointer items-center gap-3 px-3 py-2 hover:bg-ink/[0.03]">
                          <input
                            type="checkbox"
                            class="accent-[var(--accent)]"
                            checked={picked().includes(r.full_name)}
                            onChange={() => toggle(r.full_name)}
                          />
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
                        </label>
                      )}
                    </For>
                  </Show>
                </Show>
              </div>
            </div>

            <div class="mt-6 grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-[9rem_1fr] sm:items-center">
              <span class="text-[13px] text-mute">Name</span>
              <input
                class={`${inputClass} w-full sm:max-w-xs`}
                placeholder="Pick a repo first"
                spellcheck={false}
                value={projectName()}
                onInput={(e) => setName(e.currentTarget.value)}
              />

              <span class="text-[13px] text-mute">Runs on</span>
              <div>
                <Show
                  when={places().length}
                  fallback={<span class="text-[13px] text-warn">Connect a provider in Settings first, or pair a machine.</span>}
                >
                  <Menu
                    label="Runs on"
                    class={pill}
                    items={places().map((p) => ({ label: p.label, checked: p.key === place()?.key, onSelect: () => setWhere(p.key) }))}
                  >
                    <span>{place()?.label}</span>
                    <ChevronDown size={13} class="text-faint" />
                  </Menu>
                  <Show when={kinds().length > 1}>
                    <Menu
                      label="Kind"
                      class={`${pill} ml-2`}
                      items={kinds().map((k) => ({
                        label: k.label,
                        checked: k.name === kind()?.name,
                        onSelect: () => setKindName(k.name),
                      }))}
                    >
                      <span>{kind()?.label}</span>
                      <ChevronDown size={13} class="text-faint" />
                    </Menu>
                  </Show>
                  <Show when={kind()?.about}>
                    <p class="mt-1.5 text-[12.5px] text-faint">{kind()!.about}</p>
                  </Show>
                </Show>
              </div>

              <span class="text-[13px] text-mute">Setup</span>
              <div class="flex flex-wrap items-center gap-2">
                <Menu
                  label="Setup"
                  class={pill}
                  items={[
                    { label: 'None', checked: !setup(), onSelect: () => (setSetupTouched(true), setSetup(null)) },
                    ...(setups() ?? []).map((s) => ({
                      label: s.name,
                      checked: s.name === setup(),
                      onSelect: () => (setSetupTouched(true), setSetup(s.name), tickRepos(s.name)),
                    })),
                  ]}
                >
                  <span>{setup() ?? 'None'}</span>
                  <ChevronDown size={13} class="text-faint" />
                </Menu>
                <Show when={!(setups() ?? []).length}>
                  <span class="text-[12.5px] text-faint">Setups come from your setup repo's corra/setups folder.</span>
                </Show>
              </div>

              <Show when={env().length || (envNames() ?? []).length}>
                <span class="self-start pt-1.5 text-[13px] text-mute">Env vars</span>
                <div class="space-y-1.5">
                  <For each={env()}>
                    {(n) => (
                      <div class="flex items-center gap-2">
                        <span class="w-44 shrink-0 truncate font-mono text-[12.5px]">{n}</span>
                        <Show when={!envNames()?.includes(n)} fallback={<span class="text-[12.5px] text-mute">set</span>}>
                          <input
                            class={`${inputClass} min-w-0 flex-1`}
                            type="password"
                            autocomplete="off"
                            placeholder="Its value"
                            value={values()[n] ?? ''}
                            onInput={(e) => setValues({ ...values(), [n]: e.currentTarget.value })}
                          />
                        </Show>
                      </div>
                    )}
                  </For>
                  <Show when={(envNames() ?? []).some((n) => !env().includes(n))}>
                    <Menu
                      label="Add an env var"
                      class="inline-flex h-7 items-center gap-1 rounded-md px-1.5 text-[12.5px] text-mute hover:bg-ink/5 hover:text-ink"
                      items={(envNames() ?? [])
                        .filter((n) => !env().includes(n))
                        .map((n) => ({ label: n, onSelect: () => setExtraEnv([...extraEnv(), n]) }))}
                    >
                      + Add one
                    </Menu>
                  </Show>
                </div>
              </Show>

              <span class="self-start pt-1.5 text-[13px] text-mute">Main and tags</span>
              <div>
                <div class="inline-flex flex-wrap rounded-lg bg-ink/[0.05] p-0.5 text-[13px]">
                  <For each={MAIN}>
                    {(m) => (
                      <button
                        class="h-7 rounded-md px-3"
                        classList={{
                          'bg-raised font-medium shadow-[0_1px_2px_rgb(0_0_0/0.08)]': allowMain() === m.value,
                          'text-mute': allowMain() !== m.value,
                        }}
                        onClick={() => setAllowMain(m.value)}
                      >
                        {m.label}
                      </button>
                    )}
                  </For>
                </div>
                <p class="mt-1.5 text-[12.5px] text-faint">{MAIN.find((m) => m.value === allowMain())?.hint}</p>
              </div>

              <span class="text-[13px] text-mute">Tools</span>
              <div class="flex flex-wrap gap-4 text-[13px]">
                <label class="flex items-center gap-1.5">
                  <input
                    type="checkbox"
                    class="accent-[var(--accent)]"
                    checked={browser()}
                    onChange={(e) => setBrowser(e.currentTarget.checked)}
                  />
                  Browser
                </label>
                <label class="flex items-center gap-1.5" classList={{ 'text-faint': !phoneReady() }}>
                  <input
                    type="checkbox"
                    class="accent-[var(--accent)]"
                    disabled={!phoneReady()}
                    checked={phone() && phoneReady()}
                    onChange={(e) => setPhone(e.currentTarget.checked)}
                  />
                  Android test phone
                </label>
              </div>
            </div>

            <div class="mt-8 flex items-center gap-3">
              <Btn kind="primary" disabled={!picked().length || !place() || busy() || missing().length > 0} onClick={() => void create()}>
                {busy() ? 'Creating…' : 'Create project'}
              </Btn>
              <Show when={missing().length}>
                <span class="text-[12.5px] text-mute">Give {missing().join(', ')} a value first.</span>
              </Show>
              <Show when={!missing().length && !picked().length}>
                <span class="text-[12.5px] text-mute">Pick at least one repo.</span>
              </Show>
              <Show when={!missing().length && picked().length > 0 && !place()}>
                <span class="text-[12.5px] text-mute">Connect a provider in Settings, or pair a machine, so it has somewhere to run.</span>
              </Show>
            </div>
          </Show>
        </Show>
        <p class="mt-3 text-[12.5px] text-bad">{error()}</p>
      </div>
    </div>
  )
}
