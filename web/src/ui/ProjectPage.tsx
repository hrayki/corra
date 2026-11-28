import { For, Show, createEffect, createMemo, createResource, createSignal } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import ChevronRight from 'lucide-solid/icons/chevron-right'
import Play from 'lucide-solid/icons/play'
import Moon from 'lucide-solid/icons/moon'
import RotateCw from 'lucide-solid/icons/rotate-cw'
import Trash from 'lucide-solid/icons/trash'
import X from 'lucide-solid/icons/x'
import { call, state } from '../lib/store'
import type { Project } from '../proto/Project'
import type { SetupLog } from '../proto/SetupLog'
import { ago } from '../lib/format'
import { kindOf, providerOf, sizeLabel, sizesFor } from '../lib/catalog'
import { RepoPicker } from './RepoPicker'
import { Btn, Menu } from './bits'
import { MainAndTagsPicker, PlaceAndKindPicker, SetupPicker, formPill, projectPlaces, type SetupRow } from './ProjectForm'

type RuntimeInfo = {
  stage?: string
  setup?: SetupLog
  env_injected: string[]
  manager_env: string[]
  tools: string[]
}

export function ProjectPage(props: { id: string }) {
  const p = () => state.projects[props.id] as Project | undefined
  const [setups] = createResource(() => call<SetupRow[]>({ op: 'list_setups' }).catch(() => []))
  const [envNames] = createResource(() => call<string[]>({ op: 'list_env' }).catch(() => []))
  const [runtime, { refetch: refetchRuntime }] = createResource(
    () => (p() ? [props.id, p()!.trusted, p()!.env.join(',')] : null),
    () => call<RuntimeInfo>({ op: 'project_setup_log', project: props.id }).catch(() => undefined),
  )
  const [error, setError] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const [confirmReset, setConfirmReset] = createSignal(false)
  const [confirmManager, setConfirmManager] = createSignal(false)
  const [confirmDelete, setConfirmDelete] = createSignal(false)
  const [logOpen, setLogOpen] = createSignal(false)

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
  const update = (fields: Record<string, unknown>) =>
    run(async () => {
      await call({ op: 'update_project', project: props.id, ...fields })
      void refetchRuntime()
    })
  const machine = createMemo(
    () =>
      Object.values(state.machines)
        .filter((m) => m.project === props.id && m.status !== 'destroyed')
        .sort((a, b) => b.created_at - a.created_at)[0],
  )
  const threadCount = createMemo(() => Object.values(state.threads).filter((t) => t.project === props.id).length)
  const liveStatus = () => state.projectStatus[props.id]
  const stage = () => liveStatus()?.stage ?? runtime()?.stage
  const setupLog = () => liveStatus()?.setup ?? runtime()?.setup
  createEffect(() => {
    if (setupLog()?.ok === false) setLogOpen(true)
  })

  const status = () => {
    const m = machine()
    if (!m) return 'No machine yet: press Wake or start a thread to make one.'
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
  const places = createMemo(() => projectPlaces())
  const makes = () => !!providerOf(p()?.provider)?.creates_machines
  const placeKey = () => (makes() ? p()?.provider : `machine:${p()?.default_machine}`)
  const kinds = () => providerOf(p()?.provider)?.kinds ?? []
  const kind = () => kindOf(p()?.provider, p()?.kind)
  const canWake = () => makes() && (!machine() || machine()!.status === 'asleep' || machine()!.status === 'failed')
  const canSleep = () => makes() && machine()?.status === 'connected'

  const deleteProject = () =>
    run(async () => {
      await call({ op: 'delete_project', project: props.id })
      location.hash = '#/'
    })

  return (
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto max-w-2xl px-4 pt-4 pb-16 sm:px-6 md:pt-10">
        <Show when={p()} fallback={<p class="text-[13.5px] text-mute">This project isn't on the hub.</p>}>
          <div class="mb-6 flex items-center gap-2">
            <a
              href="#/"
              aria-label="Back"
              class="-ml-1 flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 md:hidden"
            >
              <ArrowLeft size={18} />
            </a>
            <h1 class="text-[20px] font-semibold tracking-tight">{p()!.name}</h1>
          </div>

          <Show when={stage()}>
            <div class="mb-4 flex items-center gap-2.5 rounded-xl border border-line bg-panel px-4 py-2.5 text-[13px] text-ink">
              <span class="size-2 shrink-0 rounded-full bg-accent pulse" />
              <span>{stage()}</span>
            </div>
          </Show>

          <div class="grid grid-cols-1 overflow-hidden rounded-xl border border-line sm:grid-cols-[10rem_1fr] [&>*]:px-4 [&>div]:border-b [&>div]:border-line [&>div]:pt-1.5 [&>div]:pb-4 [&>div:last-child]:border-b-0 [&>span]:!pt-4 [&>span]:font-medium [&>span]:text-ink sm:[&>div]:pt-3.5 sm:[&>div]:pl-0 sm:[&>span]:border-b sm:[&>span]:border-line sm:[&>span:nth-last-child(2)]:border-b-0">
            <span class="pt-1 text-[13px] text-mute">Repos</span>
            <div>
              <div class="space-y-1">
                <For each={p()!.repos} fallback={<span class="text-[13px] text-faint">None</span>}>
                  {(r) => (
                    <div class="flex items-center gap-2 font-mono text-[12.5px]">
                      <span class="min-w-0 truncate">{r.github ?? r.name}</span>
                      <button
                        aria-label={`Remove ${r.github ?? r.name}`}
                        title="Remove repo"
                        disabled={busy()}
                        class="flex size-5 shrink-0 items-center justify-center rounded text-faint hover:bg-ink/5 hover:text-ink"
                        onClick={() => void run(() => call({ op: 'remove_repo', project: props.id, name: r.name }))}
                      >
                        <X size={12} />
                      </button>
                    </div>
                  )}
                </For>
              </div>
              <div class="mt-2">
                <RepoPicker
                  placeholder="Add a repo"
                  exclude={p()!.repos.map((r) => r.github ?? '')}
                  onPick={(r) => void run(() => call({ op: 'add_repo', project: props.id, github: r }))}
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
                <PlaceAndKindPicker
                  places={places()}
                  selectedPlace={placeKey()}
                  fallbackLabel={p()!.provider}
                  onSelectPlace={(key) =>
                    void update(
                      key.startsWith('machine:')
                        ? { provider: state.machines[key.slice(8)]?.provider ?? 'existing', default_machine: key.slice(8) }
                        : { provider: key },
                    )
                  }
                  kinds={makes() ? kinds() : []}
                  selectedKind={kind()}
                  onSelectKind={(k) => void update({ kind: k })}
                />
                <Show when={makes()}>
                  <Menu
                    label="Size"
                    class={formPill}
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
                <Show when={canWake()}>
                  <Btn
                    kind="ghost"
                    disabled={busy() || !!stage()}
                    onClick={() => void run(() => call({ op: 'wake_project', project: props.id }))}
                  >
                    <Play size={13} />
                    Wake
                  </Btn>
                </Show>
                <Show when={canSleep()}>
                  <Btn
                    kind="ghost"
                    disabled={busy() || !!stage()}
                    onClick={() => void run(() => call({ op: 'sleep_project', project: props.id }))}
                  >
                    <Moon size={13} />
                    Sleep
                  </Btn>
                </Show>
                <Show when={machine() && makes() && !confirmReset()}>
                  <Btn kind="danger" disabled={busy() || !!stage()} onClick={() => setConfirmReset(true)}>
                    Reset machine
                  </Btn>
                </Show>
              </div>
              <Show when={confirmReset()}>
                <div class="mt-2.5 rounded-lg border border-bad/30 bg-bad/5 p-3 text-[12.5px]">
                  <p class="text-ink">
                    Reset the machine? Its disk is erased: anything not pushed is lost. The next wake starts fresh with setup run again.
                  </p>
                  <div class="mt-2 flex gap-2">
                    <Btn
                      kind="danger"
                      disabled={busy()}
                      onClick={() =>
                        void run(async () => {
                          await call({ op: 'reset_machine', project: props.id })
                          setConfirmReset(false)
                        })
                      }
                    >
                      Confirm reset
                    </Btn>
                    <Btn kind="ghost" disabled={busy()} onClick={() => setConfirmReset(false)}>
                      Cancel
                    </Btn>
                  </div>
                </div>
              </Show>
              <Show when={makes() && kind()?.about}>
                <p class="mt-1.5 text-[12.5px] text-faint">{kind()!.about}</p>
              </Show>
            </div>

            <span class="pt-1.5 text-[13px] text-mute">Setup</span>
            <div>
              <div class="flex flex-wrap items-center gap-2">
                <SetupPicker setups={setups() ?? []} value={p()!.setup} onChange={(name) => void update({ setup: name ?? '' })} />
                <Btn
                  kind="ghost"
                  disabled={busy() || !!stage()}
                  onClick={() =>
                    void run(async () => {
                      await call({ op: 'rerun_setup', project: props.id })
                      setLogOpen(true)
                    })
                  }
                >
                  <RotateCw size={13} />
                  Re-run setup
                </Btn>
              </div>
              <p class="mt-1 text-[12.5px] text-faint">A change runs at the machine's next wake, or immediately with Re-run setup.</p>
              <Show when={setupLog()}>
                {(s) => (
                  <div class="mt-2.5 rounded-lg border border-line bg-panel">
                    <button
                      type="button"
                      class="flex w-full items-center gap-2 px-3 py-2 text-left text-[12.5px] hover:bg-ink/[0.03]"
                      onClick={() => setLogOpen(!logOpen())}
                    >
                      <ChevronRight
                        size={13}
                        class="shrink-0 text-faint transition-transform duration-100"
                        classList={{ 'rotate-90': logOpen() }}
                      />
                      <span class="font-medium" classList={{ 'text-ok': s().ok, 'text-bad': !s().ok }}>
                        {s().ok ? 'Last setup succeeded' : 'Last setup failed'}
                      </span>
                      <span class="text-faint">·</span>
                      <span class="text-mute">{ago(s().at)} ago</span>
                      <Show when={s().setup}>
                        <span class="font-mono text-[11.5px] text-faint">({s().setup})</span>
                      </Show>
                    </button>
                    <Show when={logOpen()}>
                      <pre class="max-h-64 overflow-auto border-t border-line px-3 py-2 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap text-mute">
                        {s().log.trim() || 'No output recorded.'}
                      </pre>
                    </Show>
                  </div>
                )}
              </Show>
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
              <p class="mt-1 text-[12.5px] text-faint">Values are set once, in Settings → Secrets & Setup.</p>
            </div>

            <span class="pt-1.5 text-[13px] text-mute">Main and tags</span>
            <MainAndTagsPicker value={p()!.allow_main} onChange={(allow_main) => void update({ allow_main })} />

            <span class="text-[13px] text-mute">Manager</span>
            <div>
              <label class="flex items-center gap-1.5 text-[13px]">
                <input
                  type="checkbox"
                  class="accent-[var(--accent)]"
                  checked={p()!.trusted}
                  onChange={(e) => {
                    const on = e.currentTarget.checked
                    if (on) {
                      e.currentTarget.checked = false
                      setConfirmManager(true)
                      return
                    }
                    setConfirmManager(false)
                    void update({ trusted: false })
                  }}
                />
                Make this project a manager
              </label>
              <Show when={confirmManager()}>
                <div class="mt-2 rounded-lg border border-warn/30 bg-warn/5 p-3 text-[12.5px]">
                  <p class="text-ink">
                    Give this project's agents your real hub keys, push access to every repo without asking, and full control over projects,
                    threads, machines, and credentials?
                  </p>
                  <div class="mt-2 flex gap-2">
                    <Btn
                      kind="primary"
                      disabled={busy()}
                      onClick={() =>
                        void run(async () => {
                          await update({ trusted: true })
                          setConfirmManager(false)
                        })
                      }
                    >
                      Enable manager
                    </Btn>
                    <Btn kind="ghost" disabled={busy()} onClick={() => setConfirmManager(false)}>
                      Cancel
                    </Btn>
                  </div>
                </div>
              </Show>
              <p class="mt-1 text-[12.5px] text-faint">
                Its agents get your keys as the usual variables (GitHub, Modal, Railway, Cloudflare), reach and push any repo with nothing
                held back, and manage every project, thread, machine and credential.
              </p>
              <Show when={runtime()}>
                {(info) => (
                  <div class="mt-2.5 space-y-1.5 rounded-lg border border-line bg-panel px-3 py-2 text-[12px]">
                    <div class="flex flex-wrap items-center gap-1.5">
                      <span class="text-mute">Tools:</span>
                      <For each={info().tools}>
                        {(t) => <span class="rounded bg-ink/[0.06] px-1.5 py-0.5 font-mono text-[11.5px]">{t}</span>}
                      </For>
                    </div>
                    <Show when={p()!.trusted}>
                      <div class="flex flex-wrap items-center gap-1.5">
                        <span class="text-mute">Manager keys injected:</span>
                        <For each={info().manager_env} fallback={<span class="text-faint">None configured in Settings</span>}>
                          {(k) => <span class="rounded bg-ink/[0.06] px-1.5 py-0.5 font-mono text-[11.5px]">{k}</span>}
                        </For>
                      </div>
                    </Show>
                  </div>
                )}
              </Show>
            </div>
          </div>

          <div class="mt-6 rounded-xl border border-line p-4">
            <div class="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div class="text-[13.5px] font-medium text-ink">Delete project</div>
                <p class="text-[12.5px] text-faint">
                  Removes this project, its cloud machine, and {threadCount() === 1 ? 'its 1 thread' : `all ${threadCount()} threads`}.
                </p>
              </div>
              <Show when={!confirmDelete()}>
                <Btn kind="danger" disabled={busy()} onClick={() => setConfirmDelete(true)}>
                  <Trash size={13} />
                  Delete project
                </Btn>
              </Show>
            </div>
            <Show when={confirmDelete()}>
              <div class="mt-3 rounded-lg border border-bad/30 bg-bad/5 p-3 text-[12.5px]">
                <p class="text-ink">
                  Delete <span class="font-medium">{p()!.name}</span> permanently? Anything unpushed on its cloud machine and all its thread
                  history will be deleted.
                </p>
                <div class="mt-2 flex gap-2">
                  <Btn kind="danger" disabled={busy()} onClick={() => void deleteProject()}>
                    Confirm delete
                  </Btn>
                  <Btn kind="ghost" disabled={busy()} onClick={() => setConfirmDelete(false)}>
                    Cancel
                  </Btn>
                </div>
              </div>
            </Show>
          </div>

          <p class="mt-4 text-[12.5px] text-bad">{error()}</p>
        </Show>
      </div>
    </div>
  )
}
