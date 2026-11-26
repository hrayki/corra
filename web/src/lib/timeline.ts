import type { StoredEvent } from '../proto/StoredEvent'
import type { ItemBody } from '../proto/ItemBody'
import type { InputPart } from '../proto/InputPart'
import type { AgentSettings } from '../proto/AgentSettings'
import type { AgentRequest } from '../proto/AgentRequest'
import type { RequestResponse } from '../proto/RequestResponse'
import type { TurnOutcome } from '../proto/TurnOutcome'
import type { Usage } from '../proto/Usage'
import type { Delta } from '../proto/Delta'
import type { NoticeLevel } from '../proto/NoticeLevel'
import type { RepoCheckpoint } from '../proto/RepoCheckpoint'
import type { Choice } from '../proto/Choice'

export type ItemView = {
  id: string
  parent?: string
  body: ItemBody
  done: boolean
  at?: number
  ended?: number
  live?: string
  liveInput?: string
  unread?: boolean
}

export type RequestView = {
  id: string
  request: AgentRequest
  response?: RequestResponse
}

export type Entry =
  | { type: 'item'; item: ItemView }
  | { type: 'request'; request: RequestView }
  | { type: 'notice'; level: NoticeLevel; text: string; ts: number }
  | { type: 'compacted'; trigger?: string }

export type TurnView = {
  id: string
  input: InputPart[]
  cause?: string
  settings: AgentSettings
  started: number
  entries: Entry[]
  after: Entry[]
  outcome?: TurnOutcome
  usage?: Usage
  error?: string
  resetsAt?: number
  ended?: number
  checkpoint?: RepoCheckpoint[]
  unread?: boolean
}

export type ThreadView = {
  lastSeq: number
  partial?: boolean
  sessionId?: string
  model?: string
  models?: Choice[]
  efforts?: Choice[]
  modes?: Choice[]
  permissionMode?: string
  slashCommands: string[]
  preface: Entry[]
  turns: TurnView[]
}

export const isWork = (e: Entry) =>
  (e.type === 'item' && (e.item.body.type === 'tool_call' || e.item.body.type === 'reasoning' || e.item.body.type === 'todos')) ||
  (e.type === 'request' && !!e.request.response)

const isThought = (e: Entry) => e.type === 'item' && e.item.body.type === 'reasoning'

export type Fold = { lead?: Entry; work: Entry[]; rest: Entry[] }

export function folds(turn: string, entries: Entry[]): Fold[] {
  const out: Fold[] = []
  let lead: Entry | undefined
  let cur: Entry[] = []
  const close = () => {
    let end = cur.length
    while (end > 0 && isThought(cur[end - 1])) end--
    const body = end > 0 ? cur.slice(0, end) : []
    const trailing = cur.slice(end)
    let last = -1
    body.forEach((e, i) => {
      if (isWork(e)) last = i
    })
    out.push({ lead, work: [...body.slice(0, last + 1), ...trailing], rest: body.slice(last + 1) })
  }
  for (const e of entries)
    if (e.type === 'item' && e.item.body.type === 'user_message' && e.item.id !== `${turn}:input`) {
      close()
      lead = e
      cur = []
    } else cur.push(e)
  close()
  return out
}

export const emptyView = (): ThreadView => ({ lastSeq: 0, slashCommands: [], preface: [], turns: [] })

function findTurn(v: ThreadView, id: string): TurnView | undefined {
  for (let i = v.turns.length - 1; i >= 0; i--) if (v.turns[i].id === id) return v.turns[i]
  return undefined
}

function ensureTurn(v: ThreadView, id: string, ts: number): TurnView {
  let t = findTurn(v, id)
  if (!t) {
    t = { id, input: [], settings: {}, started: ts, entries: [], after: [] }
    v.turns.push(t)
  }
  return t
}

function turnOf(v: ThreadView, id: string, ts: number): TurnView | undefined {
  return v.partial ? findTurn(v, id) : ensureTurn(v, id, ts)
}

export function buildView(events: StoredEvent[], partial: boolean): ThreadView {
  const v = emptyView()
  v.partial = partial
  for (const e of events) apply(v, e)
  return v
}

function findItem(t: TurnView, id: string): ItemView | undefined {
  for (let i = t.entries.length - 1; i >= 0; i--) {
    const e = t.entries[i]
    if (e.type === 'item' && e.item.id === id) return e.item
  }
  return undefined
}

