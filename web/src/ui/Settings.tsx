import { For, Show, createEffect, createMemo, createResource, createRoot, createSignal, on, onCleanup, type JSX } from 'solid-js'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import Laptop from 'lucide-solid/icons/laptop'
import Cloud from 'lucide-solid/icons/cloud'
import Bell from 'lucide-solid/icons/bell'
import Server from 'lucide-solid/icons/server'
import History from 'lucide-solid/icons/rotate-ccw-clock'
import TriangleAlert from 'lucide-solid/icons/triangle-exclamation-point'
import ArrowUpRight from 'lucide-solid/icons/arrow-up-right'
import CircleArrowUp from 'lucide-solid/icons/circle-arrow-up'
import { call, state } from '../lib/store'
import { newPairingCode, signOut } from '../lib/auth'
import { renderSVG } from 'uqr'
import { choice, setChoice, type ThemeChoice } from '../lib/theme'
import { ago } from '../lib/format'
import { runningLine, when } from '../lib/hubversion'
import { Btn, Menu, inputClass } from './bits'
import { RepoPicker } from './RepoPicker'
import { pushState, turnOff, turnOn, type PushState } from '../lib/notify'
import { paired, providerLabel, providerOf, providers, refreshCatalog, tools } from '../lib/catalog'
import type { KindInfo } from '../proto/KindInfo'
import type { Machine } from '../proto/Machine'
import type { ProviderInfo } from '../proto/ProviderInfo'
import type { HubVersion } from '../proto/HubVersion'

export function Settings() {
  const hubUp = () => state.conn === 'open'
  const pages = createMemo(() => {
    const all: { key: string; label: string; show: boolean }[] = [
      { key: 'general', label: 'General', show: true },
      { key: 'providers', label: 'Providers', show: hubUp() },
      { key: 'tools', label: 'Tools', show: hubUp() },
      { key: 'keys', label: 'Keys', show: hubUp() },
      { key: 'machines', label: 'Machines', show: true },
      { key: 'env', label: 'Env vars', show: hubUp() },
      { key: 'setup', label: 'Setup repo', show: hubUp() },
      { key: 'data', label: 'Data', show: hubUp() },
      { key: 'devices', label: 'Devices', show: true },
    ]
    return all.filter((p) => p.show)
  })
  const [page, setPageRaw] = createSignal(localStorage.getItem('corra_settings_page') ?? 'general')
  const setPage = (k: string) => {
    setPageRaw(k)
    localStorage.setItem('corra_settings_page', k)
  }
  const on = () => (pages().some((p) => p.key === page()) ? page() : 'general')
  return (
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto flex max-w-4xl gap-10 px-4 pt-4 pb-16 sm:px-6 md:pt-10">
        <nav class="sticky top-10 hidden w-40 shrink-0 self-start md:block" aria-label="Settings">
          <h1 class="mb-4 px-2.5 text-[20px] font-semibold tracking-tight">Settings</h1>
          <For each={pages()}>
            {(p) => (
              <button
                class="flex h-8 w-full items-center rounded-lg px-2.5 text-left text-[13px]"
                classList={{
                  'bg-ink/[0.06] font-medium text-ink': on() === p.key,
                  'text-mute hover:bg-ink/[0.04] hover:text-ink': on() !== p.key,
                }}
                aria-current={on() === p.key ? 'page' : undefined}
                onClick={() => setPage(p.key)}
              >
                {p.label}
              </button>
            )}
          </For>
        </nav>
        <div class="min-w-0 flex-1 md:max-w-2xl md:pt-[46px]">
          <div class="mb-4 flex items-center gap-2 md:hidden">
            <a href="#/" aria-label="Back" class="-ml-1 flex size-9 items-center justify-center rounded-lg text-mute hover:bg-ink/5">
              <ArrowLeft size={18} />
            </a>
            <h1 class="text-[20px] font-semibold tracking-tight">Settings</h1>
          </div>
          <div class="-mx-4 mb-6 flex gap-1 overflow-x-auto px-4 md:hidden">
            <For each={pages()}>
              {(p) => (
                <button
                  class="h-9 shrink-0 rounded-lg px-3 text-[13px] whitespace-nowrap"
                  classList={{ 'bg-ink/[0.07] font-medium text-ink': on() === p.key, 'text-mute': on() !== p.key }}
                  onClick={() => setPage(p.key)}
                >
                  {p.label}
                </button>
              )}
            </For>
          </div>
          <div class="space-y-10">
            <Show when={on() === 'general'}>
              <Appearance />
              <Show when={hubUp()}>
                <Notifications />
                <Compaction />
                <Version />
              </Show>
            </Show>
            <Show when={on() === 'machines'}>
              <Machines />
            </Show>
            <Show when={on() === 'providers'}>
              <Providers />
            </Show>
            <Show when={on() === 'tools'}>
              <Tools />
            </Show>
            <Show when={on() === 'keys'}>
              <Keys />
            </Show>
            <Show when={on() === 'env'}>
              <EnvVars />
            </Show>
            <Show when={on() === 'setup'}>
              <MachineSetup />
            </Show>
            <Show when={on() === 'data'}>
              <Data />
              <Trash />
            </Show>
            <Show when={on() === 'devices'}>
              <Devices />
            </Show>
          </div>
        </div>
      </div>
    </div>
  )
}

