import { batch } from 'solid-js'
import type { Queued } from '../proto/Queued'
import type { InputPart } from '../proto/InputPart'
import { createStore, produce } from 'solid-js/store'
import { Conn, type ConnState } from './conn'
import { checkBuild } from './version'
import { apply, applyDelta, buildView, emptyView, patchItemDetail, type ThreadView } from './timeline'
import type { ItemBody } from '../proto/ItemBody'
import type { Project } from '../proto/Project'
import type { Machine } from '../proto/Machine'
import type { Thread } from '../proto/Thread'
import type { HubToApp } from '../proto/HubToApp'
import type { StoredEvent } from '../proto/StoredEvent'
import type { AppCall } from '../proto/AppCall'
import type { Found } from '../proto/Found'
import type { SetupLog } from '../proto/SetupLog'
import type { AgentKind } from '../proto/AgentKind'
import type { AgentUsage } from '../proto/AgentUsage'
import type { ProviderInfo } from '../proto/ProviderInfo'
import type { ToolInfo } from '../proto/ToolInfo'

export type ProjectRuntimeStatus = {
  stage?: string
  setup?: SetupLog
}

export type LiveStatus = {
  context?: { used_tokens: number; window_tokens?: number }
  background?: number
  agents?: string[]
  continue_at?: number
  rate_limit?: { rate_limit_info?: { unifiedWindows?: Record<string, { utilization: number; resetsAt: number; status: string }> } }
}

export type DraftRestore = {
  texts: string[]
  replace: boolean
}

type State = {
  boot: 'loading' | 'signin' | 'ready' | 'unreachable'
  conn: ConnState | 'none'
  loaded: boolean
  projects: Record<string, Project>
  projectStatus: Record<string, ProjectRuntimeStatus>
  machines: Record<string, Machine>
  threads: Record<string, Thread>
  views: Record<string, ThreadView>
  loading: Record<string, boolean>
  earlier: Record<string, number | null>
  live: Record<string, LiveStatus>
  queues: Record<string, Queued[]>
  usage: Partial<Record<AgentKind, AgentUsage>>
  settings: Record<string, unknown>
  providers: ProviderInfo[]
  tools: ToolInfo[]
  pending: Record<string, { id: number; parts: InputPart[]; queued: boolean; client?: string }[]>
  restore: Record<string, DraftRestore | undefined>
}

const [state, setState] = createStore<State>({
  boot: 'loading',
  conn: 'none',
  loaded: false,
  projects: {},
  projectStatus: {},
  machines: {},
  threads: {},
  views: {},
  loading: {},
  earlier: {},
  live: {},
  queues: {},
  usage: {},
  settings: {},
  providers: [],
  tools: [],
  pending: {},
  restore: {},
})
export { state }

let conn: Conn | null = null
const raw = new Map<string, StoredEvent[]>()
const subscribed = new Set<string>()
const catching = new Map<string, StoredEvent[]>()
const TAIL = 4

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

export function restoreDraft(thread: string, texts: string[], replace = false) {
  const clean = texts.filter((x) => x.trim())
  if (!clean.length) return
  setState('restore', thread, (prev) =>
    replace || !prev ? { texts: clean, replace } : { texts: [...prev.texts, ...clean], replace: prev.replace },
  )
}

export function takeRestored(thread: string): DraftRestore | null {
  const cur = state.restore[thread]
  if (!cur?.texts.length) return null
  setState('restore', thread, undefined)
  return cur
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
}

function resetThread(thread: string) {
  raw.set(thread, [])
  setState('views', thread, emptyView())
  setState('earlier', thread, null)
}

function showHistory(thread: string, events: StoredEvent[], from: number | null) {
  raw.set(thread, events)
  const hadTurn = events.some((e) => e.kind === 'turn_started' || (e.kind === 'item_completed' && e.item.type === 'user_message'))
  batch(() => {
    setState('views', thread, buildView(events, from !== null))
    setState('earlier', thread, from)
    setState('loading', thread, false)
    if (hadTurn && state.pending[thread]?.length) setState('pending', thread, (l) => (l ?? []).filter((p) => p.queued))
  })
}

