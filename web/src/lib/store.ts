import { batch } from 'solid-js'
import type { Queued } from '../proto/Queued'
import type { InputPart } from '../proto/InputPart'
import { createStore, produce, reconcile } from 'solid-js/store'
import { Conn, type ConnState } from './conn'
import { cache } from './cache'
import { checkBuild } from './version'
import { setOmarchyTheme } from './theme'
import { apply, applyDelta, buildView, emptyView, type ThreadView } from './timeline'
import type { Project } from '../proto/Project'
import type { Machine } from '../proto/Machine'
import type { Thread } from '../proto/Thread'
import type { HubToApp } from '../proto/HubToApp'
import type { StoredEvent } from '../proto/StoredEvent'
import type { AppCall } from '../proto/AppCall'

export type Src = 'hub'
export type ProjectX = Project & { src: Src }
export type MachineX = Machine & { src: Src }
export type ThreadX = Thread & { src: Src }

export type LiveStatus = {
  context?: { used_tokens: number; window_tokens?: number }
  background?: number
  agents?: string[]
  continue_at?: number
  cache?: { tokens: number; until: number; compact: boolean }
  rate_limit?: { rate_limit_info?: { unifiedWindows?: Record<string, { utilization: number; resetsAt: number; status: string }> } }
}

type State = {
  boot: 'loading' | 'signin' | 'ready' | 'unreachable'
  desktop: boolean
  thisMachine: string | null
  offline: string | null
  connectUrl: string | null
  conn: Record<Src, ConnState | 'none'>
  loaded: Record<Src, boolean>
  projects: Record<string, ProjectX>
  machines: Record<string, MachineX>
  threads: Record<string, ThreadX>
  views: Record<string, ThreadView>
  earlier: Record<string, number | null>
  live: Record<string, LiveStatus>
  queues: Record<string, Queued[]>
  usage: { at: number; limits: unknown } | null
  pending: Record<string, { id: number; parts: InputPart[]; queued: boolean }[]>
}

const [state, setState] = createStore<State>({
  boot: 'loading',
  desktop: false,
  thisMachine: null,
  offline: null,
  connectUrl: null,
  conn: { hub: 'none' },
  loaded: { hub: false },
  projects: {},
  machines: {},
  threads: {},
  views: {},
  earlier: {},
  live: {},
  queues: {},
  usage: null,
  pending: {},
})
export { state }

const conns: Partial<Record<Src, Conn>> = {}
const raw = new Map<string, StoredEvent[]>()
const subscribed = new Map<string, Src | null>()
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

function persistSnapshot(src: Src) {
  const of = <T extends { src: Src }>(r: Record<string, T>) => Object.values(r).filter((x) => x.src === src)
  void cache.set(`snapshot:${src}`, { projects: of(state.projects), machines: of(state.machines), threads: of(state.threads) })
}

let pendingId = 0

export function addPending(thread: string, parts: InputPart[], queued = false) {
  const id = ++pendingId
  setState('pending', thread, (l) => [...(l ?? []), { id, parts, queued }])
  return id
}

