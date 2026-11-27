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
  { id: 'acceptEdits', name: 'Accept edits' },
  { id: 'plan', name: 'Plan' },
  { id: 'bypassPermissions', name: 'Bypass Permissions' },
]

export const AGENT_NAMES: Record<AgentKind, string> = { claude: 'Claude Code', antigravity: 'Antigravity' }

const RANKS = ['low', 'medium', 'high', 'xhigh', 'max']

export function normalizeModeId(id: string | undefined): string | undefined {
  return id === 'accept-edits' ? 'acceptEdits' : id
}

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
  const defaultModes = p.agent === 'antigravity' ? AGY_MODES : CLAUDE_MODES
  const modelId = p.settings.model ?? (p.agent === 'claude' ? 'default' : p.model)
  const model = findModel(p.models, modelId)
  const efforts = model?.efforts ?? (model && model.id === p.model ? p.efforts : undefined) ?? []
  const effort = p.settings.effort ?? 'high'
  const rawModes = p.modes?.length ? p.modes.map((m) => ({ ...m, id: normalizeModeId(m.id)! })) : defaultModes
  const all = rawModes.some((m) => m.id === 'bypassPermissions')
    ? rawModes
    : [...rawModes, { id: 'bypassPermissions', name: 'Bypass Permissions' }]
  const modes = p.agent === 'claude' ? all.filter((m) => m.id !== 'auto' || model?.supports_auto_mode === true) : all
  const stored = normalizeModeId(p.settings.permission_mode) ?? 'default'
  const liveMode = normalizeModeId(p.mode)
  const mode = all.find((m) => m.id === stored) ?? all.find((m) => m.id === liveMode) ?? all[0]
  return {
    model,
    modelName: model?.name ?? p.settings.model ?? (p.agent === 'claude' ? undefined : p.model) ?? 'Default model',
    efforts,
    effort,
    effortName: efforts.find((c) => c.id === effort)?.name ?? '',
    modes,
    mode,
  }
}