function lastTurn(v: ThreadView): TurnView | undefined {
  return v.turns[v.turns.length - 1]
}

export function apply(v: ThreadView, e: StoredEvent) {
  if (e.seq <= v.lastSeq) return
  v.lastSeq = e.seq
  switch (e.kind) {
    case 'session':
      v.sessionId = e.session_id
      v.model = e.model
      v.slashCommands = e.slash_commands
      if (e.models?.length) v.models = e.models
      if (e.efforts?.length) v.efforts = e.efforts
      if (e.modes?.length) v.modes = e.modes
      v.permissionMode = e.permission_mode
      break
    case 'turn_started': {
      const t = ensureTurn(v, e.turn, e.ts)
      t.input = e.input
      t.cause = e.cause
      t.settings = e.settings
      t.started = e.ts
      break
    }
    case 'item_started':
    case 'item_completed': {
      const t = turnOf(v, e.turn, e.ts)
      if (!t) break
      const done = e.kind === 'item_completed'
      const existing = findItem(t, e.item.id)
      const { id, parent, ...rest } = e.item
      const body = rest as ItemBody
      if (body.type === 'user_message' && id === `${t.id}:input`)
        for (const other of v.turns) if (other !== t) other.entries = other.entries.filter((x) => !(x.type === 'item' && x.item.id === id))
      if (existing) {
        existing.body = body
        existing.done = done
        if (done) {
          existing.ended ??= e.ts
          existing.live = undefined
          existing.liveInput = undefined
        }
      } else {
        if (body.type === 'user_message' && id === `${t.id}:input` && t.input.length > 0) break
        t.entries.push({ type: 'item', item: { id, parent: parent ?? undefined, body, done, at: e.ts, ended: done ? e.ts : undefined } })
      }
      break
    }
    case 'request': {
      const t = turnOf(v, e.turn, e.ts)
      if (!t) break
      t.entries.push({ type: 'request', request: { id: e.id, request: e.request } })
      break
    }
    case 'request_resolved': {
      for (let i = v.turns.length - 1; i >= 0; i--) {
        const r = v.turns[i].entries.find((x) => x.type === 'request' && x.request.id === e.id)
        if (r && r.type === 'request') {
          r.request.response = e.response
          break
        }
      }
      break
    }
    case 'turn_completed': {
      const t = turnOf(v, e.turn, e.ts)
      if (!t) break
      t.outcome = e.outcome
      t.usage = e.usage
      t.error = e.error
      t.resetsAt = e.resets_at
      t.ended = e.ts
      for (const x of t.entries)
        if (x.type === 'item') {
          x.item.live = undefined
          x.item.liveInput = undefined
        }
      break
    }
    case 'notice': {
      const t = lastTurn(v)
      const entry: Entry = { type: 'notice', level: e.level, text: e.text, ts: e.ts }
      const into = !t ? v.preface : t.outcome ? t.after : t.entries
      const last = into[into.length - 1]
      if (last?.type === 'notice' && last.level === e.level && last.text === e.text) break
      into.push(entry)
      break
    }
    case 'compacted': {
      const t = lastTurn(v)
      const entry: Entry = { type: 'compacted', trigger: e.trigger }
      if (t) t.entries.push(entry)
      else v.preface.push(entry)
      break
    }
    case 'checkpoint': {
      const t = findTurn(v, e.turn)
      if (t) t.checkpoint = e.repos
      break
    }
    case 'unread': {
      for (const t of v.turns) {
        if (`${t.id}:input` === e.item) t.unread = true
        const it = findItem(t, e.item)
        if (it) it.unread = true
      }
      break
    }
    case 'settings_changed':
    case 'status':
      break
  }
}

export function applyDelta(v: ThreadView, turn: string, item: string, parent: string | undefined, d: Delta) {
  const t = turnOf(v, turn, Date.now())
  if (!t) return
  let it = findItem(t, item)
  if (!it) {
    const body: ItemBody =
      d.type === 'reasoning'
        ? { type: 'reasoning', text: '' }
        : d.type === 'tool_input'
          ? { type: 'tool_call', name: '', input: null, status: 'running' }
          : { type: 'agent_message', text: '' }
    it = { id: item, parent, body, done: false }
    t.entries.push({ type: 'item', item: it })
  }
  if (it.done) return
  if (d.type === 'tool_input') it.liveInput = (it.liveInput ?? '') + d.json
  else it.live = (it.live ?? '') + d.text
}
