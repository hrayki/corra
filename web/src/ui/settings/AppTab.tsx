import { For, Show, createEffect, createResource, createSignal, on, onCleanup, type JSX } from 'solid-js'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import Bell from 'lucide-solid/icons/bell'
import Server from 'lucide-solid/icons/server'
import History from 'lucide-solid/icons/rotate-ccw-clock'
import TriangleAlert from 'lucide-solid/icons/triangle-exclamation-point'
import ArrowUpRight from 'lucide-solid/icons/arrow-up-right'
import CircleArrowUp from 'lucide-solid/icons/circle-arrow-up'
import { renderSVG } from 'uqr'
import { call, state, updateSettings } from '../../lib/store'
import { newPairingCode, signOut } from '../../lib/auth'
import { choice, setChoice, type ThemeChoice } from '../../lib/theme'
import { ago } from '../../lib/format'
import { runningLine, when } from '../../lib/hubversion'
import { pushState, turnOff, turnOn, type PushState } from '../../lib/notify'
import type { HubVersion } from '../../proto/HubVersion'
import { Btn, inputClass } from '../bits'
import { Card, InlineConfirm, Section } from './shared'

export function AppTab() {
  const hubUp = () => state.conn === 'open'
  return (
    <>
      <Appearance />
      <Show when={hubUp()}>
        <Notifications />
        <Compaction />
        <Version />
      </Show>
      <Devices />
    </>
  )
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
  const [over, setOver] = createSignal<string | null>(null)
  const [error, setError] = createSignal('')
  const enabled = () => state.settings.compact_idle !== 'never' && state.settings.compact_idle !== false
  const shown = () => over() ?? Number(state.settings.compact_over ?? 200000).toLocaleString('en-US')
  const setEnabled = async (on: boolean) => {
    setError('')
    try {
      await updateSettings({ compact_idle: on ? 'before' : 'never' })
    } catch (e) {
      setError((e as Error).message)
    }
  }
  const save = async () => {
    const v = shown().replace(/[\s,_]/g, '')
    if (!/^[1-9][0-9]{3,}$/.test(v)) {
      setError('A number of tokens, like 200000.')
      return
    }
    setError('')
    try {
      await updateSettings({ compact_over: v })
      setOver(null)
    } catch (e) {
      setError((e as Error).message)
    }
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
  const [push, { refetch: recheck }] = createResource<PushState>(() => pushState())
  const [busy, setBusy] = createSignal(false)
  const [note, setNote] = createSignal('')
  const kinds = () => (state.settings.notify ?? {}) as Record<string, boolean>
  const setKind = async (k: string, on: boolean) => {
    setNote('')
    try {
      await updateSettings({ notify: { ...kinds(), [k]: on } })
    } catch (e) {
      setNote((e as Error).message)
    }
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
  const [pendingSwitch, setPendingSwitch] = createSignal<{ to: string; back: boolean } | null>(null)
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
  const every = window.setInterval(() => !going() && void refetch(), 30_000)
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
  const executeSwitch = async () => {
    const req = pendingSwitch()
    if (!req) return
    setPendingSwitch(null)
    setError('')
    setGoing({ to: req.to, restarting: false })
    try {
      await call({ op: 'switch_hub', version: req.to })
      setGoing({ to: req.to, restarting: true })
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
    <Section
      title="Hub version"
      hint="Updates come from corra's releases on GitHub. A version that doesn't start is rolled back automatically."
    >
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
                <Btn small kind="primary" onClick={() => setPendingSwitch({ to: update()!, back: false })}>
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
                  <Btn small onClick={() => setPendingSwitch({ to: p().version, back: p().older })}>
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
                  <Btn small onClick={() => setPendingSwitch({ to: f().version, back: false })}>
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
      <Show when={pendingSwitch()}>
        {(s) => (
          <div class="mt-3 rounded-xl border border-warn/40 bg-warn/[0.06] px-3.5 py-3 text-[12.5px]">
            <div class="text-ink">
              {s().back ? 'Roll the hub back' : 'Update the hub'} to <span class="font-mono font-medium">{s().to}</span>? The hub restarts
              and cancels any open approval cards while apps and machines reconnect.
            </div>
            <div class="mt-2.5 flex gap-2">
              <Btn small kind="primary" onClick={() => void executeSwitch()}>
                {s().back ? 'Roll back now' : 'Update now'}
              </Btn>
              <Btn small kind="ghost" onClick={() => setPendingSwitch(null)}>
                Cancel
              </Btn>
            </div>
          </div>
        )}
      </Show>
      <p class="mt-2 text-[12.5px] text-bad">{error() || info()?.launcher_error}</p>
    </Section>
  )
}

type SessionRow = { id: string; kind: string; name: string; created: number; expires: number; push?: boolean }

function Devices() {
  const [code, setCode] = createSignal('')
  const [error, setError] = createSignal('')
  const [sessions, { refetch }] = createResource(
    () => state.conn === 'open',
    (up) => (up ? call<SessionRow[]>({ op: 'sessions' }) : Promise.resolve([] as SessionRow[])),
  )
  const label = (s: SessionRow) => (s.kind === 'device' ? s.name || 'A device' : 'A browser')
  return (
    <Section title="Devices & sessions" hint="Pair a phone with a one-time code, or sign out any browser or device connected to this hub.">
      <Card>
        <div class="px-3.5 py-3">
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
              Sign out this browser
            </Btn>
          </div>
          <Show when={code()}>
            <div
              class="mt-4 size-44 rounded-xl bg-white p-2.5 text-black [&_svg]:size-full"
              innerHTML={renderSVG(`${location.origin}/#/pair/${code()}`, { border: 0, pixelSize: 6 })}
            />
          </Show>
        </div>
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
              <InlineConfirm
                label="Sign out"
                confirmLabel="Sign out"
                danger
                onConfirm={() => call({ op: 'revoke_session', session: s.id }).then(() => refetch())}
              />
            </div>
          )}
        </For>
      </Card>
      <Show when={error()}>
        <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
      </Show>
    </Section>
  )
}