function Section(props: { title: string; hint?: string; children: JSX.Element }) {
  return (
    <section>
      <h2 class="text-[14px] font-semibold">{props.title}</h2>
      <Show when={props.hint}>
        <p class="mt-0.5 text-[13px] text-mute">{props.hint}</p>
      </Show>
      <div class="mt-3">{props.children}</div>
    </section>
  )
}

function Card(props: { children: JSX.Element }) {
  return <div class="divide-y divide-line overflow-hidden rounded-xl border border-line">{props.children}</div>
}

function Appearance() {
  const options: { value: ThemeChoice; label: string }[] = [
    { value: 'auto', label: 'Auto' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
  ]
  return (
    <Section title="Appearance">
      <div class="inline-flex rounded-lg border border-line p-0.5">
        <For each={options}>
          {(o) => (
            <button
              class="h-7 rounded-md px-3 text-[13px]"
              classList={{ 'bg-ink text-bg font-medium': choice() === o.value, 'text-mute hover:text-ink': choice() !== o.value }}
              aria-pressed={choice() === o.value}
              onClick={() => setChoice(o.value)}
            >
              {o.label}
            </button>
          )}
        </For>
      </div>
    </Section>
  )
}

function Compaction() {
  const [settings, { refetch }] = createResource(() => call<Record<string, unknown>>({ op: 'get_settings' }))
  const [over, setOver] = createSignal<string | null>(null)
  const [error, setError] = createSignal('')
  const enabled = () => settings()?.compact_idle !== 'never' && settings()?.compact_idle !== false
  const shown = () => over() ?? Number(settings()?.compact_over ?? 200000).toLocaleString('en-US')
  const setEnabled = async (on: boolean) => {
    await call({ op: 'set_settings', settings: { compact_idle: on ? 'before' : 'never' } })
    await refetch()
  }
  const save = async () => {
    const v = shown().replace(/[\s,_]/g, '')
    if (!/^[1-9][0-9]{3,}$/.test(v)) {
      setError('A number of tokens, like 200000.')
      return
    }
    setError('')
    await call({ op: 'set_settings', settings: { compact_over: v } })
    setOver(null)
    await refetch()
  }
  const options = [
    { value: true, label: 'Before the cache expires' },
    { value: false, label: 'Never' },
  ]
  return (
    <Section
      title="Compact idle threads"
      hint="Claude's prompt cache lasts about an hour on a subscription. Compacting an idle long thread shortly before its cache expires keeps the summary cheap."
    >
      <div class="inline-flex max-w-full flex-wrap rounded-lg border border-line p-0.5">
        <For each={options}>
          {(o) => (
            <button
              class="h-7 rounded-md px-3 text-[13px]"
              classList={{ 'bg-ink text-bg font-medium': enabled() === o.value, 'text-mute hover:text-ink': enabled() !== o.value }}
              aria-pressed={enabled() === o.value}
              onClick={() => void setEnabled(o.value)}
            >
              {o.label}
            </button>
          )}
        </For>
      </div>
      <p class="mt-2 text-[12.5px] text-mute">
        <Show when={enabled()} fallback="Threads keep their full history. You can still send /compact yourself.">
          Corra sends /compact to an idle long thread shortly before its cache expires, while the summary is cheap.
        </Show>
      </p>
      <Show when={enabled()}>
        <div class="mt-3 flex flex-wrap items-center gap-2 text-[13px]">
          <span>For threads over</span>
          <input
            class={`${inputClass} w-28 text-right tabular-nums`}
            inputmode="numeric"
            value={shown()}
            onInput={(e) => setOver(e.currentTarget.value)}
            onKeyDown={(e) => e.key === 'Enter' && void save()}
          />
          <span class="text-mute">tokens</span>
          <Btn small disabled={over() === null} onClick={() => void save()}>
            Save
          </Btn>
        </div>
        <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
      </Show>
    </Section>
  )
}

const KINDS = [
  { key: 'needs', label: 'Needs you' },
  { key: 'done', label: 'Finished' },
  { key: 'failed', label: 'Failed' },
]

function Notifications() {
  const [settings, { refetch }] = createResource(() => call<Record<string, unknown>>({ op: 'get_settings' }))
  const [push, { refetch: recheck }] = createResource<PushState>(() => pushState())
  const [busy, setBusy] = createSignal(false)
  const [note, setNote] = createSignal('')
  const kinds = () => (settings()?.notify ?? {}) as Record<string, boolean>
  const setKind = async (k: string, on: boolean) => {
    await call({ op: 'set_settings', settings: { notify: { ...kinds(), [k]: on } } })
    await refetch()
  }
  const act = async (f: () => Promise<unknown>) => {
    setBusy(true)
    setNote('')
    try {
      await f()
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
      await recheck()
    }
  }
  const status = () => {
    switch (push()) {
      case 'on':
        return 'On'
      case 'blocked':
        return "Blocked in this browser's site settings"
      case 'unsupported':
        return "This browser can't receive them"
      case 'off':
        return 'Off'
      default:
        return ''
    }
  }
  return (
    <Section title="Notifications" hint="When a thread needs you, finishes or fails while corra isn't in front of you.">
      <Card>
        <div class="flex items-center gap-3 px-3.5 py-3">
          <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-ink/[0.05] text-mute">
            <Bell size={15} />
          </span>
          <div class="min-w-0 flex-1">
            <div class="truncate text-[13.5px] font-medium">This browser</div>
            <div class="text-[12.5px] text-mute">{status()}</div>
          </div>
          <Show when={push() === 'off'}>
            <Btn small disabled={busy()} onClick={() => void act(turnOn)}>
              Turn on
            </Btn>
          </Show>
          <Show when={push() === 'on'}>
            <Btn small kind="ghost" disabled={busy()} onClick={() => void act(() => call({ op: 'push_test' }))}>
              Send a test
            </Btn>
          </Show>
          <Show when={push() === 'on'}>
            <Btn small disabled={busy()} onClick={() => void act(turnOff)}>
              Turn off
            </Btn>
          </Show>
        </div>
        <div class="flex flex-wrap items-center gap-x-5 gap-y-2 px-3.5 py-3 text-[13px]">
          <For each={KINDS}>
            {(k) => (
              <label class="flex items-center gap-1.5">
                <input
                  type="checkbox"
                  class="accent-[var(--accent)]"
                  checked={kinds()[k.key] ?? true}
                  onChange={(e) => void setKind(k.key, e.currentTarget.checked)}
                />
                {k.label}
              </label>
            )}
          </For>
        </div>
      </Card>
      <Show when={note()}>
        <p class="mt-2 text-[12.5px] text-bad">{note()}</p>
      </Show>
    </Section>
  )
}

function Machines() {
  const list = createMemo(() =>
    Object.values(state.machines)
      .filter((m) => m.status !== 'destroyed')
      .sort((a, b) => Number(b.status === 'connected') - Number(a.status === 'connected') || b.created_at - a.created_at),
  )
  const gone = (m: Machine) => providers().length > 0 && !providerOf(m.provider)
  const detail = (m: Machine) => {
    const parts: string[] = []
    if (gone(m)) return `From ${m.provider}, no longer in corra`
    parts.push(paired(m.provider) ? 'Paired' : providerLabel(m.provider))
    if (m.project && state.projects[m.project]) parts.push(state.projects[m.project].name)
    if (m.info?.disk_used_mb) parts.push(`${(m.info.disk_used_mb / 1024).toFixed(1)} GB of disk`)
    parts.push(m.status === 'connected' ? 'awake' : m.status === 'offline' && m.last_seen ? `seen ${ago(m.last_seen)} ago` : m.status)
    return parts.join(' · ')
  }
  return (
    <Section
      title="Machines"
      hint={
        state.conn === 'open'
          ? 'Machines corra makes sleep after 15 idle minutes, with their disk kept, and wake with the next message.'
          : undefined
      }
    >
      <Card>
        <For each={list()} fallback={<p class="px-3.5 py-3 text-[13px] text-mute">No machines yet.</p>}>
          {(m) => (
            <div class="flex items-center gap-3 px-3.5 py-2.5">
              <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-ink/[0.05] text-mute">
                {paired(m.provider) ? <Laptop size={15} /> : <Cloud size={15} />}
              </span>
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2 text-[13.5px] font-medium">
                  <span class="truncate">{m.name}</span>
                  <span
                    class="size-1.5 shrink-0 rounded-full"
                    classList={{
                      'bg-ok': m.status === 'connected',
                      'bg-accent pulse': m.status === 'creating' || m.status === 'destroying',
                      'bg-faint': m.status === 'offline' || m.status === 'asleep',
                      'bg-bad': m.status === 'failed',
                    }}
                  />
                </div>
                <div class="truncate text-[12.5px] text-mute">{detail(m)}</div>
              </div>
              <Show when={gone(m)}>
                <Btn
                  small
                  kind="ghost"
                  title="Removes it from corra. Anything it left on its provider stays there."
                  onClick={() => void call({ op: 'destroy_machine', machine: m.id })}
                >
                  Forget
                </Btn>
              </Show>
            </div>
          )}
        </For>
      </Card>
    </Section>
  )
}

const [secretNames, secretsRes] = createRoot(() =>
  createResource(
    () => state.conn === 'open',
    (up) => (up ? call<string[]>({ op: 'list_secrets' }) : Promise.resolve([] as string[])),
  ),
)
const hasSecret = (n: string) => !!secretNames()?.includes(n)
const refreshKeys = () => {
  void secretsRes.refetch()
  refreshCatalog()
}

function KeyRow(props: { name: string; label: string; hint?: string; inset?: boolean }) {
  const [open, setOpen] = createSignal(false)
  const [value, setValue] = createSignal('')
  const [error, setError] = createSignal('')
  const save = async () => {
    setError('')
    try {
      await call({ op: 'set_secret', name: props.name, value: value() })
      setValue('')
      setOpen(false)
      refreshKeys()
    } catch (e) {
      setError((e as Error).message)
    }
  }
  return (
    <div classList={{ 'px-3.5 py-3': !props.inset, 'py-2': props.inset }}>
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-baseline gap-x-2">
            <span class="text-[13px] font-medium">{props.label}</span>
            <span class="text-[12px]" classList={{ 'text-ok': hasSecret(props.name), 'text-faint': !hasSecret(props.name) }}>
              {hasSecret(props.name) ? 'Saved' : 'Not set'}
            </span>
          </div>
          <Show when={props.hint}>
            <div class="mt-0.5 text-[12.5px] text-mute">{props.hint}</div>
          </Show>
        </div>
        <Btn small onClick={() => (setOpen(!open()), setValue(''))}>
          {hasSecret(props.name) ? 'Replace' : 'Add'}
        </Btn>
        <Show when={hasSecret(props.name)}>
          <Menu
            label={`More for ${props.label}`}
            align="end"
            class="flex size-8 items-center justify-center rounded-lg text-faint hover:bg-ink/5 hover:text-ink max-md:size-9"
            items={[
              {
                label: 'Remove',
                danger: true,
                onSelect: () => confirm(`Remove ${props.label}?`) && void call({ op: 'delete_secret', name: props.name }).then(refreshKeys),
              },
            ]}
          >
            <span class="text-[15px] leading-none">⋯</span>
          </Menu>
        </Show>
      </div>
      <Show when={open()}>
        <div class="mt-2 flex gap-2">
          <input
            class={`${inputClass} flex-1`}
            type="password"
            autocomplete="off"
            autofocus
            placeholder="Paste the value"
            value={value()}
            onInput={(e) => setValue(e.currentTarget.value)}
            onKeyDown={(e) => e.key === 'Enter' && value() && void save()}
          />
          <Btn kind="primary" disabled={!value()} onClick={() => void save()}>
            Save
          </Btn>
        </div>
      </Show>
      <Show when={error()}>
        <p class="mt-1.5 text-[12.5px] text-bad">{error()}</p>
      </Show>
    </div>
  )
}

function Status(props: { ok: boolean; text: string }) {
  return (
    <span class="inline-flex items-center gap-1.5 text-[12px]" classList={{ 'text-ok': props.ok, 'text-mute': !props.ok }}>
      <span class="size-1.5 rounded-full" classList={{ 'bg-ok': props.ok, 'bg-faint': !props.ok }} />
      {props.text}
    </span>
  )
}

const range = (xs: number[]) => {
  const lo = Math.min(...xs)
  const hi = Math.max(...xs)
  return lo === hi ? `${lo}` : `${lo}–${hi}`
}

function kindLine(k: KindInfo) {
  const parts = [k.sizes.length ? `${range(k.sizes.map((s) => s.cpu))} cores, ${range(k.sizes.map((s) => s.memory_gb))} GB` : 'Any size']
  parts.push(k.kvm ? 'Has KVM' : 'No KVM')
  if (k.max_hours) parts.push(`${k.max_hours} hours at most`)
  return parts.join(' · ')
}

function ProviderRow(props: { p: ProviderInfo }) {
  const [open, setOpen] = createSignal<boolean>(false)
  return (
    <div>
      <button
        class="flex w-full items-start gap-3 px-3.5 py-3 text-left hover:bg-ink/[0.025]"
        onClick={() => setOpen(!open())}
        aria-expanded={open()}
      >
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-center gap-x-2.5 gap-y-0.5">
            <span class="text-[13.5px] font-medium">{props.p.label}</span>
            <Status ok={props.p.connected} text={props.p.connected ? 'Connected' : 'Not connected'} />
          </div>
          <div class="mt-0.5 text-[12.5px] text-mute">{props.p.about}</div>
        </div>
        <ChevronDown size={15} class="mt-1 shrink-0 text-faint transition-transform" classList={{ 'rotate-180': open() }} />
      </button>
      <Show when={open()}>
        <div class="space-y-3 px-3.5 pb-3.5">
          <div class="divide-y divide-line">
            <For each={props.p.credentials}>{(c) => <KeyRow inset name={c.name} label={c.label} hint={c.hint} />}</For>
          </div>
          <div>
            <div class="mb-1 text-[12px] font-medium text-mute">{props.p.kinds.length > 1 ? 'Kinds of machine' : 'Machines'}</div>
            <div class="divide-y divide-line rounded-lg border border-line">
              <For each={props.p.kinds}>
                {(k) => (
                  <div class="px-3 py-2">
                    <div class="flex flex-wrap items-baseline gap-x-2">
                      <span class="text-[13px] font-medium">{k.label}</span>
                      <span class="text-[12px] text-faint">{kindLine(k)}</span>
                    </div>
                    <div class="text-[12.5px] text-mute">{k.about}</div>
                    <div class="text-[12.5px] text-mute">Asleep: {k.sleeps}</div>
                  </div>
                )}
              </For>
            </div>
          </div>
        </div>
      </Show>
    </div>
  )
}

function Providers() {
  const list = () => providers().filter((p) => p.creates_machines)
  return (
    <Section title="Providers" hint="Where corra's machines come from. A project picks one and a size on its own page.">
      <Card>
        <For each={list()}>{(p) => <ProviderRow p={p} />}</For>
      </Card>
      <p class="mt-2 text-[12.5px] text-mute">Any Linux computer can also be a machine: pair it under Machines.</p>
    </Section>
  )
}

function Tools() {
  return (
    <Section title="Tools" hint="What agents can use besides their own tools.">
      <Card>
        <For each={tools()}>
          {(t) => (
            <div class="px-3.5 py-3">
              <div class="flex flex-wrap items-center gap-x-2.5 gap-y-0.5">
                <span class="text-[13.5px] font-medium">{t.label}</span>
                <Status ok={t.ready} text={t.ready ? 'Ready' : (t.missing ?? 'Not ready')} />
              </div>
              <div class="mt-0.5 text-[12.5px] text-mute">{t.about}</div>
              <Show when={t.name === 'github'}>
                <div class="mt-1 border-t border-line">
                  <KeyRow
                    inset
                    name="github.token"
                    label="GitHub token"
                    hint="Fine-grained, for your repos: Contents, Pull requests and Issues read and write, Actions and Checks read, no Workflows."
                  />
                </div>
              </Show>
            </div>
          )}
        </For>
      </Card>
    </Section>
  )
}

function Keys() {
  const known = new Set([
    'claude.token',
    'anthropic.api_key',
    'gemini.api_key',
    'railway.token',
    'cloudflare.token',
    'cloudflare.account_id',
    'github.token',
  ])
  const others = () =>
    (secretNames() ?? []).filter((n) => !known.has(n) && !providers().some((p) => p.credentials.some((c) => c.name === n)))
  return (
    <>
      <Section
        title="Claude Code"
        hint="What signs Claude Code in on corra's machines. The hub uses the Claude token, and the API key when there's no token."
      >
        <Card>
          <KeyRow name="claude.token" label="Claude token" hint="From claude setup-token: your subscription." />
          <KeyRow name="anthropic.api_key" label="Anthropic API key" hint="Billed per use, from the Anthropic Console." />
        </Card>
      </Section>
      <Section
        title="Antigravity"
        hint="Without a key, each machine signs in with Google the first time, and only you can send its Antigravity threads messages."
      >
        <Card>
          <KeyRow
            name="gemini.api_key"
            label="Gemini API key"
            hint="Billed per use, from Google AI Studio. Lets agents message these threads too."
          />
        </Card>
      </Section>
      <Section title="Full-access projects" hint="Only projects with full access get these, as the usual variables.">
        <Card>
          <KeyRow name="railway.token" label="Railway" hint="An account token, given as RAILWAY_API_TOKEN." />
          <KeyRow name="cloudflare.token" label="Cloudflare" hint="An API token, given as CLOUDFLARE_API_TOKEN." />
          <KeyRow name="cloudflare.account_id" label="Cloudflare account ID" hint="Given with the token as CLOUDFLARE_ACCOUNT_ID." />
        </Card>
      </Section>
      <Show when={others().length}>
        <Section title="Other keys" hint="Saved earlier; nothing in corra reads them.">
          <Card>
            <For each={others()}>{(n) => <KeyRow name={n} label={n} />}</For>
          </Card>
        </Section>
      </Show>
      <p class="text-[12.5px] text-mute">
        Keys are kept encrypted on the hub; they can be replaced or removed, never read back. Each provider's keys are under Providers, and
        GitHub's under Tools.
      </p>
    </>
  )
}

function Data() {
  const [settings, { refetch }] = createResource(() => call<Record<string, unknown>>({ op: 'get_settings' }))
  const [status] = createResource(() => call<{ db_bytes: number | null }>({ op: 'db_stats' }))
  const [days, setDays] = createSignal<string | null>(null)
  const [error, setError] = createSignal('')
  const value = () => days() ?? String(settings()?.retention_days ?? '')
  const save = async () => {
    const v = value().trim()
    if (v && !/^[1-9][0-9]*$/.test(v)) {
      setError('A whole number of days, or blank to keep them.')
      return
    }
    setError('')
    await call({ op: 'set_settings', settings: { retention_days: v } })
    setDays(null)
    await refetch()
  }
  const size = (n: number) => (n >= 1e9 ? `${(n / 1e9).toFixed(2)} GB` : `${(n / 1e6).toFixed(1)} MB`)
  return (
    <Section title="Data" hint="Threads, their events and session files all live in the hub's database.">
      <Card>
        <div class="flex items-center gap-3 px-3.5 py-3 text-[13px]">
          <span class="flex-1">Database</span>
          <span class="text-mute tabular-nums">{status()?.db_bytes != null ? size(status()!.db_bytes!) : '…'}</span>
        </div>
        <div class="flex flex-wrap items-center gap-x-3 gap-y-2 px-3.5 py-3 text-[13px]">
          <span class="flex-1">Empty the trash after</span>
          <div class="flex items-center gap-2">
            <input
              class={`${inputClass} w-20 text-right tabular-nums`}
              inputmode="numeric"
              placeholder="never"
              value={value()}
              onInput={(e) => setDays(e.currentTarget.value)}
              onKeyDown={(e) => e.key === 'Enter' && void save()}
            />
            <span class="text-mute">days</span>
            <Btn small disabled={days() === null} onClick={() => void save()}>
              Save
            </Btn>
          </div>
        </div>
      </Card>
      <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
    </Section>
  )
}

function Trash() {
  const threads = createMemo(() =>
    Object.values(state.threads)
      .filter((t) => t.hidden_at)
      .sort((a, b) => (b.hidden_at ?? 0) - (a.hidden_at ?? 0)),
  )
  const [error, setError] = createSignal('')
  const run = (p: Promise<unknown>) => void p.catch((e) => setError((e as Error).message))
  return (
    <Section title="Trash" hint="Threads you moved to the trash. They stay here until you delete them or the trash is emptied.">
      <Card>
        <For each={threads()} fallback={<div class="px-3.5 py-3 text-[13px] text-mute">The trash is empty.</div>}>
          {(t) => (
            <div class="flex items-center gap-3 px-3.5 py-2.5">
              <div class="min-w-0 flex-1">
                <div class="truncate text-[13.5px]">{t.title}</div>
                <div class="text-[12px] text-faint">
                  {state.projects[t.project]?.name ?? 'No project'} · trashed {ago(t.hidden_at!)} ago
                </div>
              </div>
              <Btn small onClick={() => run(call({ op: 'update_thread', thread: t.id, hidden: false }))}>
                Restore
              </Btn>
              <Btn
                small
                kind="ghost"
                onClick={() =>
                  confirm(`Delete "${t.title}" for good? This can't be undone.`) && run(call({ op: 'delete_thread', thread: t.id }))
                }
              >
                Delete
              </Btn>
            </div>
          )}
        </For>
      </Card>
      <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
    </Section>
  )
}

function MachineSetup() {
  const [settings, { refetch }] = createResource(() => call<Record<string, unknown>>({ op: 'get_settings' }))
  const [saved, setSaved] = createSignal(false)
  const value = () => String(settings()?.setup_repo ?? '')
  const save = async (r: string) => {
    await call({ op: 'set_settings', settings: { setup_repo: r } })
    setSaved(true)
    await refetch()
  }
  return (
    <Section
      title="Your setup repo"
      hint="Corra reads its corra/ folder: shared/ for every machine (AGENTS.md, skills, claude-settings.json, a setup script) and setups/<name>/ for each kind of project (a setup script, its repos and the names of the env vars it needs)."
    >
      <RepoPicker value={value()} placeholder="Pick your setup repo" onPick={(r) => void save(r)} />
      <Show when={saved()}>
        <p class="mt-2 text-[12.5px] text-mute">Saved. Machines take it at their next wake.</p>
      </Show>
    </Section>
  )
}

function EnvVars() {
  const [names, { refetch }] = createResource(() => call<string[]>({ op: 'list_env' }))
  const [editing, setEditing] = createSignal<string | null>(null)
  const [name, setName] = createSignal('')
  const [value, setValue] = createSignal('')
  const [error, setError] = createSignal('')
  const save = async (n: string) => {
    setError('')
    try {
      await call({ op: 'set_env', name: n.trim(), value: value() })
      setValue('')
      setName('')
      setEditing(null)
      await refetch()
    } catch (e) {
      setError((e as Error).message)
    }
  }
  return (
    <Section
      title="Env vars"
      hint="Set once and shared: a project gets the ones its setup names, or that you add to it. Kept encrypted, never shown again."
    >
      <Card>
        <For each={names() ?? []} fallback={<p class="px-3.5 py-3 text-[13px] text-mute">None yet.</p>}>
          {(n) => (
            <div class="px-3.5 py-2.5">
              <div class="flex items-center gap-3">
                <span class="min-w-0 flex-1 truncate font-mono text-[12.5px]">{n}</span>
                <Btn
                  small
                  kind="ghost"
                  onClick={() => confirm(`Remove ${n}?`) && void call({ op: 'delete_env', name: n }).then(() => refetch())}
                >
                  Remove
                </Btn>
                <Btn small onClick={() => (setEditing(editing() === n ? null : n), setValue(''))}>
                  Replace
                </Btn>
              </div>
              <Show when={editing() === n}>
                <div class="mt-2 flex gap-2">
                  <input
                    class={`${inputClass} flex-1`}
                    type="password"
                    autocomplete="off"
                    autofocus
                    placeholder="Its new value"
                    value={value()}
                    onInput={(e) => setValue(e.currentTarget.value)}
                    onKeyDown={(e) => e.key === 'Enter' && value() && void save(n)}
                  />
                  <Btn kind="primary" disabled={!value()} onClick={() => void save(n)}>
                    Save
                  </Btn>
                </div>
              </Show>
            </div>
          )}
        </For>
      </Card>
      <div class="mt-2 flex gap-2">
        <input
          class={`${inputClass} w-44 font-mono text-[12.5px]`}
          placeholder="NAME"
          spellcheck={false}
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <input
          class={`${inputClass} min-w-0 flex-1`}
          type="password"
          autocomplete="off"
          placeholder="value"
          value={editing() ? '' : value()}
          onInput={(e) => (setEditing(null), setValue(e.currentTarget.value))}
          onKeyDown={(e) => e.key === 'Enter' && name().trim() && value() && void save(name())}
        />
        <Btn disabled={!name().trim() || !value()} onClick={() => void save(name())}>
          Add
        </Btn>
      </div>
      <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
    </Section>
  )
}

type SessionRow = { id: string; kind: string; name: string; created: number; expires: number; push?: boolean }

function Devices() {
  const [code, setCode] = createSignal('')
  const [error, setError] = createSignal('')
  const [sessions, { refetch }] = createResource(() => call<SessionRow[]>({ op: 'sessions' }))
  const label = (s: SessionRow) => (s.kind === 'device' ? s.name || 'A device' : 'A browser')
  return (
    <>
      <Section
        title="This browser"
        hint="To sign in a phone, make a pairing code here and scan its QR code with the phone, or type the code in. It works once, within 10 minutes."
      >
        <div class="flex flex-wrap items-center gap-3">
          <Btn
            onClick={async () => {
              setError('')
              try {
                setCode(await newPairingCode())
              } catch (e) {
                setError((e as Error).message)
              }
            }}
          >
            Make a pairing code
          </Btn>
          <Show when={code()}>
            <span class="num font-mono text-[18px] tracking-[0.3em]">{code()}</span>
          </Show>
          <div class="flex-1" />
          <Btn kind="ghost" onClick={() => void signOut().then(() => location.reload())}>
            Sign out
          </Btn>
        </div>
        <Show when={code()}>
          <div
            class="mt-4 size-44 rounded-xl bg-white p-2.5 text-black [&_svg]:size-full"
            innerHTML={renderSVG(`${location.origin}/#/pair/${code()}`, { border: 0, pixelSize: 6 })}
          />
        </Show>
        <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
      </Section>
      <Section title="Signed in" hint="Every browser and device that can use this hub. Signing one out takes effect at once.">
        <Show when={(sessions() ?? []).length > 0}>
          <Card>
            <For each={sessions() ?? []}>
              {(s) => (
                <div class="flex items-center gap-3 px-3.5 py-2.5">
                  <div class="min-w-0 flex-1">
                    <div class="truncate text-[13.5px]">{label(s)}</div>
                    <div class="text-[12.5px] text-mute">
                      Signed in {ago(s.created)}
                      {s.push ? ' · notifications on' : ''}
                    </div>
                  </div>
                  <Btn
                    small
                    kind="ghost"
                    onClick={() =>
                      confirm(`Sign ${label(s)} out?`) && void call({ op: 'revoke_session', session: s.id }).then(() => refetch())
                    }
                  >
                    Sign out
                  </Btn>
                </div>
              )}
            </For>
          </Card>
        </Show>
      </Section>
    </>
  )
}

const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).replace(/([^.])$/, '$1.')

