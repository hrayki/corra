import { createSignal } from 'solid-js'

const KEY = 'corra:seen'

function load(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}')
  } catch {
    return {}
  }
}

const [seen, setSeen] = createSignal<Record<string, number>>(load())

function save(next: Record<string, number>) {
  setSeen(next)
  try {
    localStorage.setItem(KEY, JSON.stringify(next))
  } catch {}
}

export function markSeen(thread: string, seq: number) {
  if (seen()[thread] !== seq) save({ ...seen(), [thread]: seq })
}

export function knowThreads(threads: { id: string; last_seq: number }[]) {
  const cur = seen()
  const fresh = threads.filter((t) => cur[t.id] === undefined)
  if (fresh.length) save({ ...cur, ...Object.fromEntries(fresh.map((t) => [t.id, t.last_seq])) })
}

export function unread(t: { id: string; last_seq: number; status: string }) {
  const at = seen()[t.id]
  return t.status === 'idle' && at !== undefined && t.last_seq > at
}

type Window = { name: string; used?: number; resets?: number; blocked: boolean }

export function usageWindows(limits: unknown): Window[] {
  const info = (limits as { rate_limit_info?: Record<string, unknown> })?.rate_limit_info
  if (!info) return []
  const one = (name: string, w: Record<string, unknown>): Window => {
    const u = typeof w.utilization === 'number' ? w.utilization : undefined
    const r = typeof w.resetsAt === 'number' ? w.resetsAt : undefined
    return {
      name: name === 'five_hour' ? '5-hour limit' : name === 'seven_day' ? 'Weekly limit' : name.replace(/_/g, ' '),
      used: u === undefined ? undefined : Math.round(u <= 1 ? u * 100 : u),
      resets: r === undefined ? undefined : r < 1e12 ? r * 1000 : r,
      blocked: w.status === 'rejected',
    }
  }
  const many = info.unifiedWindows as Record<string, Record<string, unknown>> | undefined
  if (many && typeof many === 'object') return Object.entries(many).map(([k, w]) => one(k, w))
  return [one(String(info.rateLimitType ?? 'limit'), info)]
}

export function limitedNow(usage: { limits: unknown } | null) {
  return !!usage && usageWindows(usage.limits).some((w) => w.blocked && (w.resets ?? 0) > Date.now())
}
