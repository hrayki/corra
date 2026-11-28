import { For, Show, createMemo, createResource, createSignal } from 'solid-js'
import Laptop from 'lucide-solid/icons/laptop'
import Cloud from 'lucide-solid/icons/cloud'
import { call, state, updateSettings } from '../../lib/store'
import { paired, providerLabel, providerOf, providers } from '../../lib/catalog'
import { ago } from '../../lib/format'
import type { Machine } from '../../proto/Machine'
import { Btn, inputClass } from '../bits'
import { Card, InlineConfirm, Section } from './shared'

export function MachinesTab() {
  const hubUp = () => state.conn === 'open'
  return (
    <>
      <Machines />
      <Show when={hubUp()}>
        <Data />
        <Trash />
      </Show>
    </>
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
          ? 'Cloud machines sleep after 15 idle minutes with their disk kept, and wake on the next message or from the project page.'
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

function Data() {
  const [status] = createResource(() => call<{ db_bytes: number | null }>({ op: 'db_stats' }))
  const [days, setDays] = createSignal<string | null>(null)
  const [settleDays, setSettleDays] = createSignal<string | null>(null)
  const [error, setError] = createSignal('')
  const value = () => days() ?? String(state.settings.retention_days ?? '')
  const settleValue = () => settleDays() ?? String(state.settings.auto_settle_days ?? '')
  const saveRetention = async () => {
    const v = value().trim()
    if (v && !/^[1-9][0-9]*$/.test(v)) {
      setError('A whole number of days, or blank to keep them.')
      return
    }
    setError('')
    try {
      await updateSettings({ retention_days: v })
      setDays(null)
    } catch (e) {
      setError((e as Error).message)
    }
  }
  const saveSettle = async () => {
    const v = settleValue().trim()
    if (v && !/^[1-9][0-9]*$/.test(v)) {
      setError('A whole number of days, or blank to never auto-settle.')
      return
    }
    setError('')
    try {
      await updateSettings({ auto_settle_days: v })
      setSettleDays(null)
    } catch (e) {
      setError((e as Error).message)
    }
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
          <span class="flex-1">Auto-settle idle threads after</span>
          <div class="flex items-center gap-2">
            <input
              aria-label="Auto-settle idle threads after days"
              class={`${inputClass} w-20 text-right tabular-nums`}
              inputmode="numeric"
              placeholder="never"
              value={settleValue()}
              onInput={(e) => setSettleDays(e.currentTarget.value)}
              onKeyDown={(e) => e.key === 'Enter' && void saveSettle()}
            />
            <span class="text-mute">days</span>
            <Btn small disabled={settleDays() === null} onClick={() => void saveSettle()}>
              Save
            </Btn>
          </div>
        </div>
        <div class="flex flex-wrap items-center gap-x-3 gap-y-2 px-3.5 py-3 text-[13px]">
          <span class="flex-1">Empty the trash after</span>
          <div class="flex items-center gap-2">
            <input
              aria-label="Empty the trash after days"
              class={`${inputClass} w-20 text-right tabular-nums`}
              inputmode="numeric"
              placeholder="never"
              value={value()}
              onInput={(e) => setDays(e.currentTarget.value)}
              onKeyDown={(e) => e.key === 'Enter' && void saveRetention()}
            />
            <span class="text-mute">days</span>
            <Btn small disabled={days() === null} onClick={() => void saveRetention()}>
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
  const run = (p: Promise<unknown>) => p.catch((e) => setError((e as Error).message))
  return (
    <Section title="Trash" hint="Threads you moved to the trash. They stay here until you delete them or the trash is emptied.">
      <Card>
        <For each={threads()} fallback={<div class="px-3.5 py-3 text-[13px] text-mute">The trash is empty.</div>}>
          {(t) => (
            <div class="flex items-center gap-2 px-3.5 py-2.5">
              <div class="min-w-0 flex-1">
                <div class="truncate text-[13.5px]">{t.title}</div>
                <div class="text-[12px] text-faint">
                  {state.projects[t.project]?.name ?? 'No project'} · trashed {ago(t.hidden_at!)} ago
                </div>
              </div>
              <Btn small onClick={() => void run(call({ op: 'update_thread', thread: t.id, hidden: false }))}>
                Restore
              </Btn>
              <InlineConfirm
                label="Delete"
                confirmLabel="Delete forever"
                danger
                onConfirm={() => run(call({ op: 'delete_thread', thread: t.id }))}
              />
            </div>
          )}
        </For>
      </Card>
      <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
    </Section>
  )
}
