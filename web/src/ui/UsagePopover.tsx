import { For, Show, createMemo, createSignal, onCleanup, type JSX } from 'solid-js'
import { Portal } from 'solid-js/web'
import RefreshCw from 'lucide-solid/icons/refresh-cw'
import { refreshUsage, state } from '../lib/store'
import { activeWindows, formatReset, formatUpdatedAgo, limitedNow, topUsagePct } from '../lib/usage'
import type { AgentKind } from '../proto/AgentKind'
import { AgentIcon } from './bits'

const PROVIDERS: { agent: AgentKind; name: string }[] = [
  { agent: 'claude', name: 'Claude Code' },
  { agent: 'antigravity', name: 'Antigravity' },
]

export function UsageMeter() {
  const [open, setOpen] = createSignal(false)
  const [now, setNow] = createSignal(Date.now())
  const [refreshing, setRefreshing] = createSignal<Partial<Record<AgentKind, boolean>>>({})
  const [localError, setLocalError] = createSignal<Partial<Record<AgentKind, string>>>({})
  const [pos, setPos] = createSignal({ x: 0, y: 0, w: 0, h: 0 })
  let btn!: HTMLButtonElement

  const anyBlocked = createMemo(() => PROVIDERS.some((p) => limitedNow(state.usage[p.agent], now())))
  const hasAnyWindow = createMemo(() => PROVIDERS.some((p) => activeWindows(state.usage[p.agent], now()).length > 0))
  const worstPct = createMemo(() => Math.max(0, ...PROVIDERS.map((p) => topUsagePct(state.usage[p.agent], now()))))

  const close = () => {
    setOpen(false)
    document.removeEventListener('keydown', onKey)
  }
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') close()
  }
  onCleanup(() => document.removeEventListener('keydown', onKey))

  const toggle = () => {
    if (open()) return close()
    setNow(Date.now())
    setLocalError({})
    const r = btn.getBoundingClientRect()
    setPos({ x: r.left, y: r.top, w: r.width, h: r.height })
    setOpen(true)
    document.addEventListener('keydown', onKey)
  }

  const refresh = async (agent: AgentKind) => {
    if (refreshing()[agent]) return
    setRefreshing((m) => ({ ...m, [agent]: true }))
    setLocalError((m) => ({ ...m, [agent]: '' }))
    try {
      await refreshUsage(agent)
      setNow(Date.now())
    } catch (e) {
      setLocalError((m) => ({ ...m, [agent]: e instanceof Error ? e.message : String(e) }))
    } finally {
      setRefreshing((m) => ({ ...m, [agent]: false }))
    }
  }

  const style = (): JSX.CSSProperties => {
    const p = pos()
    const vw = window.innerWidth
    const width = Math.min(316, vw - 16)
    const left = Math.max(8, Math.min(p.x, vw - width - 8))
    return {
      position: 'fixed',
      top: `${p.y + p.h + 6}px`,
      left: `${left}px`,
      width: `${width}px`,
    }
  }

  const R = 7
  const C = 2 * Math.PI * R
  const ringStroke = () => {
    if (anyBlocked() || worstPct() >= 90) return 'var(--bad)'
    if (worstPct() >= 70) return 'var(--warn)'
    if (worstPct() > 0) return 'var(--ink)'
    return 'var(--mute)'
  }
  const barTone = (pct: number, isBlocked: boolean) => {
    if (isBlocked || pct >= 90) return 'bg-bad'
    if (pct >= 70) return 'bg-warn'
    return 'bg-ink/75'
  }
  const triggerLabel = () => {
    if (anyBlocked()) return 'Usage: limit reached'
    if (hasAnyWindow()) return `Usage: ${worstPct()}%`
    return 'Usage limits'
  }

  return (
    <Show when={state.loaded}>
      <>
        <button
          ref={btn}
          type="button"
          aria-label={triggerLabel()}
          title={triggerLabel()}
          aria-haspopup="dialog"
          aria-expanded={open()}
          onClick={toggle}
          class="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-[12.5px] text-mute transition-colors duration-100 hover:bg-ink/[0.06] hover:text-ink"
          classList={{ 'bg-ink/[0.07] text-ink': open() }}
        >
          <svg viewBox="0 0 20 20" class="size-[18px] -rotate-90" aria-hidden="true">
            <circle cx="10" cy="10" r={R} fill="none" stroke="var(--line)" stroke-width="2.4" />
            <circle
              cx="10"
              cy="10"
              r={R}
              fill="none"
              stroke={ringStroke()}
              stroke-width="2.4"
              stroke-linecap="round"
              stroke-dasharray={`${(C * Math.min(100, worstPct())) / 100} ${C}`}
            />
          </svg>
          <Show when={anyBlocked() || hasAnyWindow()}>
            <span
              class="num font-medium"
              classList={{
                'text-bad': anyBlocked() || worstPct() >= 90,
                'text-warn': !anyBlocked() && worstPct() >= 70 && worstPct() < 90,
              }}
            >
              {anyBlocked() ? 'Limited' : `${worstPct()}%`}
            </span>
          </Show>
        </button>

        <Show when={open()}>
          <Portal>
            <div class="fixed inset-0 z-40" onClick={close} />
            <div
              role="dialog"
              aria-label="Usage limits"
              style={style()}
              class="z-50 max-h-[80vh] overflow-y-auto rounded-xl border border-line bg-raised p-4 text-[13px] shadow-[var(--shadow)]"
            >
              <div class="text-[12px] font-medium tracking-wide text-faint uppercase">Usage limits</div>

              <div class="mt-3 space-y-4">
                <For each={PROVIDERS}>
                  {(p, i) => {
                    const usage = () => state.usage[p.agent]
                    const windows = createMemo(() => activeWindows(usage(), now()))
                    const busy = () => !!refreshing()[p.agent]
                    const err = () => localError()[p.agent] || usage()?.error || ''
                    const updated = () => usage()?.updated_at ?? 0
                    return (
                      <section classList={{ 'border-t border-line pt-3.5': i() > 0 }}>
                        <div class="flex items-center justify-between gap-2">
                          <div class="flex items-center gap-2 text-[13.5px] font-semibold text-ink">
                            <AgentIcon agent={p.agent} size={14} class="text-mute" />
                            <span>{p.name}</span>
                          </div>
                          <Show
                            when={usage()?.refreshable}
                            fallback={
                              <Show when={updated() > 0}>
                                <span class="text-[11px] text-faint">{formatUpdatedAgo(updated(), now())}</span>
                              </Show>
                            }
                          >
                            <button
                              type="button"
                              disabled={busy()}
                              onClick={() => void refresh(p.agent)}
                              aria-label={`Refresh ${p.name} usage`}
                              class="inline-flex h-7 items-center gap-1.5 rounded-lg border border-line px-2 text-[11.5px] font-medium text-mute transition-colors hover:border-faint hover:text-ink disabled:opacity-50"
                            >
                              <RefreshCw size={11.5} class={busy() ? 'animate-spin' : ''} />
                              <span>{busy() ? 'Checking' : 'Refresh'}</span>
                            </button>
                          </Show>
                        </div>

                        <div class="mt-2.5 space-y-2.5">
                          <For each={windows()}>
                            {(w) => {
                              const pct = () => (w.blocked ? 100 : Math.min(100, Math.max(0, Math.round(w.used_pct ?? 0))))
                              return (
                                <div>
                                  <div class="flex items-baseline justify-between gap-2">
                                    <span class="text-[12.5px] font-medium text-ink/90">{w.label}</span>
                                    <span
                                      class="num text-[12px] font-medium"
                                      classList={{
                                        'text-bad': w.blocked || pct() >= 90,
                                        'text-warn': !w.blocked && pct() >= 70 && pct() < 90,
                                        'text-ink': !w.blocked && pct() < 70,
                                      }}
                                    >
                                      {w.blocked ? 'Limited' : w.used_pct !== undefined ? `${pct()}%` : '—'}
                                    </span>
                                  </div>
                                  <div class="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-ink/[0.08]">
                                    <div
                                      class={`h-full rounded-full transition-[width] duration-200 ${barTone(pct(), w.blocked)}`}
                                      style={{ width: `${pct()}%` }}
                                    />
                                  </div>
                                  <Show when={w.resets_at !== undefined || w.detail}>
                                    <div class="mt-1 flex flex-col gap-0.5 text-[11px] text-faint">
                                      <Show when={w.resets_at !== undefined}>
                                        <span>{formatReset(w.resets_at!, now())}</span>
                                      </Show>
                                      <Show when={w.detail}>
                                        <span class="line-clamp-2 text-mute" title={w.detail}>
                                          {w.detail}
                                        </span>
                                      </Show>
                                    </div>
                                  </Show>
                                </div>
                              )
                            }}
                          </For>

                          <Show when={!windows().length}>
                            <p class="text-[12px] leading-relaxed text-mute">
                              {p.agent === 'claude'
                                ? 'Updates live during Claude turns.'
                                : usage()?.refreshable
                                  ? 'No usage checked yet. Press Refresh to check.'
                                  : 'Updates after turns when an Antigravity machine is awake.'}
                            </p>
                          </Show>
                        </div>

                        <Show when={err()}>
                          <div class="mt-2.5 rounded-lg border border-warn/30 bg-warn/[0.08] px-2.5 py-1.5 text-[11.5px] leading-snug text-ink/90">
                            {err()}
                          </div>
                        </Show>

                        <Show when={usage()?.refreshable && updated() > 0}>
                          <div class="mt-2 text-[11px] text-faint">{formatUpdatedAgo(updated(), now())}</div>
                        </Show>
                      </section>
                    )
                  }}
                </For>
              </div>
            </div>
          </Portal>
        </Show>
      </>
    </Show>
  )
}