function VersionRow(props: {
  icon: JSX.Element
  tone?: 'warn'
  version: string
  children: JSX.Element
  details?: JSX.Element
  action?: JSX.Element
}) {
  return (
    <div class="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 px-3.5 py-3 sm:grid-cols-[auto_minmax(0,1fr)_auto]">
      <div
        class="flex size-9 shrink-0 items-center justify-center rounded-lg"
        classList={{ 'bg-ink/[0.05] text-mute': !props.tone, 'bg-warn/10 text-warn': props.tone === 'warn' }}
      >
        {props.icon}
      </div>
      <div class="min-w-0">
        <div class="truncate font-mono text-[12px] font-medium sm:text-[12.5px]">{props.version}</div>
        <div class="text-[12.5px] text-mute">{props.children}</div>
      </div>
      <div class="col-start-2 mt-1 text-[12.5px] text-mute empty:hidden sm:col-end-4">{props.details}</div>
      <div class="col-start-2 mt-2 empty:hidden sm:col-start-3 sm:row-start-1 sm:mt-0">{props.action}</div>
    </div>
  )
}

function Version() {
  const [info, { mutate, refetch }] = createResource(() => call<HubVersion>({ op: 'hub_version', check: false }))
  const [checking, setChecking] = createSignal(false)
  const [going, setGoing] = createSignal<{ to: string; restarting: boolean } | null>(null)
  createEffect(
    on(
      () => state.conn,
      (c) => {
        if (c !== 'open') return
        if (going()?.restarting) setGoing(null)
        void refetch()
      },
      { defer: true },
    ),
  )
  const every = setInterval(() => !going() && void refetch(), 30_000)
  onCleanup(() => clearInterval(every))
  const [error, setError] = createSignal('')
  const [log, setLog] = createSignal(false)
  const checkNow = async () => {
    setChecking(true)
    setError('')
    try {
      mutate(await call<HubVersion>({ op: 'hub_version', check: true }))
    } catch (e) {
      setError((e as Error).message)
    }
    setChecking(false)
  }
  const switchTo = async (to: string, back: boolean) => {
    const ask = `${back ? 'Roll the hub back' : 'Update the hub'} to ${to}? It restarts, so the app and machines reconnect after a few seconds, and requests waiting on you are cancelled.`
    if (!confirm(ask)) return
    setError('')
    setGoing({ to, restarting: false })
    try {
      await call({ op: 'switch_hub', version: to })
      setGoing({ to, restarting: true })
    } catch (e) {
      setError((e as Error).message)
      setGoing(null)
    }
  }
  const managed = () => !!info()?.managed
  const update = () => (managed() ? (info()?.update ?? null) : null)
  const retry = () => !!update() && info()?.failed?.version === update()
  const previous = () => {
    const i = info()
    if (!i?.managed || !i.previous || i.previous === i.update) return null
    return { version: i.previous, older: i.previous < i.version, tooOld: i.previous_too_old }
  }
  return (
    <Section title="Version" hint="Updates come from corra's releases on GitHub. A version that doesn't start is rolled back by itself.">
      <Card>
        <VersionRow
          icon={<Server size={16} />}
          version={info()?.version ?? '…'}
          action={
            <Show when={managed() && !info()?.update && !info()?.trial && !going()}>
              <Btn small kind="ghost" class="max-sm:-ml-2.5" disabled={checking()} onClick={() => void checkNow()}>
                {checking() ? 'Checking' : 'Check now'}
              </Btn>
            </Show>
          }
        >
          <Show
            when={going()}
            fallback={
              <Show when={info()} fallback="Checking…">
                {(i) => <span classList={{ 'text-bad': !!runningLine(i()).bad }}>{runningLine(i()).text}</span>}
              </Show>
            }
          >
            {(g) => (
              <span class="text-ink">
                {g().restarting ? 'Restarting into ' : 'Getting '}
                <span class="font-mono">{g().to}</span>…
              </span>
            )}
          </Show>
        </VersionRow>
        <Show when={update() && !retry()}>
          <VersionRow
            icon={<CircleArrowUp size={16} />}
            version={update()!}
            action={
              <Show when={!going()}>
                <Btn small kind="primary" onClick={() => void switchTo(update()!, false)}>
                  Update
                </Btn>
              </Show>
            }
          >
            New version
            <Show when={info()?.changes}>
              {(c) => (
                <>
                  {' · '}
                  <a
                    class="inline-flex items-center gap-0.5 text-ink underline-offset-2 hover:underline"
                    href={c()}
                    target="_blank"
                    rel="noreferrer"
                  >
                    What's new
                    <ArrowUpRight size={12} />
                  </a>
                </>
              )}
            </Show>
          </VersionRow>
        </Show>
        <Show when={previous()}>
          {(p) => (
            <VersionRow
              icon={<History size={16} />}
              version={p().version}
              action={
                <Show when={!going() && !p().tooOld}>
                  <Btn small onClick={() => void switchTo(p().version, p().older)}>
                    {p().older ? 'Roll back' : 'Switch back'}
                  </Btn>
                </Show>
              }
            >
              {p().tooOld ? "Previous version · can't go back past a database change" : 'Previous version'}
            </VersionRow>
          )}
        </Show>
        <Show when={info()?.failed}>
          {(f) => (
            <VersionRow
              icon={<TriangleAlert size={16} />}
              tone="warn"
              version={f().version}
              action={
                <Show when={retry() && !going()}>
                  <Btn small onClick={() => void switchTo(f().version, false)}>
                    Try again
                  </Btn>
                </Show>
              }
              details={
                <>
                  {sentence(f().reason)}
                  <Show when={f().log.length}>
                    <button class="mt-1.5 flex items-center gap-1 hover:text-ink" onClick={() => setLog(!log())}>
                      <ChevronDown size={14} class={log() ? 'rotate-180' : ''} />
                      {log() ? 'Hide its last log lines' : 'Show its last log lines'}
                    </button>
                    <Show when={log()}>
                      <pre class="mt-2 max-h-64 overflow-auto rounded-lg bg-ink/[0.04] p-2.5 font-mono text-[11.5px] leading-[1.5] break-words whitespace-pre-wrap">
                        {f().log.join('\n')}
                      </pre>
                    </Show>
                  </Show>
                </>
              }
            >
              <span class="text-ink">
                {f().version === info()?.version ? 'Failed its first start, then started again' : 'Didn’t start, so the hub went back'}
              </span>{' '}
              · {when(f().at)}
            </VersionRow>
          )}
        </Show>
      </Card>
      <p class="mt-2 text-[12.5px] text-bad">{error() || info()?.launcher_error}</p>
    </Section>
  )
}