export function dropPending(thread: string, id: number) {
  setState('pending', thread, (l) => (l ?? []).filter((p) => p.id !== id))
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

function subscribe(src: Src, id: string) {
  const c = conns[src]
  if (!c) return
  if (!catching.has(id)) catching.set(id, [])
  void c
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

const fetching = new Set<string>()

export async function loadEarlier(id: string) {
  const from = state.earlier[id]
  const src = state.threads[id]?.src
  if (from == null || !src || fetching.has(id)) return
  fetching.add(id)
  try {
    const r = await callOn<{ events: StoredEvent[]; from_seq: number; earlier: boolean }>(src, {
      op: 'earlier',
      thread: id,
      before_seq: from,
    })
    if (state.earlier[id] !== from) return
    const have = raw.get(id) ?? []
    const seen = new Set(have.map((e) => e.seq))
    const merged = [...r.events.filter((e) => !seen.has(e.seq)), ...have].sort((a, b) => a.seq - b.seq)
    showHistory(id, merged, r.earlier ? r.from_seq : null)
  } finally {
    fetching.delete(id)
  }
}

function putThread(src: Src, t: Thread) {
  setState('threads', t.id, {
    machine: undefined,
    session_id: undefined,
    pinned_at: undefined,
    settled_at: undefined,
    hidden_at: undefined,
    ...t,
    src,
  })
  if (subscribed.has(t.id) && subscribed.get(t.id) !== src) resubscribe(t.id)
}

function applySnapshot(src: Src, m: Extract<HubToApp, { t: 'snapshot' }>) {
  batch(() => {
    setState(
      produce((s) => {
        for (const k of Object.keys(s.projects)) if (s.projects[k].src === src) delete s.projects[k]
        for (const k of Object.keys(s.machines)) if (s.machines[k].src === src) delete s.machines[k]
        const keep = new Set(m.threads.map((t) => t.id))
        for (const k of Object.keys(s.threads)) if (s.threads[k].src === src && !keep.has(k)) delete s.threads[k]
        for (const p of m.projects) s.projects[p.id] = { ...p, src }
        for (const x of m.machines) s.machines[x.id] = { ...x, src }
        s.loaded[src] = true
      }),
    )
    for (const t of m.threads) putThread(src, t)
  })
  persistSnapshot(src)
}

function onMessage(src: Src, m: HubToApp) {
  switch (m.t) {
    case 'snapshot':
      if (!state.offline) checkBuild(m.build)
      applySnapshot(src, m)
      break
    case 'project':
      setState('projects', m.project.id, { ...m.project, src })
      break
    case 'machine':
      setState('machines', m.machine.id, { ...m.machine, src })
      break
    case 'thread':
      putThread(src, m.thread)
      break
    case 'thread_removed':
      if (state.threads[m.thread]?.src === src) setState('threads', m.thread, undefined!)
      break
    case 'reset':
      if (state.threads[m.thread]?.src === src || !state.threads[m.thread]) {
        resetThread(m.thread)
        if (subscribed.has(m.thread)) subscribe(src, m.thread)
      }
      break
    case 'events':
      if (state.threads[m.thread]?.src !== src) break
      if (catching.has(m.thread)) catching.get(m.thread)!.push(...m.events)
      else addEvents(m.thread, m.events)
      break
    case 'window':
      if (state.threads[m.thread]?.src === src) showHistory(m.thread, [...m.events], m.earlier ? m.from_seq : null)
      break
    case 'delta': {
      const d = m.delta
      if (state.threads[d.thread]?.src !== src) break
      if (!state.views[d.thread]) setState('views', d.thread, emptyView())
      setState(
        'views',
        d.thread,
        produce((v) => applyDelta(v, d.turn, d.item, d.parent ?? undefined, d.delta)),
      )
      break
    }
    case 'status':
      if (state.threads[m.thread]?.src === src) setState('live', m.thread, m.status as LiveStatus)
      break
    case 'usage':
      setState('usage', { at: m.at, limits: m.limits })
      break
    case 'queue':
      if (state.threads[m.thread]?.src === src) {
        setState('queues', m.thread, m.items)
        setState('pending', m.thread, (l) => (l ?? []).filter((p) => !p.queued))
      }
      break
    case 'term_output':
      termListeners.get(m.term)?.data(b64bytes(m.data))
      break
    case 'term_exit':
      termListeners.get(m.term)?.exit()
      break
    case 'browser_frame':
      browserListeners.get(m.machine)?.(m)
      break
    case 'phone_video':
    case 'phone_frame':
      phoneListener?.(m)
      break
    case 'reply':
      break
  }
}

export type BrowserFrame = Extract<HubToApp, { t: 'browser_frame' }>
const browserListeners = new Map<string, (f: BrowserFrame) => void>()
export function listenBrowser(machine: string, l: (f: BrowserFrame) => void) {
  browserListeners.set(machine, l)
  return () => browserListeners.delete(machine)
}

export type PhoneMsg = Extract<HubToApp, { t: 'phone_video' | 'phone_frame' }>
let phoneListener: ((m: PhoneMsg) => void) | undefined
export function listenPhone(l: (m: PhoneMsg) => void) {
  phoneListener = l
  return () => {
    if (phoneListener === l) phoneListener = undefined
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

function startConn(src: Src, url: () => string, onRefused?: () => void) {
  conns[src]?.close()
  const c = new Conn(
    url,
    (m) => onMessage(src, m),
    () => {
      for (const [t, s] of subscribed) if (s === src) subscribe(src, t)
    },
    onRefused,
  )
  conns[src] = c
  const track = () => {
    if (conns[src] !== c) return
    setState('conn', src, c.state())
    setTimeout(track, 400)
  }
  track()
}

const sameOrigin = () => `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}`

const DESKTOP = 'corra:desktop'

function desktopHandOff() {
  const m = /^#desktop=(.*)$/.exec(location.hash)
  if (m) {
    try {
      const d = JSON.parse(decodeURIComponent(m[1])) as { token?: string; machine?: string; connect?: string; offline?: string }
      if (d.token) localStorage.setItem('corra_token', d.token)
      sessionStorage.setItem(
        DESKTOP,
        JSON.stringify({ machine: d.machine ?? null, connect: d.connect ?? null, offline: d.offline ?? null }),
      )
    } catch {}
    history.replaceState(null, '', `${location.pathname}${location.search}#/`)
  }
  try {
    const d = JSON.parse(sessionStorage.getItem(DESKTOP) ?? 'null') as {
      machine: string | null
      connect: string | null
      offline: string | null
    } | null
    if (d) setState({ desktop: true, thisMachine: d.machine, connectUrl: d.connect, offline: d.offline })
  } catch {}
}

;(window as unknown as { corraTheme: typeof setOmarchyTheme }).corraTheme = setOmarchyTheme

async function loadCached(src: Src) {
  const s = await cache.get<{ projects: Project[]; machines: Machine[]; threads: Thread[] }>(`snapshot:${src}`)
  if (!s || state.loaded[src]) return
  batch(() => {
    for (const p of s.projects) if (!state.projects[p.id]) setState('projects', p.id, { ...p, src })
    for (const m of s.machines) if (!state.machines[m.id]) setState('machines', m.id, { ...m, src })
    for (const t of s.threads) if (!state.threads[t.id]) setState('threads', t.id, { ...t, src })
  })
}

export async function boot() {
  desktopHandOff()
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
  void loadCached('hub')
  startConn(
    'hub',
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

export function callOn<T = unknown>(src: Src, c: AppCall): Promise<T> {
  const conn = conns[src]
  if (!conn) return Promise.reject(new Error("The hub isn't connected"))
  return conn.call<T>(c)
}

export const call = <T = unknown>(c: AppCall) => callOn<T>('hub', c)

function resubscribe(id: string) {
  if (!subscribed.has(id)) return
  const src = state.threads[id]?.src ?? null
  subscribed.set(id, src)
  if (src) subscribe(src, id)
}

export async function openThread(id: string) {
  if (subscribed.has(id)) return
  subscribed.set(id, null)
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
  resubscribe(id)
}

export function closeThread(id: string) {
  const src = subscribed.get(id)
  if (!subscribed.delete(id)) return
  if (src) void conns[src]?.call({ op: 'unsubscribe', thread: id }).catch(() => {})
}

export function threadSubscribedLate(id: string) {
  if (subscribed.has(id) && !subscribed.get(id)) resubscribe(id)
}

export const eventsOf = (id: string) => raw.get(id) ?? []
