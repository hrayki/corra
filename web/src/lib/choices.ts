import type { AgentKind } from '../proto/AgentKind'
import type { AgentSettings } from '../proto/AgentSettings'
import type { Choice } from '../proto/Choice'

export const CLAUDE_MODES: Choice[] = [
  { id: 'default', name: 'Manual' },
  { id: 'acceptEdits', name: 'Accept edits' },
  { id: 'plan', name: 'Plan' },
  { id: 'auto', name: 'Auto' },
  { id: 'bypassPermissions', name: 'Bypass Permissions' },
]

export const AGY_MODES: Choice[] = [
  { id: 'default', name: 'Default' },
  { id: 'accept-edits', name: 'Accept edits' },
  { id: 'plan', name: 'Plan' },
]

export const AGENT_NAMES: Record<AgentKind, string> = { claude: 'Claude Code', antigravity: 'Antigravity' }

const RANKS = ['low', 'medium', 'high', 'xhigh', 'max']

export function fitEffort(effort: string | undefined, list: Choice[]): string | undefined {
  if (!effort || !list.length || list.some((c) => c.id === effort)) return effort
  const want = RANKS.indexOf(effort)
  const ranked = list.map((c) => ({ c, r: RANKS.indexOf(c.id) })).filter((x) => x.r >= 0)
  const below = ranked.filter((x) => x.r <= want).sort((a, b) => b.r - a.r)[0]
  return (below ?? ranked.sort((a, b) => a.r - b.r)[0])?.c.id ?? list[0].id
}

export function findModel(list: Choice[], id: string | undefined): Choice | undefined {
  if (!id) return undefined
  return (
    list.find((c) => c.id === id) ??
    list.find((c) => c.resolved_model === id && c.id !== 'default') ??
    list.find((c) => c.resolved_model === id)
  )
}

export type PickerInput = {
  agent: AgentKind
  settings: AgentSettings
  models: Choice[]
  modes?: Choice[]
  efforts?: Choice[]
  model?: string
  mode?: string
}

export function pickerState(p: PickerInput) {
  const agy = p.agent === 'antigravity'
  const modelId = p.settings.model ?? (agy ? p.model : 'default')
  const model = findModel(p.models, modelId)
  const efforts = model?.efforts ?? (agy && model && model.id === p.model ? p.efforts : undefined) ?? []
  const effort = p.settings.effort ?? 'high'
  const all = agy ? (p.modes?.length ? p.modes : AGY_MODES) : CLAUDE_MODES
  const modes = agy ? all : all.filter((m) => m.id !== 'auto' || model?.supports_auto_mode === true)
  const stored = p.settings.permission_mode ?? 'default'
  const mode = all.find((m) => m.id === stored) ?? all.find((m) => m.id === p.mode) ?? all[0]
  return {
    model,
    modelName: model?.name ?? p.settings.model ?? (agy ? p.model : undefined) ?? 'Default model',
    efforts,
    effort,
    effortName: efforts.find((c) => c.id === effort)?.name ?? '',
    modes,
    mode,
  }
}
