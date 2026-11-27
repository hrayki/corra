import { batch } from 'solid-js'
import type { Queued } from '../proto/Queued'
import type { InputPart } from '../proto/InputPart'
import { createStore, produce, reconcile } from 'solid-js/store'
import { Conn, type ConnState } from './conn'
import { cache } from './cache'
import { checkBuild } from './version'
import { apply, applyDelta, buildView, emptyView, type ThreadView } from './timeline'
import type { Project } from '../proto/Project'
import type { Machine } from '../proto/Machine'
import type { Thread } from '../proto/Thread'
import type { HubToApp } from '../proto/HubToApp'
import type { StoredEvent } from '../proto/StoredEvent'
import type { AppCall } from '../proto/AppCall'
import type { Found } from '../proto/Found'

export type LiveStatus = {
  context?: { used_tokens: number; window_tokens?: number }
  background?: number
  agents?: string[]
  continue_at?: number
  rate_limit?: { rate_limit_info?: { unifiedWindows?: Record<string, { utilization: number; resetsAt: number; status: string }> } }
}

type State = {
  boot: 'loading' | 'signin' | 'ready' | 'unreachable'
  conn: ConnState | 'none'
  loaded: boolean
  projects: Record<string, Project>
  machines: Record<string, Machine>
  threads: Record<string, Thread>
  views: Record<string, ThreadView>
  earlier: Record<string, number | null>
  live: Record<string, LiveStatus>
  queues: Record<string, Queued[]>
  usage: { at: number; limits: unknown } | null
  pending: Record<string, { id: number; parts: InputPart[]; queued: boolean; client?: string }[]>
  restore: Record<string, string[]>
}

const [state, setState] = createStore<State>({
  boot: 'loading',
  conn: 'none',
  loaded: false,
  projects: {},
  machines: {},
  threads: {},
  views: {},
  earlier: {},
  live: {},
  queues: {},
  usage: null,
  pending: {},
  restore: {},
})
export { state }

let conn: Conn | null = null
const raw = new Map<string, StoredEvent[]>()
const subscribed = new Set<string>()
const catching = new Map<string, StoredEvent[]>()
const TAIL = 4
const saveTimers = new Map<string, number>()

function persistEvents(thread: string) {
  if (saveTimers.has(thread)) return
  saveTimers.set(
    thread,
    window.setTimeout(() => {
      saveTimers.delete(thread)
      const evs = raw.get(thread)
      if (evs) void cache.setEvents(thread, evs)
    }, 1200),
  )
}

function persistSnapshot() {
  void cache.set('snapshot:hub', {
    projects: Object.values(state.projects),
    machines: Object.values(state.machines),
    threads: Object.values(state.threads),
  })
}

let pendingId = 0

export function addPending(thread: string, parts: InputPart[], queued = false, client?: string) {
  const id = ++pendingId
  setState('pending', thread, (l) => [...(l ?? []), { id, parts, queued, client }])
  return id
}

export function settlePending(thread: string, id: number, queued: boolean) {
  if (queued) dropPending(thread, id)
  else setState('pending', thread, (l) => (l ?? []).map((p) => (p.id === id ? { ...p, queued: false } : p)))
}

export function dropPending(thread: string, id: number) {
  setState('pending', thread, (l) => (l ?? []).filter((p) => p.id !== id))
}

const stoppedAt = new Map<string, number>()
const STOP_RETURNS = 60_000

export function restoreDraft(thread: string, texts: string[]) {
  const t = texts.filter((x) => x.trim())
  if (t.length) setState('restore', thread, (l) => [...(l ?? []), ...t])
}

export function takeRestored(thread: string): string[] {
  const l = state.restore[thread] ?? []
  if (l.length) setState('restore', thread, [])
  return l
}

export async function stopThread(thread: string) {
  stoppedAt.set(thread, Date.now())
  const r = await call<{ unsent?: Queued[] }>({ op: 'interrupt', thread })
  restoreDraft(
    thread,
    (r?.unsent ?? []).map((q) => textOf(q.input)),
  )
}

const textOf = (parts: InputPart[]) =>
  parts
    .filter((p) => p.type === 'text')
    .map((p) => (p.type === 'text' ? p.text : ''))
    .join('\n')

function returnUnread(thread: string, fresh: StoredEvent[]) {
  const at = stoppedAt.get(thread)
  if (!at) return
  if (Date.now() - at > STOP_RETURNS) {
    stoppedAt.delete(thread)
    return
  }
  const v = state.views[thread]
  if (!v) return
  const texts: string[] = []
  for (const e of fresh) {
    if (e.kind === 'turn_completed') stoppedAt.delete(thread)
    if (e.kind !== 'unread' || e.ts < at) continue
    for (const t of v.turns) {
      if (`${t.id}:input` === e.item) texts.push(textOf(t.input))
      for (const x of t.entries)
        if (x.type === 'item' && x.item.id === e.item && x.item.body.type === 'user_message') texts.push(textOf(x.item.body.content))
    }
  }
  restoreDraft(thread, texts)
}

