import { Show, createMemo } from 'solid-js'
import type { AgentSettings } from '../proto/AgentSettings'
import type { AgentKind } from '../proto/AgentKind'
import type { Choice } from '../proto/Choice'
import { AGENT_NAMES, fitEffort, pickerState } from '../lib/choices'
import { Menu } from './bits'

const pill =
  'inline-flex h-7 items-center gap-1 whitespace-nowrap rounded-lg border border-line px-2 text-[12.5px] text-mute hover:border-faint hover:text-ink max-md:h-9'

const choiceKey = (agent: AgentKind, what: 'models' | 'modes' | 'default') => `corra_${agent}_${what}`

const remembered = (agent: AgentKind, what: 'models' | 'modes'): Choice[] | undefined => {
  try {
    return JSON.parse(localStorage.getItem(choiceKey(agent, what)) ?? 'null') ?? undefined
  } catch {
    return undefined
  }
}

export function rememberChoices(agent: AgentKind, v?: { model?: string; models?: Choice[]; modes?: Choice[] }) {
  if (v?.models?.length) localStorage.setItem(choiceKey(agent, 'models'), JSON.stringify(v.models))
  if (v?.modes?.length) localStorage.setItem(choiceKey(agent, 'modes'), JSON.stringify(v.modes))
  if (v?.model) localStorage.setItem(choiceKey(agent, 'default'), v.model)
}

export function SettingsPickers(props: {
  settings: AgentSettings
  onChange: (s: AgentSettings) => void
  agent?: AgentKind
  model?: string
  models?: Choice[]
  efforts?: Choice[]
  modes?: Choice[]
  mode?: string
}) {
  const agent = () => props.agent ?? 'claude'
  const models = () => (props.models?.length ? props.models : (remembered(agent(), 'models') ?? []))
  const st = createMemo(() =>
    pickerState({
      agent: agent(),
      settings: props.settings,
      models: models(),
      modes: props.modes?.length ? props.modes : remembered(agent(), 'modes'),
      efforts: props.efforts,
      model: props.model ?? localStorage.getItem(choiceKey(agent(), 'default')) ?? undefined,
      mode: props.mode,
    }),
  )
  const pickModel = (id: string) => {
    const list = models().find((c) => c.id === id)?.efforts ?? []
    const e = fitEffort(st().effort, list)
    props.onChange(e && e !== st().effort ? { model: id, effort: e } : { model: id })
  }
  return {
    mode: (
      <Menu
        label="Permissions"
        side="top"
        class={pill}
        items={st().modes.map((m) => ({
          label: m.name,
          detail: m.hint,
          checked: m.id === st().mode.id,
          onSelect: () => props.onChange({ permission_mode: m.id }),
        }))}
      >
        <span classList={{ 'text-warn': st().mode.id === 'bypassPermissions' }}>{st().mode.name}</span>
      </Menu>
    ),
    model: (
      <Menu
        label="Model and effort"
        side="top"
        align="end"
        class={pill}
        items={[
          { heading: 'Model' },
          ...(!models().length
            ? [
                {
                  label: st().modelName,
                  detail: `The list shows once ${AGENT_NAMES[agent()]} has started here.`,
                  checked: true,
                  onSelect: () => {},
                },
              ]
            : models().map((m) => ({
                label: m.name,
                detail: m.hint,
                checked: st().model?.id === m.id,
                onSelect: () => pickModel(m.id),
              }))),
          ...(st().efforts.length
            ? [
                'col' as const,
                { heading: 'Effort' },
                ...st().efforts.map((e) => ({
                  label: e.name,
                  checked: st().effort === e.id,
                  onSelect: () => props.onChange({ effort: e.id }),
                })),
              ]
            : []),
        ]}
      >
        <span>{st().modelName}</span>
        <Show when={st().effortName}>
          <span class="text-faint @max-xl:hidden">{st().effortName}</span>
        </Show>
      </Menu>
    ),
  }
}
