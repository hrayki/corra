import type { StoredEvent } from '../proto/StoredEvent'
import type { FindHit } from '../proto/FindHit'
import type { Entry, ThreadView } from './timeline'

export type Spot = { hit: number; at: number }

function fold(text: string) {
  let folded = ''
  const from: number[] = []
  const to: number[] = []
  let i = 0
  for (const ch of text) {
    const low = ch.toLowerCase()
    for (let k = 0; k < low.length; k++) {
      from.push(i)
      to.push(i + ch.length)
    }
    folded += low
    i += ch.length
  }
  return { folded, from, to }
}

export function occurrences(text: string, query: string): [number, number][] {
  const q = fold(query).folded
  if (!q) return []
  const { folded, from, to } = fold(text)
  const out: [number, number][] = []
  for (let i = folded.indexOf(q); i >= 0; i = folded.indexOf(q, i + q.length)) out.push([from[i], to[i + q.length - 1]])
  return out
}

export const total = (hits: FindHit[]) => hits.reduce((n, h) => n + h.n, 0)

export function spotAt(hits: FindHit[], index: number): Spot | null {
  if (index < 0) return null
  let left = index
  for (let i = 0; i < hits.length; i++) {
    if (left < hits[i].n) return { hit: i, at: left }
    left -= hits[i].n
  }
  return null
}

export function indexAt(hits: FindHit[], s: Spot): number {
  let n = 0
  for (let i = 0; i < s.hit; i++) n += hits[i].n
  return n + Math.min(s.at, hits[s.hit].n - 1)
}

export function step(index: number, count: number, by: 1 | -1): number {
  if (!count) return -1
  if (index < 0) return by > 0 ? 0 : count - 1
  return (index + by + count) % count
}

export function label(index: number, hits: FindHit[], capped: boolean): string {
  const n = total(hits)
  if (!n) return 'No results'
  return `${Math.max(index, 0) + 1} of ${n}${capped ? '+' : ''}`
}

export function keep(old: FindHit[], index: number, fresh: FindHit[]): number {
  const s = spotAt(old, index)
  if (!s) return fresh.length ? 0 : -1
  const seq = old[s.hit].seq
  const same = fresh.findIndex((h) => h.seq === seq)
  if (same >= 0) return indexAt(fresh, { hit: same, at: s.at })
  const after = fresh.findIndex((h) => h.seq > seq)
  if (after >= 0) return indexAt(fresh, { hit: after, at: 0 })
  return total(fresh) - 1
}

export function itemAt(events: StoredEvent[], seq: number): { turn: string; item: string } | undefined {
  let lo = 0
  let hi = events.length - 1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const e = events[mid]
    if (e.seq < seq) lo = mid + 1
    else if (e.seq > seq) hi = mid - 1
    else if (e.kind === 'turn_started') return { turn: e.turn, item: `${e.turn}:input` }
    else if (e.kind === 'item_completed') return { turn: e.turn, item: e.item.id }
    else return undefined
  }
  return undefined
}

export const holds = (entries: Entry[], item: string) => entries.some((e) => e.type === 'item' && e.item.id === item)

export const shows = (v: ThreadView | undefined, item: string) =>
  !!v?.turns.some((t) => (`${t.id}:input` === item && t.input.length > 0) || holds(t.entries, item))