function addEvents(thread: string, events: StoredEvent[]) {
  const list = raw.get(thread) ?? []
  const last = list.length ? list[list.length - 1].seq : 0
  const fresh = events.filter((e) => e.seq > last)
  if (!fresh.length) return
  const seen = new Set(list.flatMap((e) => (e.kind === 'item_completed' && e.item.type === 'user_message' ? [e.item.id] : [])))
  let arrived = 0
  for (const e of fresh)
    if (e.kind === 'item_completed' && e.item.type === 'user_message' && !seen.has(e.item.id)) {
      seen.add(e.item.id)
      arrived++
    }
  if (arrived && state.pending[thread]?.length)
    setState('pending', thread, (l) => {
      const sending = l.filter((p) => !p.queued)
      const gone = new Set(sending.slice(0, arrived).map((p) => p.id))
      return l.filter((p) => !gone.has(p.id))
    })
  list.push(...fresh)
  raw.set(thread, list)
  if (!state.views[thread]) setState('views', thread, emptyView())
  setState(
    'views',
    thread,
    produce((v) => {
      for (const e of fresh) apply(v, e)
    }),
  )
  returnUnread(thread, fresh)
  persistEvents(thread)
}

function resetThread(thread: string) {
  raw.delete(thread)
  void cache.setEvents(thread, [])
  setEarlier(thread, null)
  setState('views', thread, emptyView())
}

function setEarlier(thread: string, from: number | null) {
  setState('earlier', thread, from)
  void cache.set(`earlier:${thread}`, from)
}

function keepLive(from: ThreadView | undefined, to: ThreadView) {
  const last = from?.turns.at(-1)
  if (!last || last.outcome) return
  const t = to.turns.find((x) => x.id === last.id)
  if (!t) return
  for (const e of last.entries) {
    if (e.type !== 'item' || (!e.item.live && !e.item.liveInput)) continue
    const it = t.entries.find((x) => x.type === 'item' && x.item.id === e.item.id)
    if (it?.type === 'item' && !it.item.done) {
      it.item.live = e.item.live
      it.item.liveInput = e.item.liveInput
    }
  }
}

function showHistory(thread: string, events: StoredEvent[], earlier: number | null) {
  raw.set(thread, events)
  const v = buildView(events, earlier !== null)
  keepLive(state.views[thread], v)
  setState('views', thread, reconcile(v, { key: 'id', merge: false }))
  setEarlier(thread, earlier)
  persistEvents(thread)
}

function subscribe(id: string) {
  if (!conn) return
  if (!catching.has(id)) catching.set(id, [])
  void conn
    .call({ op: 'subscribe', thread: id, after_seq: raw.get(id)?.at(-1)?.seq ?? 0, tail: TAIL })
    .catch(() => {})
    .finally(() => {
      const caught = catching.get(id)
      catching.delete(id)
      if (caught?.length)
        addEvents(
          id,
          caught.sort((a, b) => a.seq - b.seq),
        )
    })
}

const fetching = new Map<string, Promise<void>>()

export function loadEarlier(id: string, toSeq?: number): Promise<void> {
  const busy = fetching.get(id)
  if (busy) return toSeq === undefined ? busy : busy.then(() => loadEarlier(id, toSeq))
  const from = state.earlier[id]
  if (from == null) return Promise.resolve()
  const load = (async () => {
    const r = await call<{ events: StoredEvent[]; from_seq: number; earlier: boolean }>({
      op: 'earlier',
      thread: id,
      before_seq: from,
      to_seq: toSeq,
    })
    if (state.earlier[id] !== from) return
    const have = raw.get(id) ?? []
    const seen = new Set(have.map((e) => e.seq))
    const merged = [...r.events.filter((e) => !seen.has(e.seq)), ...have].sort((a, b) => a.seq - b.seq)
    showHistory(id, merged, r.earlier ? r.from_seq : null)
  })().finally(() => fetching.delete(id))
  fetching.set(id, load)
  return load
}

export const loadingEarlier = (id: string) => fetching.get(id)

export function findIn(id: string, query: string) {
  return call<Found>({ op: 'find', thread: id, query })
}

function putThread(t: Thread) {
  setState('threads', t.id, {
    machine: undefined,
    session_id: undefined,
    pinned_at: undefined,
    settled_at: undefined,
    hidden_at: undefined,
    ...t,
  })
}

function applySnapshot(m: Extract<HubToApp, { t: 'snapshot' }>) {
  batch(() => {
    setState(
      produce((s) => {
        for (const k of Object.keys(s.projects)) delete s.projects[k]
        for (const k of Object.keys(s.machines)) delete s.machines[k]
        const keep = new Set(m.threads.map((t) => t.id))
        for (const k of Object.keys(s.threads)) if (!keep.has(k)) delete s.threads[k]
        for (const p of m.projects) s.projects[p.id] = p
        for (const x of m.machines) s.machines[x.id] = x
        s.loaded = true
      }),
    )
    for (const t of m.threads) putThread(t)
  })
  persistSnapshot()
}

