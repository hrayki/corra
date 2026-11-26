import type { ItemView, ThreadView, TurnView } from './timeline'

export type AgentState = 'running' | 'done' | 'failed' | 'stopped'

export type AgentInfo = {
  id: string
  description: string
  kind?: string
  prompt: string
  background: boolean
  state: AgentState
  started?: number
  ended?: number
  result?: string
  latest?: ItemView
}

type ToolBody = Extract<ItemView['body'], { type: 'tool_call' }>

export const isAgentCall = (it: ItemView) => it.body.type === 'tool_call' && (it.body.name === 'Agent' || it.body.name === 'Task')

export function startedThread(it: ItemView): string | undefined {
  const b = it.body
  if (b.type !== 'tool_call' || b.name !== 'mcp__corra__threads_start' || b.status !== 'completed') return undefined
  try {
    const v = JSON.parse(b.output?.text ?? '') as { thread?: unknown }
    return typeof v.thread === 'string' ? v.thread : undefined
  } catch {
    return undefined
  }
}

export const isTaskCall = (it: ItemView) =>
  isAgentCall(it) || (it.body.type === 'tool_call' && it.body.name === 'Bash' && field(it.body, 'run_in_background') === true)

function field(b: ToolBody, k: string): unknown {
  const i = b.input
  return i && typeof i === 'object' && !Array.isArray(i) ? (i as Record<string, unknown>)[k] : undefined
}

const TRAILER = /\n*agentId: [\s\S]*$/

export function agentInfo(it: ItemView, kids: ItemView[], running: string[] | undefined, now: number): AgentInfo {
  const b = it.body as ToolBody
  const background = field(b, 'run_in_background') === true
  const task = kids.find((k) => k.id.startsWith('task:'))
  const steps = kids.filter((k) => k !== task)
  const info: AgentInfo = {
    id: it.id,
    description: String(field(b, 'description') ?? 'Agent'),
    kind: typeof field(b, 'subagent_type') === 'string' ? String(field(b, 'subagent_type')) : undefined,
    prompt: String(field(b, 'prompt') ?? ''),
    background,
    state: 'running',
    started: it.at,
    latest: steps.at(-1),
  }
  if (background) {
    const text = task?.body.type === 'agent_message' ? task.body.text : undefined
    const m = text?.match(/^Background (?:agent|command)[^:]*? (completed|failed|stopped|killed)(?:: ([\s\S]*))?/)
    if (task) {
      info.ended = task.at
      info.state = m?.[1] === 'completed' ? 'done' : m?.[1] === 'failed' ? 'failed' : m ? 'stopped' : 'done'
      info.result = m?.[2]
    } else if (running && !running.includes(it.id) && now - (it.at ?? now) > 10_000) {
      info.state = 'stopped'
    }
    return info
  }
  if (b.status === 'running') return info
  info.ended = it.ended
  info.state = b.status === 'completed' ? 'done' : b.status === 'denied' ? 'stopped' : 'failed'
  info.result = b.output?.text.replace(TRAILER, '').trim() || undefined
  return info
}

export function kidsOf(t: TurnView): Map<string, ItemView[]> {
  const m = new Map<string, ItemView[]>()
  for (const e of t.entries)
    if (e.type === 'item' && e.item.parent) {
      const l = m.get(e.item.parent) ?? []
      l.push(e.item)
      m.set(e.item.parent, l)
    }
  return m
}

export function findCall(v: ThreadView | undefined, id: string): { turn: TurnView; item: ItemView } | undefined {
  if (!v) return undefined
  for (let i = v.turns.length - 1; i >= 0; i--)
    for (const e of v.turns[i].entries) if (e.type === 'item' && e.item.id === id) return { turn: v.turns[i], item: e.item }
  return undefined
}