function subscribe(id: string) {
  const had = raw.has(id)
  if (!had) {
    catching.set(id, [])
    setState('loading', id, true)
  }
  void conn
    ?.call({ op: 'subscribe', thread: id, after_seq: raw.get(id)?.at(-1)?.seq ?? 0, tail: TAIL })
    .then(() => {
      const caught = catching.get(id)
      catching.delete(id)
      if (!caught) return
      if (caught.length)
        addEvents(
          id,
          caught.sort((a, b) => a.seq - b.seq),
        )
      else if (!raw.has(id)) showHistory(id, [], null)
    })
    .catch(() => catching.delete(id))
    .finally(() => setState('loading', id, false))
}

const fetchingDetail = new Map<string, Promise<void>>()

export function loadItemDetail(thread: string, itemId: string, seq: number): Promise<void> {
  const key = `${thread}:${seq}`
  const existing = fetchingDetail.get(key)
  if (existing) return existing
  const p = (async () => {
    const full = await call<StoredEvent>({ op: 'event_detail', thread, seq })
    if (full.kind !== 'item_completed' && full.kind !== 'item_started') return
    const list = raw.get(thread)
    if (list) {
      const idx = list.findIndex((e) => e.seq === seq)
      if (idx >= 0) list[idx] = full
    }
    const { id: _id, parent: _parent, ...rest } = full.item
    const body = rest as ItemBody
    if (state.views[thread])
      setState(
        'views',
        thread,
        produce((v) => {
          patchItemDetail(v, itemId, body)
        }),
      )
  })().finally(() => fetchingDetail.delete(key))
  fetchingDetail.set(key, p)
  return p
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

export async function refreshUsage(agent?: AgentKind) {
  const r = await call<AgentUsage | AgentUsage[]>({ op: 'refresh_usage', ...(agent ? { agent } : {}) })
  const list = Array.isArray(r) ? r : r ? [r] : []
  batch(() => {
    for (const u of list) setState('usage', u.agent, u)
  })
  return list
}

export async function updateSettings(patch: Record<string, unknown>) {
  const next = await call<Record<string, unknown>>({ op: 'set_settings', settings: patch })
  if (next && typeof next === 'object') setState('settings', next)
  return next
}

export async function refreshCatalogState() {
  if (state.conn !== 'open') return
  const [providers, tools] = await Promise.all([
    call<ProviderInfo[]>({ op: 'list_providers' }).catch(() => state.providers),
    call<ToolInfo[]>({ op: 'list_tools' }).catch(() => state.tools),
  ])
  batch(() => {
    setState('providers', providers)
    setState('tools', tools)
  })
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
        const keepProjects = new Set(m.projects.map((p) => p.id))
        for (const k of Object.keys(s.projectStatus)) if (!keepProjects.has(k)) delete s.projectStatus[k]
        for (const k of Object.keys(s.machines)) delete s.machines[k]
        const keep = new Set(m.threads.map((t) => t.id))
        for (const k of Object.keys(s.threads)) if (!keep.has(k)) delete s.threads[k]
        for (const p of m.projects) s.projects[p.id] = p
        for (const x of m.machines) s.machines[x.id] = x
        if (m.settings && typeof m.settings === 'object') s.settings = m.settings
        if (Array.isArray(m.providers)) s.providers = m.providers
        if (Array.isArray(m.tools)) s.tools = m.tools
        s.loaded = true
      }),
    )
    for (const t of m.threads) putThread(t)
  })
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
    case 'project_removed':
      setState('projects', m.project, undefined!)
      setState('projectStatus', m.project, undefined!)
      break
    case 'project_status':
      setState('projectStatus', m.project, { stage: m.stage, setup: m.setup })
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
      setState('usage', m.usage.agent, m.usage)
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

let connGen = 0

function startConn(url: () => string, onRefused?: () => void) {
  conn?.close()
  const gen = ++connGen
  conn = new Conn(
    url,
    onMessage,
    () => {
      for (const t of subscribed) subscribe(t)
    },
    onRefused,
    (s) => {
      if (gen === connGen) setState('conn', s)
    },
  )
  setState('conn', conn.state())
}

const sameOrigin = () => `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}`

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

export function openThread(id: string) {
  if (subscribed.has(id)) return
  subscribed.add(id)
  subscribe(id)
}

export function closeThread(id: string) {
  if (!subscribed.delete(id)) return
  void conn?.call({ op: 'unsubscribe', thread: id }).catch(() => {})
}

export const eventsOf = (id: string) => raw.get(id) ?? []
