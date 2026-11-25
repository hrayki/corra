import { createResource, createRoot } from 'solid-js'
import { callOn, state } from './store'
import type { ProviderInfo } from '../proto/ProviderInfo'
import type { ToolInfo } from '../proto/ToolInfo'
import type { KindInfo } from '../proto/KindInfo'

const hubUp = () => state.conn.hub === 'open'

const [providerList, providerRes] = createRoot(() =>
  createResource(hubUp, (up) => (up ? callOn<ProviderInfo[]>('hub', { op: 'list_providers' }) : Promise.resolve([]))),
)
const [toolList, toolRes] = createRoot(() =>
  createResource(hubUp, (up) => (up ? callOn<ToolInfo[]>('hub', { op: 'list_tools' }) : Promise.resolve([]))),
)

export const providers = () => providerList() ?? []
export const tools = () => toolList() ?? []

export function refreshCatalog() {
  void providerRes.refetch()
  void toolRes.refetch()
}

export const providerOf = (name: string | undefined) => providers().find((p) => p.name === name)

export const providerLabel = (name: string | undefined) => providerOf(name)?.label ?? name ?? ''

export const paired = (provider: string) => providerOf(provider)?.creates_machines === false

export const makers = () => providers().filter((p) => p.creates_machines && p.connected)

export function kindOf(provider: string | undefined, kind: string | undefined): KindInfo | undefined {
  const kinds = providerOf(provider)?.kinds ?? []
  return kinds.find((k) => k.name === kind) ?? kinds[0]
}

export const FREE_SIZES = ['2x4', '4x8', '8x16', '8x32', '16x64']

export function sizesFor(kind: KindInfo | undefined) {
  return kind?.sizes.length ? kind.sizes.map((s) => `${s.cpu}x${s.memory_gb}`) : FREE_SIZES
}

export function sizeLabel(size: string) {
  const [c, m] = size.split('x')
  return m ? `${c} cores, ${m} GB` : size
}