function onMessage(m: HubToApp) {
  switch (m.t) {
    case 'snapshot':
      checkBuild(m.build)
      applySnapshot(m)
      break
    case 'project':
      setState('projects', m.project.id, m.project)
      break
    case 'machine':
      setState('machines', m.machine.id, m.machine)
      break
    case 'thread':
      putThread(m.thread)
      break
    case 'thread_removed':
      setState('threads', m.thread, undefined!)
      break
    case 'reset':
      resetThread(m.thread)
      if (subscribed.has(m.thread)) subscribe(m.thread)
      break
    case 'events':
      if (catching.has(m.thread)) catching.get(m.thread)!.push(...m.events)
      else addEvents(m.thread, m.events)
      break
    case 'window':
      showHistory(m.thread, [...m.events], m.earlier ? m.from_seq : null)
      break
    case 'delta': {
      const d = m.delta
      if (!state.views[d.thread]) setState('views', d.thread, emptyView())
      setState(
        'views',
        d.thread,
        produce((v) => applyDelta(v, d.turn, d.item, d.parent ?? undefined, d.delta)),
      )
      break
    }
    case 'status':
      setState('live', m.thread, m.status as LiveStatus)
      break
    case 'usage':
      setState('usage', { at: m.at, limits: m.limits })
      break
    case 'queue':
      setState('queues', m.thread, m.items)
      setState('pending', m.thread, (l) => (l ?? []).filter((p) => !p.queued))
      break
    case 'not_sent': {
      const p = state.pending[m.thread]?.find((x) => x.client === m.client_id)
      if (p) {
        dropPending(m.thread, p.id)
        restoreDraft(m.thread, [textOf(p.parts)])
      }
      break
    }
    case 'term_output':
      termListeners.get(m.term)?.data(b64bytes(m.data))
      break
    case 'term_exit':
      termListeners.get(m.term)?.exit()
      break
    case 'reply':
      break
  }
}

type TermListener = { data: (bytes: Uint8Array) => void; exit: () => void }
const termListeners = new Map<string, TermListener>()
export function listenTerm(term: string, l: TermListener) {
  termListeners.set(term, l)
  return () => termListeners.delete(term)
}
export function b64bytes(s: string): Uint8Array {
  const bin = atob(s)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

function startConn(url: () => string, onRefused?: () => void) {
  conn?.close()
  const c = new Conn(
    url,
    onMessage,
    () => {
      for (const t of subscribed) subscribe(t)
    },
    onRefused,
  )
  conn = c
  const track = () => {
    if (conn !== c) return
    setState('conn', c.state())
    setTimeout(track, 400)
  }
  track()
}

const sameOrigin = () => `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}`

async function loadCached() {
  const s = await cache.get<{ projects: Project[]; machines: Machine[]; threads: Thread[] }>('snapshot:hub')
  if (!s || state.loaded) return
  batch(() => {
    for (const p of s.projects) if (!state.projects[p.id]) setState('projects', p.id, p)
    for (const m of s.machines) if (!state.machines[m.id]) setState('machines', m.id, m)
    for (const t of s.threads) if (!state.threads[t.id]) setState('threads', t.id, t)
  })
}

export async function boot() {
  let auth: { signed_in: boolean }
  try {
    const token = localStorage.getItem('corra_token')
    const r = await fetch('/auth/state', { credentials: 'include', headers: token ? { authorization: `Bearer ${token}` } : {} })
    auth = await r.json()
  } catch {
    setState('boot', 'unreachable')
    return
  }
  if (!auth.signed_in) {
    setState('boot', 'signin')
    return
  }
  void loadCached()
  startConn(
    () => {
      const token = localStorage.getItem('corra_token')
      return `${sameOrigin()}/app${token ? `?token=${encodeURIComponent(token)}` : ''}`
    },
    () => {
      void fetch('/auth/state', { credentials: 'include' })
        .then((r) => r.json())
        .then((a) => {
          if (!a.signed_in && !localStorage.getItem('corra_token')) setState('boot', 'signin')
        })
        .catch(() => {})
    },
  )
  setState('boot', 'ready')
}

export function call<T = unknown>(c: AppCall): Promise<T> {
  if (!conn) return Promise.reject(new Error("The hub isn't connected"))
  return conn.call<T>(c)
}

export async function openThread(id: string) {
  if (subscribed.has(id)) return
  subscribed.add(id)
  if (!raw.has(id)) {
    const cached = await cache.events(id)
    const from = (await cache.get<number | null>(`earlier:${id}`)) ?? null
    if (cached?.length && !raw.has(id)) {
      let v: ThreadView | undefined
      try {
        v = buildView(cached, from !== null)
      } catch {
        void cache.setEvents(id, [])
        void cache.set(`earlier:${id}`, null)
      }
      if (v) {
        raw.set(id, cached)
        setState('views', id, v)
        setState('earlier', id, from)
      }
    }
  }
  subscribe(id)
}

export function closeThread(id: string) {
  if (!subscribed.delete(id)) return
  void conn?.call({ op: 'unsubscribe', thread: id }).catch(() => {})
}

export const eventsOf = (id: string) => raw.get(id) ?? []
