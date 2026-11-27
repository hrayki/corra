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
