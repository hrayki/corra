import { For, Show, createMemo, createResource, createSignal } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import X from 'lucide-solid/icons/x'
import { callOn, state, type ProjectX } from '../lib/store'
import { ago } from '../lib/format'
import { kindOf, makers, paired, providerOf, sizeLabel, sizesFor } from '../lib/catalog'
import { RepoPicker } from './RepoPicker'
import { Btn, Menu } from './bits'

type SetupRow = { name: string; repos: string[]; env: string[] }

const MAIN = [
  { value: false, label: 'Wait for you', hint: 'Pushing to the default branch or a tag, and merging, wait for your yes.' },
  { value: true, label: 'On its own', hint: 'It pushes to the default branch and tags, and merges, on its own.' },
]

const pill = 'inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-raised px-2.5 text-[13px] hover:border-faint'

export function ProjectPage(props: { id: string }) {
  const p = () => state.projects[props.id] as ProjectX | undefined
  const [setups] = createResource(() => callOn<SetupRow[]>('hub', { op: 'list_setups' }).catch(() => []))
  const [envNames] = createResource(() => callOn<string[]>('hub', { op: 'list_env' }).catch(() => []))
  const [error, setError] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const run = async (f: () => Promise<unknown>) => {
    setError('')
    setBusy(true)
    try {
      await f()
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }
  const update = (fields: Record<string, unknown>) => run(() => callOn('hub', { op: 'update_project', project: props.id, ...fields }))
  const machine = createMemo(
    () =>
      Object.values(state.machines)
        .filter((m) => m.project === props.id && m.status !== 'destroyed')
        .sort((a, b) => b.created_at - a.created_at)[0],
  )
  const status = () => {
    const m = machine()
    if (!m) return 'No machine yet: the next thread makes one.'
    const word = {
      connected: 'Awake',
      asleep: 'Asleep',
      creating: 'Starting',
      offline: 'Not answering',
      failed: 'Failed',
      destroying: 'Being removed',
    }[m.status as string]
    const parts = [word ?? m.status]
    if (m.info?.disk_used_mb) parts.push(`${(m.info.disk_used_mb / 1024).toFixed(1)} GB of disk`)
    if (m.last_seen) parts.push(`seen ${ago(m.last_seen)} ago`)
    return parts.join(' · ')
  }
  const places = createMemo(() => {
    const out = makers().map((x) => ({ key: x.name, label: x.label }))
    for (const m of Object.values(state.machines))
      if (m.src === 'hub' && paired(m.provider) && m.status !== 'destroyed') out.push({ key: `machine:${m.id}`, label: m.name })
    return out
  })
  const makes = () => !!providerOf(p()?.provider)?.creates_machines
  const placeKey = () => (makes() ? p()?.provider : `machine:${p()?.default_machine}`)
  const kinds = () => providerOf(p()?.provider)?.kinds ?? []
  const kind = () => kindOf(p()?.provider, p()?.kind)
  const has = (t: string) => !!p()?.tools.includes(t)
  const setTool = (t: string, on: boolean) =>
    update({ tools: on ? [...new Set([...(p()?.tools ?? []), t])] : (p()?.tools ?? []).filter((x) => x !== t) })

  return (
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto max-w-2xl px-4 pt-4 pb-16 sm:px-6 md:pt-10">
        <Show when={p()} fallback={<p class="text-[13.5px] text-mute">This project isn't on the hub.</p>}>
          <div class="mb-8 flex items-center gap-2">
            <a
              href="#/"
              aria-label="Back"
              class="-ml-1 flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 md:hidden"
            >
              <ArrowLeft size={18} />
            </a>
            <h1 class="text-[20px] font-semibold tracking-tight">{p()!.name}</h1>
          </div>

          <div class="grid grid-cols-1 overflow-hidden rounded-xl border border-line sm:grid-cols-[10rem_1fr] [&>*]:px-4 [&>div]:border-b [&>div]:border-line [&>div]:pt-1.5 [&>div]:pb-4 [&>div:last-child]:border-b-0 [&>span]:!pt-4 [&>span]:font-medium [&>span]:text-ink sm:[&>div]:pt-3.5 sm:[&>div]:pl-0 sm:[&>span]:border-b sm:[&>span]:border-line sm:[&>span:nth-last-child(2)]:border-b-0">
            <span class="pt-1 text-[13px] text-mute">Repos</span>
            <div>
              <div class="space-y-1">
                <For each={p()!.repos}>{(r) => <div class="font-mono text-[12.5px]">{r.github ?? r.name}</div>}</For>
              </div>
              <div class="mt-2">
                <RepoPicker
                  placeholder="Add a repo"
                  exclude={p()!.repos.map((r) => r.github ?? '')}
                  onPick={(r) => void run(() => callOn('hub', { op: 'add_repo', project: props.id, github: r }))}
                />
              </div>
            </div>

            <span class="pt-1 text-[13px] text-mute">Machine</span>
            <div>
              <div class="text-[13.5px]">{status()}</div>
              <p class="mt-0.5 text-[12.5px] text-faint">
                It sleeps after 15 idle minutes, with its disk kept, and wakes with the next message.
              </p>
              <div class="mt-2 flex flex-wrap items-center gap-2">
                <Show when={places().length}>
                  <Menu
                    label="Runs on"
                    class={pill}
                    items={places().map((x) => ({
                      label: x.label,
                      checked: x.key === placeKey(),
                      onSelect: () =>
                        void update(
                          x.key.startsWith('machine:')
                            ? { provider: state.machines[x.key.slice(8)]?.provider ?? 'existing', default_machine: x.key.slice(8) }
                            : { provider: x.key },
                        ),
                    }))}
                  >
                    <span>{places().find((x) => x.key === placeKey())?.label ?? p()!.provider}</span>
                    <ChevronDown size={13} class="text-faint" />
                  </Menu>
                </Show>
                <Show when={makes() && kinds().length > 1}>
                  <Menu
                    label="Kind"
                    class={pill}
                    items={kinds().map((k) => ({
                      label: k.label,
                      checked: k.name === kind()?.name,
                      onSelect: () => void update({ kind: k.name }),
                    }))}
                  >
                    <span>{kind()?.label}</span>
                    <ChevronDown size={13} class="text-faint" />
                  </Menu>
                </Show>
                <Show when={makes()}>
                  <Menu
                    label="Size"
                    class={pill}
                    items={sizesFor(kind()).map((s) => ({
                      label: sizeLabel(s),
                      checked: s === p()!.size,
                      onSelect: () => void update({ size: s }),
                    }))}
                  >
                    <span>{sizeLabel(p()!.size)}</span>
                    <ChevronDown size={13} class="text-faint" />
                  </Menu>
                </Show>
                <Show when={machine() && makes()}>
                  <Btn
                    kind="danger"
                    disabled={busy()}
                    onClick={() =>
                      confirm(
                        'Reset the machine? Its disk is erased: anything not pushed is lost. The next thread starts on a fresh machine with the setup run again.',
                      ) && void run(() => callOn('hub', { op: 'reset_machine', project: props.id }))
                    }
                  >
                    Reset machine
                  </Btn>
                </Show>
              </div>
              <Show when={makes() && kind()?.about}>
                <p class="mt-1.5 text-[12.5px] text-faint">{kind()!.about}</p>
              </Show>
            </div>

            <span class="pt-1.5 text-[13px] text-mute">Setup</span>
            <div>
              <Menu
                label="Setup"
                class={pill}
                items={[
                  { label: 'None', checked: !p()!.setup, onSelect: () => void update({ setup: '' }) },
                  ...(setups() ?? []).map((s) => ({
                    label: s.name,
                    checked: s.name === p()!.setup,
                    onSelect: () => void update({ setup: s.name }),
                  })),
                ]}
              >
                <span>{p()!.setup ?? 'None'}</span>
                <ChevronDown size={13} class="text-faint" />
              </Menu>
              <p class="mt-1 text-[12.5px] text-faint">A change runs at the machine's next wake.</p>
            </div>

            <span class="pt-1 text-[13px] text-mute">Env vars</span>
            <div>
              <div class="flex flex-wrap gap-1.5">
                <For each={p()!.env} fallback={<span class="text-[13px] text-faint">None</span>}>
                  {(n) => (
                    <span class="inline-flex h-7 items-center gap-1 rounded-md bg-ink/[0.05] pr-1 pl-2 font-mono text-[12px]">
                      {n}
                      <Show when={!envNames()?.includes(n)}>
                        <span class="text-warn" title="No value yet: set it in Settings">
                          ·
                        </span>
                      </Show>
                      <button
                        aria-label={`Remove ${n}`}
                        class="flex size-5 items-center justify-center rounded text-faint hover:text-ink"
                        onClick={() => void update({ env: p()!.env.filter((x) => x !== n) })}
                      >
                        <X size={12} />
                      </button>
                    </span>
                  )}
                </For>
                <Show when={(envNames() ?? []).some((n) => !p()!.env.includes(n))}>
                  <Menu
                    label="Add an env var"
                    class="inline-flex h-7 items-center rounded-md px-1.5 text-[12.5px] text-mute hover:bg-ink/5 hover:text-ink"
                    items={(envNames() ?? [])
                      .filter((n) => !p()!.env.includes(n))
                      .map((n) => ({ label: n, onSelect: () => void update({ env: [...p()!.env, n] }) }))}
                  >
                    + Add one
                  </Menu>
                </Show>
              </div>
              <p class="mt-1 text-[12.5px] text-faint">Values are set once, in Settings → Env vars.</p>
            </div>

            <span class="pt-1.5 text-[13px] text-mute">Main and tags</span>
            <div>
              <div class="inline-flex flex-wrap self-start justify-self-start rounded-lg bg-ink/[0.05] p-0.5 text-[13px]">
                <For each={MAIN}>
                  {(m) => (
                    <button
                      class="h-7 rounded-md px-3"
                      classList={{
                        'bg-raised font-medium shadow-[0_1px_2px_rgb(0_0_0/0.08)]': p()!.allow_main === m.value,
                        'text-mute': p()!.allow_main !== m.value,
                      }}
                      onClick={() => void update({ allow_main: m.value })}
                    >
                      {m.label}
                    </button>
                  )}
                </For>
              </div>
              <p class="mt-1 text-[12.5px] text-faint">{MAIN.find((m) => m.value === p()!.allow_main)?.hint}</p>
            </div>

            <span class="text-[13px] text-mute">Tools</span>
            <div class="flex flex-wrap gap-4 text-[13px]">
              <label class="flex items-center gap-1.5">
                <input
                  type="checkbox"
                  class="accent-[var(--accent)]"
                  checked={has('browser')}
                  onChange={(e) => void setTool('browser', e.currentTarget.checked)}
                />
                Browser
              </label>
              <label class="flex items-center gap-1.5">
                <input
                  type="checkbox"
                  class="accent-[var(--accent)]"
                  checked={has('phone')}
                  onChange={(e) => void setTool('phone', e.currentTarget.checked)}
                />
                Android test phone
              </label>
            </div>

            <span class="text-[13px] text-mute">Manager</span>
            <div>
              <label class="flex items-center gap-1.5 text-[13px]">
                <input
                  type="checkbox"
                  class="accent-[var(--accent)]"
                  checked={p()!.trusted}
                  onChange={(e) => {
                    const on = e.currentTarget.checked
                    if (
                      on &&
                      !confirm("Give this project's agents your real keys, every repo with nothing held back, and control of all of corra?")
                    ) {
                      e.currentTarget.checked = false
                      return
                    }
                    void update({ trusted: on })
                  }}
                />
                Make this project a manager
              </label>
              <p class="mt-1 text-[12.5px] text-faint">
                Its agents get your keys as the usual variables (GitHub, Modal, DigitalOcean, Agent37, Railway, Cloudflare), reach and push
                any repo with nothing held back, and manage every project, thread, machine and credential.
              </p>
            </div>
          </div>
          <p class="mt-4 text-[12.5px] text-bad">{error()}</p>
        </Show>
      </div>
    </div>
  )
}
