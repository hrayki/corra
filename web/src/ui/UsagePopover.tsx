import { For, Show, createMemo, createSignal, onCleanup, type JSX } from 'solid-js'
import { Portal } from 'solid-js/web'
import RefreshCw from 'lucide-solid/icons/refresh-cw'
import { refreshUsage, state } from '../lib/store'
import { activeWindows, formatReset, formatUpdatedAgo, limitedNow, topUsagePct } from '../lib/usage'
import type { AgentKind } from '../proto/AgentKind'

const AGENT_NAME: Record<AgentKind, string> = {
  claude: 'Claude Code',
  antigravity: 'Antigravity',
}

export function UsageMeter(props: { agent: AgentKind }) {
  const [open, setOpen] = createSignal(false)
  const [now, setNow] = createSignal(Date.now())
  const [refreshing, setRefreshing] = createSignal(false)
  const [localError, setLocalError] = createSignal('')
  const [pos, setPos] = createSignal({ x: 0, y: 0, w: 0, h: 0 })
  let btn!: HTMLButtonElement

  const usage = () => state.usage[props.agent]
  const windows = createMemo(() => activeWindows(usage(), now()))
  const blocked = () => limitedNow(usage(), now())
  const top = () => topUsagePct(usage(), now())
  const show = () => {
    const u = usage()
    return !!u && (windows().length > 0 || !!u.error || u.refreshable)
  }

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
    setLocalError('')
    const r = btn.getBoundingClientRect()
    setPos({ x: r.left, y: r.top, w: r.width, h: r.height })
    setOpen(true)
    document.addEventListener('keydown', onKey)
  }

  const refresh = async () => {
    if (refreshing()) return
    setRefreshing(true)
    setLocalError('')
    try {
      await refreshUsage(props.agent)
      setNow(Date.now())
    } catch (e) {
      setLocalError(e instanceof Error ? e.message : String(e))
    } finally {
      setRefreshing(false)
    }
  }

  const style = (): JSX.CSSProperties => {
    const p = pos()
    const vw = window.innerWidth
    const width = Math.min(288, vw - 16)
    const left = Math.max(8, Math.min(p.x + p.w - width, vw - width - 8))
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
    if (blocked() || top() >= 90) return 'var(--bad)'
    if (top() >= 70) return 'var(--warn)'
    if (top() > 0) return 'var(--ink)'
    return 'var(--mute)'
  }
  const barTone = (pct: number, isBlocked: boolean) => {
    if (isBlocked || pct >= 90) return 'bg-bad'
    if (pct >= 70) return 'bg-warn'
    return 'bg-ink/70'
  }
  const triggerLabel = () => {
    if (blocked()) return `${AGENT_NAME[props.agent]} usage: limit reached`
    if (windows().length) return `${AGENT_NAME[props.agent]} usage: ${top()}%`
    return `${AGENT_NAME[props.agent]} usage`
  }
  const errorText = () => localError() || usage()?.error || ''

  return (
    <Show when={show()}>
      <>
        <button
          ref={btn}
          type="button"
          aria-label={triggerLabel()}
          title={triggerLabel()}
          aria-haspopup="dialog"
          aria-expanded={open()}
          onClick={toggle}
          class="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-[12px] text-mute transition-colors duration-100 hover:bg-ink/5 hover:text-ink"
          classList={{ 'bg-ink/5 text-ink': open() }}
        >
          <svg viewBox="0 0 20 20" class="size-[18px] -rotate-90" aria-hidden="true">
            <circle cx="10" cy="10" r={R} fill="none" stroke="var(--line)" stroke-width="2.5" />
            <circle
              cx="10"
              cy="10"
              r={R}
              fill="none"
              stroke={ringStroke()}
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-dasharray={`${(C * Math.min(100, top())) / 100} ${C}`}
            />
          </svg>
          <span
            class="num max-sm:hidden"
            classList={{ 'font-medium text-bad': blocked() || top() >= 90, 'text-warn': !blocked() && top() >= 70 && top() < 90 }}
          >
            {blocked() ? 'Limited' : windows().length ? `${top()}%` : 'Usage'}
          </span>
        </button>

        <Show when={open()}>
          <Portal>
            <div class="fixed inset-0 z-40" onClick={close} />
            <div
              role="dialog"
              aria-label={`${AGENT_NAME[props.agent]} usage`}
              style={style()}
              class="z-50 rounded-xl border border-line bg-raised p-3.5 text-[13px] shadow-[var(--shadow)]"
            >
              <div class="flex items-center justify-between gap-2">
                <div>
                  <div class="text-[13px] font-semibold text-ink">{AGENT_NAME[props.agent]}</div>
                  <div class="text-[11px] text-faint">Usage limits</div>
                </div>
                <Show when={usage()?.refreshable}>
                  <button
                    type="button"
                    disabled={refreshing()}
                    onClick={refresh}
                    aria-label="Refresh usage"
                    class="inline-flex h-7 items-center gap-1.5 rounded-lg border border-line px-2 text-[11.5px] font-medium text-mute transition-colors hover:border-faint hover:text-ink disabled:opacity-50"
                  >
                    <RefreshCw size={12} class={refreshing() ? 'animate-spin' : ''} />
                    <span>{refreshing() ? 'Checking' : 'Refresh'}</span>
                  </button>
                </Show>
              </div>

              <div class="mt-3 space-y-3">
                <For each={windows()}>
                  {(w) => {
                    const pct = () => (w.blocked ? 100 : Math.min(100, Math.max(0, Math.round(w.used_pct ?? 0))))
                    return (
                      <div>
                        <div class="flex items-baseline justify-between gap-2">
                          <span class="text-[12.5px] font-medium text-ink">{w.label}</span>
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
                    {usage()?.refreshable ? 'No usage fetched yet. Press Refresh to check.' : 'No quota limits reported.'}
                  </p>
                </Show>
              </div>

              <Show when={errorText()}>
                <div class="mt-3 rounded-lg border border-warn/30 bg-warn/[0.08] px-2.5 py-1.5 text-[11.5px] leading-snug text-ink/90">
                  {errorText()}
                </div>
              </Show>

              <div class="mt-3 flex items-center justify-between border-t border-line pt-2 text-[11px] text-faint">
                <span>{formatUpdatedAgo(usage()?.updated_at ?? 0, now())}</span>
                <Show when={refreshing()}>
                  <span>Updating…</span>
                </Show>
              </div>
            </div>
          </Portal>
        </Show>
      </>
    </Show>
  )
}
