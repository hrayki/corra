import { For, Show } from 'solid-js'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import { state } from '../lib/store'
import { makers, paired } from '../lib/catalog'
import type { KindInfo } from '../proto/KindInfo'
import { Menu } from './bits'

export type SetupRow = { name: string; repos: string[]; env: string[] }
export type ProjectPlace = { key: string; label: string }

export const MAIN_POLICY = [
  { value: false, label: 'Wait for you', hint: 'Pushing to the default branch or a tag, and merging, wait for your yes.' },
  { value: true, label: 'On its own', hint: 'It pushes to the default branch and tags, and merges, on its own.' },
]

export const formPill = 'inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-raised px-2.5 text-[13px] hover:border-faint'

export function projectPlaces(): ProjectPlace[] {
  const out = makers().map((p) => ({ key: p.name, label: p.label }))
  for (const m of Object.values(state.machines)) {
    if (paired(m.provider) && m.status !== 'destroyed') out.push({ key: `machine:${m.id}`, label: m.name })
  }
  return out
}

export function PlaceAndKindPicker(props: {
  places: ProjectPlace[]
  selectedPlace?: string
  fallbackLabel?: string
  onSelectPlace: (key: string) => void
  kinds: KindInfo[]
  selectedKind?: KindInfo
  onSelectKind: (kind: string) => void
}) {
  const currentLabel = () => props.places.find((x) => x.key === props.selectedPlace)?.label ?? props.fallbackLabel ?? ''
  return (
    <>
      <Show when={props.places.length}>
        <Menu
          label="Runs on"
          class={formPill}
          items={props.places.map((x) => ({
            label: x.label,
            checked: x.key === props.selectedPlace,
            onSelect: () => props.onSelectPlace(x.key),
          }))}
        >
          <span>{currentLabel()}</span>
          <ChevronDown size={13} class="text-faint" />
        </Menu>
      </Show>
      <Show when={props.kinds.length > 1}>
        <Menu
          label="Kind"
          class={formPill}
          items={props.kinds.map((k) => ({
            label: k.label,
            checked: k.name === props.selectedKind?.name,
            onSelect: () => props.onSelectKind(k.name),
          }))}
        >
          <span>{props.selectedKind?.label}</span>
          <ChevronDown size={13} class="text-faint" />
        </Menu>
      </Show>
    </>
  )
}

export function SetupPicker(props: { setups: SetupRow[]; value: string | null | undefined; onChange: (name: string | null) => void }) {
  return (
    <Menu
      label="Setup"
      class={formPill}
      items={[
        { label: 'None', checked: !props.value, onSelect: () => props.onChange(null) },
        ...props.setups.map((s) => ({
          label: s.name,
          checked: s.name === props.value,
          onSelect: () => props.onChange(s.name),
        })),
      ]}
    >
      <span>{props.value || 'None'}</span>
      <ChevronDown size={13} class="text-faint" />
    </Menu>
  )
}

export function MainAndTagsPicker(props: { value: boolean; onChange: (allowMain: boolean) => void }) {
  return (
    <div>
      <div class="inline-flex flex-wrap self-start justify-self-start rounded-lg bg-ink/[0.05] p-0.5 text-[13px]">
        <For each={MAIN_POLICY}>
          {(m) => (
            <button
              type="button"
              class="h-7 rounded-md px-3"
              classList={{
                'bg-raised font-medium shadow-[0_1px_2px_rgb(0_0_0/0.08)]': props.value === m.value,
                'text-mute': props.value !== m.value,
              }}
              onClick={() => props.onChange(m.value)}
            >
              {m.label}
            </button>
          )}
        </For>
      </div>
      <p class="mt-1 text-[12.5px] text-faint">{MAIN_POLICY.find((m) => m.value === props.value)?.hint}</p>
    </div>
  )
}
