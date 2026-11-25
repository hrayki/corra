import { For, Show, createMemo, createResource, createSignal, type JSX } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import Laptop from 'lucide-solid/icons/laptop'
import Cloud from 'lucide-solid/icons/cloud'
import Bell from 'lucide-solid/icons/bell'
import { callOn, state, type MachineX } from '../lib/store'
import { newPairingCode, signOut } from '../lib/auth'
import { renderSVG } from 'uqr'
import { choice, omarchy, setChoice, type ThemeChoice } from '../lib/theme'
import { ago } from '../lib/format'
import { Btn, Menu, inputClass } from './bits'
import { RepoPicker } from './RepoPicker'
import { pushState, turnOff, turnOn, type PushState } from '../lib/notify'
import { paired, providerLabel, providers, refreshCatalog } from '../lib/catalog'

export function Settings() {
  const hubUp = () => state.conn.hub === 'open'
  const pages = createMemo(() => {
    const all: { key: string; label: string; show: boolean }[] = [
      { key: 'general', label: 'General', show: true },
      { key: 'machines', label: 'Machines', show: true },
      { key: 'keys', label: 'Keys', show: hubUp() },
      { key: 'env', label: 'Env vars', show: hubUp() },
      { key: 'setup', label: 'Setup repo', show: hubUp() },
      { key: 'data', label: 'Backups and data', show: hubUp() },
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
              </Show>
              <Show when={state.connectUrl}>
                <HubLink />
              </Show>
            </Show>
            <Show when={on() === 'machines'}>
              <Machines />
            </Show>
            <Show when={on() === 'keys'}>
              <Credentials />
            </Show>
            <Show when={on() === 'env'}>
              <EnvVars />
            </Show>
            <Show when={on() === 'setup'}>
              <MachineSetup />
            </Show>
            <Show when={on() === 'data'}>
              <Backups />
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
  const options = createMemo(() => {
    const o: { value: ThemeChoice; label: string }[] = [
      { value: 'auto', label: 'Auto' },
      { value: 'light', label: 'Light' },
      { value: 'dark', label: 'Dark' },
    ]
    if (omarchy()) o.push({ value: 'omarchy', label: 'Omarchy' })
    return o
  })
  return (
    <Section title="Appearance">
      <div class="inline-flex rounded-lg border border-line p-0.5">
        <For each={options()}>
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
      <Show when={choice() === 'omarchy' && omarchy()}>
        <p class="mt-2 text-[12.5px] text-mute">
          Following your Omarchy theme{omarchy()!.name ? `, ${omarchy()!.name.replace(/-/g, ' ')}` : ''}. It changes when you switch themes.
        </p>
      </Show>
    </Section>
  )
}

function Compaction() {
  const [settings, { refetch }] = createResource(() => callOn<Record<string, unknown>>('hub', { op: 'get_settings' }))
  const [over, setOver] = createSignal<string | null>(null)
  const [error, setError] = createSignal('')
  const mode = () => String(settings()?.compact_idle ?? 'return')
  const shown = () => over() ?? Number(settings()?.compact_over ?? 200000).toLocaleString('en-US')
  const setMode = async (m: string) => {
    await callOn('hub', { op: 'set_settings', settings: { compact_idle: m } })
    await refetch()
  }
  const save = async () => {
    const v = shown().replace(/[\s,_]/g, '')
    if (!/^[1-9][0-9]{3,}$/.test(v)) {
      setError('A number of tokens, like 200000.')
      return
    }
    setError('')
    await callOn('hub', { op: 'set_settings', settings: { compact_over: v } })
    setOver(null)
    await refetch()
  }
  const options = [
    { value: 'never', label: 'Never' },
    { value: 'return', label: 'When I return' },
    { value: 'before', label: 'Before the cache expires' },
  ]
  return (
    <Section
      title="Compact idle threads"
      hint="Claude's prompt cache lasts about an hour on a subscription. A long thread you return to after that reads its whole history again at full price, unless it's summarized first."
    >
      <div class="inline-flex max-w-full flex-wrap rounded-lg border border-line p-0.5">
        <For each={options}>
          {(o) => (
            <button
              class="h-7 rounded-md px-3 text-[13px]"
              classList={{ 'bg-ink text-bg font-medium': mode() === o.value, 'text-mute hover:text-ink': mode() !== o.value }}
              aria-pressed={mode() === o.value}
              onClick={() => void setMode(o.value)}
            >
              {o.label}
            </button>
          )}
        </For>
      </div>
      <p class="mt-2 text-[12.5px] text-mute">
        <Show when={mode() === 'never'}>Threads keep their full history. You can still send /compact yourself.</Show>
        <Show when={mode() === 'return'}>
          A Compact button by Send summarizes a long thread before your next message, once its cache has expired.
        </Show>
        <Show when={mode() === 'before'}>
          Corra sends /compact to an idle long thread shortly before its cache expires, while the summary is cheap. If it misses that, the
          Compact button shows instead.
        </Show>
      </p>
      <Show when={mode() !== 'never'}>
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
  const [settings, { refetch }] = createResource(() => callOn<Record<string, unknown>>('hub', { op: 'get_settings' }))
  const [push, { refetch: recheck }] = createResource<PushState>(() => (state.desktop ? Promise.resolve('on') : pushState()))
  const [busy, setBusy] = createSignal(false)
  const [note, setNote] = createSignal('')
  const kinds = () => (settings()?.notify ?? {}) as Record<string, boolean>
  const setKind = async (k: string, on: boolean) => {
    await callOn('hub', { op: 'set_settings', settings: { notify: { ...kinds(), [k]: on } } })
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
    if (state.desktop) return 'The desktop app shows them while it runs.'
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
            <div class="truncate text-[13.5px] font-medium">{state.desktop ? 'This computer' : 'This browser'}</div>
            <div class="text-[12.5px] text-mute">{status()}</div>
          </div>
          <Show when={!state.desktop && push() === 'off'}>
            <Btn small disabled={busy()} onClick={() => void act(turnOn)}>
              Turn on
            </Btn>
          </Show>
          <Show when={state.desktop || push() === 'on'}>
            <Btn small kind="ghost" disabled={busy()} onClick={() => void act(() => callOn('hub', { op: 'push_test' }))}>
              Send a test
            </Btn>
          </Show>
          <Show when={!state.desktop && push() === 'on'}>
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

function HubLink() {
  const here = () => (state.thisMachine ? state.machines[state.thisMachine] : undefined)
  return (
    <Section
      title="This computer"
      hint="It's one of the hub's machines. When the hub can't be reached, corra keeps working here and hands the threads over when it's back."
    >
      <Card>
        <div class="flex items-center gap-3 px-3.5 py-3">
          <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-ink/[0.05] text-mute">
            <Laptop size={15} />
          </span>
          <div class="min-w-0 flex-1">
            <div class="truncate text-[13.5px] font-medium">{here()?.name ?? 'This computer'}</div>
            <div class="truncate text-[12.5px] text-mute">
              {state.offline ? `${state.offline} can't be reached` : `Connected to ${location.host}`}
            </div>
          </div>
          <a class="text-[13px] text-mute underline decoration-line underline-offset-4 hover:text-ink" href={state.connectUrl!}>
            Use another hub
          </a>
        </div>
      </Card>
    </Section>
  )
}

function Machines() {
  const list = createMemo(() =>
    Object.values(state.machines)
      .filter((m) => m.status !== 'destroyed')
      .sort(
        (a, b) =>
          Number(b.id === state.thisMachine) - Number(a.id === state.thisMachine) ||
          Number(b.status === 'connected') - Number(a.status === 'connected') ||
          b.created_at - a.created_at,
      ),
  )
  const label = (m: MachineX) => (m.id === state.thisMachine ? 'This computer' : m.name)
  const detail = (m: MachineX) => {
    const parts: string[] = []
    if (m.id === state.thisMachine) parts.push(m.name)
    else parts.push(paired(m.provider) ? 'Paired' : providerLabel(m.provider))
    if (m.project && state.projects[m.project]) parts.push(state.projects[m.project].name)
    if (m.info?.disk_used_mb) parts.push(`${(m.info.disk_used_mb / 1024).toFixed(1)} GB of disk`)
    parts.push(m.status === 'connected' ? 'awake' : m.status === 'offline' && m.last_seen ? `seen ${ago(m.last_seen)} ago` : m.status)
    return parts.join(' · ')
  }
  return (
    <Section
      title="Machines"
      hint={
        state.conn.hub === 'open'
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
                  <span class="truncate">{label(m)}</span>
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
            </div>
          )}
        </For>
      </Card>
    </Section>
  )
}

const known = () => [
  ...KNOWN.slice(0, 2),
  ...providers().flatMap((p) => p.credentials.map((c) => ({ name: c.name, label: `${p.label} ${c.label.toLowerCase()}`, hint: c.hint }))),
  ...KNOWN.slice(2),
]

const KNOWN: { name: string; label?: string; hint: string }[] = [
  { name: 'claude.token', label: 'Claude Code', hint: 'From claude setup-token. Signs Claude Code in on remote machines.' },
  {
    name: 'github.token',
    label: 'GitHub',
    hint: 'A fine-grained GitHub token for your repos: Contents, Pull requests and Issues read and write, Actions and Checks read, no Workflows.',
  },
  { name: 'digitalocean.token', label: 'DigitalOcean', hint: 'A DigitalOcean token that can make droplets. Runs the Android test phone.' },
  { name: 'railway.token', label: 'Railway', hint: 'A Railway account token. Only manager projects get it, as RAILWAY_API_TOKEN.' },
  {
    name: 'cloudflare.token',
    label: 'Cloudflare',
    hint: 'A Cloudflare API token. Only manager projects get it, as CLOUDFLARE_API_TOKEN.',
  },
  {
    name: 'cloudflare.account_id',
    label: 'Cloudflare account ID',
    hint: 'Your Cloudflare account ID, given with the token as CLOUDFLARE_ACCOUNT_ID.',
  },
]

function Credentials() {
  const [names, { refetch }] = createResource(() => callOn<string[]>('hub', { op: 'list_secrets' }))
  const [editing, setEditing] = createSignal<string | null>(null)
  const [name, setName] = createSignal('')
  const [value, setValue] = createSignal('')
  const [error, setError] = createSignal('')
  const has = (n: string) => !!names()?.includes(n)
  const save = async (n: string) => {
    setError('')
    try {
      await callOn('hub', { op: 'set_secret', name: n.trim(), value: value() })
      setValue('')
      setName('')
      setEditing(null)
      await refetch()
      refreshCatalog()
    } catch (e) {
      setError((e as Error).message)
    }
  }
  const row = (n: string, hint: string | undefined, label?: string) => (
    <div class="px-3.5 py-3">
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-baseline gap-x-2">
            <span class="text-[13.5px] font-medium">{label ?? n}</span>
            <span class="text-[12px]" classList={{ 'text-ok': has(n), 'text-faint': !has(n) }}>
              {has(n) ? 'Saved' : 'Not set'}
            </span>
          </div>
          <Show when={hint}>
            <div class="mt-0.5 text-[12.5px] text-mute">{hint}</div>
          </Show>
          <Show when={label}>
            <div class="mt-0.5 font-mono text-[11.5px] text-faint">{n}</div>
          </Show>
        </div>
        <Btn small onClick={() => (setEditing(editing() === n ? null : n), setValue(''))}>
          {has(n) ? 'Replace' : 'Add'}
        </Btn>
        <Show when={has(n)}>
          <Menu
            label={`More for ${label ?? n}`}
            align="end"
            class="flex size-8 items-center justify-center rounded-lg text-faint hover:bg-ink/5 hover:text-ink max-md:size-9"
            items={[
              {
                label: 'Remove',
                danger: true,
                onSelect: () =>
                  confirm(`Remove ${label ?? n}?`) &&
                  void callOn('hub', { op: 'delete_secret', name: n }).then(() => {
                    void refetch()
                    refreshCatalog()
                  }),
              },
            ]}
          >
            <span class="text-[15px] leading-none">⋯</span>
          </Menu>
        </Show>
      </div>
      <Show when={editing() === n}>
        <div class="mt-2 flex gap-2">
          <input
            class={`${inputClass} flex-1`}
            type="password"
            autocomplete="off"
            autofocus
            placeholder="Paste the value"
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
  )
  return (
    <Section title="Keys" hint="Kept encrypted on the hub. They can be replaced or removed, never read back.">
      <Card>
        <For each={known()}>{(k) => row(k.name, k.hint, k.label)}</For>
        <For each={(names() ?? []).filter((n) => !known().some((k) => k.name === n) && !n.startsWith('backup.'))}>
          {(n) => row(n, undefined)}
        </For>
      </Card>
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <input
          class={`${inputClass} w-44 font-mono text-[12.5px]`}
          placeholder="another.name"
          spellcheck={false}
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <input
          class={`${inputClass} flex-1`}
          type="password"
          autocomplete="off"
          placeholder="value"
          value={editing() ? '' : value()}
          onInput={(e) => (setEditing(null), setValue(e.currentTarget.value))}
        />
        <Btn disabled={!name().trim() || !value() || !!editing()} onClick={() => void save(name())}>
          Add
        </Btn>
      </div>
      <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
    </Section>
  )
}

function Data() {
  const [settings, { refetch }] = createResource(() => callOn<Record<string, unknown>>('hub', { op: 'get_settings' }))
  const [status] = createResource(() => callOn<{ db_bytes: number | null }>('hub', { op: 'backup_status' }))
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
    await callOn('hub', { op: 'set_settings', settings: { retention_days: v } })
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
              <Btn small onClick={() => run(callOn(t.src, { op: 'update_thread', thread: t.id, hidden: false }))}>
                Restore
              </Btn>
              <Btn
                small
                kind="ghost"
                onClick={() =>
                  confirm(`Delete "${t.title}" for good? This can't be undone.`) &&
                  run(callOn(t.src, { op: 'delete_thread', thread: t.id }))
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
  const [settings, { refetch }] = createResource(() => callOn<Record<string, unknown>>('hub', { op: 'get_settings' }))
  const [saved, setSaved] = createSignal(false)
  const value = () => String(settings()?.setup_repo ?? '')
  const save = async (r: string) => {
    await callOn('hub', { op: 'set_settings', settings: { setup_repo: r } })
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
  const [names, { refetch }] = createResource(() => callOn<string[]>('hub', { op: 'list_env' }))
  const [editing, setEditing] = createSignal<string | null>(null)
  const [name, setName] = createSignal('')
  const [value, setValue] = createSignal('')
  const [error, setError] = createSignal('')
  const save = async (n: string) => {
    setError('')
    try {
      await callOn('hub', { op: 'set_env', name: n.trim(), value: value() })
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
                  onClick={() => confirm(`Remove ${n}?`) && void callOn('hub', { op: 'delete_env', name: n }).then(() => refetch())}
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
  const [sessions, { refetch }] = createResource(() => callOn<SessionRow[]>('hub', { op: 'sessions' }))
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
                      confirm(`Sign ${label(s)} out?`) && void callOn('hub', { op: 'revoke_session', session: s.id }).then(() => refetch())
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

type BackupStatus = { configured: boolean; error: string | null; last: { at: number; db_bytes: number } | null }

function Backups() {
  const [status, { refetch }] = createResource(() => callOn<BackupStatus>('hub', { op: 'backup_status' }))
  const [open, setOpen] = createSignal(false)
  const [busy, setBusy] = createSignal(false)
  const [error, setError] = createSignal('')
  const [form, setForm] = createSignal({ endpoint: '', bucket: '', access_key: '', secret_key: '' })
  const save = async () => {
    setError('')
    try {
      for (const [k, v] of Object.entries(form())) if (v.trim()) await callOn('hub', { op: 'set_secret', name: `backup.${k}`, value: v })
      setForm({ endpoint: '', bucket: '', access_key: '', secret_key: '' })
      setOpen(false)
      await refetch()
    } catch (e) {
      setError((e as Error).message)
    }
  }
  const now = async () => {
    setBusy(true)
    setError('')
    try {
      await callOn('hub', { op: 'backup_now' })
    } catch (e) {
      setError((e as Error).message)
    }
    setBusy(false)
    await refetch()
  }
  const mb = (n: number) => `${(n / 1_000_000).toFixed(1)} MB`
  const field = (k: keyof ReturnType<typeof form>, placeholder: string, secret = false) => (
    <input
      class={`${inputClass} w-full ${secret ? '' : 'font-mono text-[12.5px]'}`}
      type={secret ? 'password' : 'text'}
      autocomplete="off"
      spellcheck={false}
      placeholder={placeholder}
      value={form()[k]}
      onInput={(e) => setForm({ ...form(), [k]: e.currentTarget.value })}
    />
  )
  return (
    <Section
      title="Backups"
      hint="Every 6 hours the hub copies its database and every session file to S3-compatible storage, such as Cloudflare R2."
    >
      <Card>
        <div class="flex items-center gap-3 px-3.5 py-3">
          <span
            class="size-2 shrink-0 rounded-full"
            classList={{
              'bg-ok': !!status()?.configured && !status()?.error,
              'bg-bad': !!status()?.error,
              'bg-faint/60': !status()?.configured,
            }}
          />
          <div class="min-w-0 flex-1 text-[13px]">
            <Show when={status()?.configured} fallback={<span class="text-mute">Not set up</span>}>
              <Show when={status()?.last} fallback={<span class="text-mute">No backup yet</span>}>
                <span>Last backup {ago(status()!.last!.at)} ago</span>
                <span class="text-mute"> · {mb(status()!.last!.db_bytes)}</span>
              </Show>
            </Show>
            <Show when={status()?.error}>
              <div class="text-[12.5px] text-bad">{status()!.error}</div>
            </Show>
          </div>
          <Btn small kind="ghost" onClick={() => setOpen(!open())}>
            {status()?.configured ? 'Change' : 'Set up'}
          </Btn>
          <Show when={status()?.configured}>
            <Btn small disabled={busy()} onClick={() => void now()}>
              {busy() ? 'Backing up' : 'Back up now'}
            </Btn>
          </Show>
        </div>
        <Show when={open()}>
          <div class="grid gap-2 px-3.5 py-3 sm:grid-cols-2">
            {field('endpoint', 'https://<account>.r2.cloudflarestorage.com')}
            {field('bucket', 'bucket name')}
            {field('access_key', 'access key id')}
            {field('secret_key', 'secret access key', true)}
            <div class="sm:col-span-2">
              <Btn kind="primary" onClick={() => void save()}>
                Save
              </Btn>
            </div>
          </div>
        </Show>
      </Card>
      <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
    </Section>
  )
}
