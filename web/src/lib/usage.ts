import { ago } from './format.ts'
import type { AgentUsage } from '../proto/AgentUsage.ts'
import type { UsageWindow } from '../proto/UsageWindow.ts'

export function activeWindows(usage: AgentUsage | undefined, now = Date.now()): UsageWindow[] {
  if (!usage) return []
  const out: UsageWindow[] = []
  for (const w of usage.windows) {
    if (w.resets_at !== undefined && w.resets_at <= now) {
      if (w.key === 'quota' || w.key === 'limit') continue
      out.push({
        key: w.key,
        label: w.label,
        used_pct: 0,
        blocked: false,
      })
      continue
    }
    out.push(w)
  }
  return out
}

export function limitedNow(usage: AgentUsage | undefined, now = Date.now()): boolean {
  return activeWindows(usage, now).some((w) => w.blocked)
}

export function topUsagePct(usage: AgentUsage | undefined, now = Date.now()): number {
  let top = 0
  for (const w of activeWindows(usage, now)) {
    const v = w.blocked ? 100 : Math.round(w.used_pct ?? 0)
    if (v > top) top = v
  }
  return Math.min(100, Math.max(0, top))
}

export function formatReset(resetsAt: number, now = Date.now()): string {
  const diff = resetsAt - now
  if (diff <= 0) return 'Resetting'
  const mins = Math.ceil(diff / 60_000)
  if (mins <= 1) return 'Resets in <1m'
  if (mins < 60) return `Resets in ${mins}m`
  if (diff < 24 * 3600_000) {
    const h = Math.floor(diff / 3600_000)
    const m = Math.floor((diff % 3600_000) / 60_000)
    const clock = new Date(resetsAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
    return m > 0 ? `Resets in ${h}h ${m}m · ${clock}` : `Resets in ${h}h · ${clock}`
  }
  const day = new Date(resetsAt).toLocaleString([], { weekday: 'short', hour: 'numeric', minute: '2-digit' })
  return `Resets ${day}`
}

export function formatUpdatedAgo(updatedAt: number, now = Date.now()): string {
  if (updatedAt <= 0) return 'Not checked yet'
  const s = Math.max(0, Math.round((now - updatedAt) / 1000))
  if (s < 30) return 'Updated just now'
  if (s < 60) return `Updated ${s}s ago`
  return `Updated ${ago(updatedAt)} ago`
}
