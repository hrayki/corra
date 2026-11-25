import { createSignal } from 'solid-js'
import { sidebarCollapsed, sidebarWidth } from './layout'

export type Panel =
  | { kind: 'diff'; turn?: string; file?: string }
  | { kind: 'files'; path?: string; line?: number }
  | { kind: 'terminal' }
  | { kind: 'browser' }
  | { kind: 'phone' }

const [panel, setPanel] = createSignal<Panel | null>(null)
const [panelWidthSignal, setPanelWidthSignal] = createSignal(Number(localStorage.getItem('corra_panel_w')) || 560)
export { panel, setPanel }

export function openFile(ref: { path: string; line?: number }) {
  setPanel({ kind: 'files', path: ref.path, line: ref.line })
}

const CHAT_MIN = 480
const sidebar = () => (sidebarCollapsed() ? 48 : sidebarWidth())

export function panelWidth() {
  return Math.max(360, Math.min(panelWidthSignal(), window.innerWidth - sidebar() - CHAT_MIN))
}

export function panelOverlays() {
  return window.innerWidth >= 768 && window.innerWidth - sidebar() - CHAT_MIN < 360
}

export function setPanelWidth(w: number) {
  const v = Math.max(360, Math.min(w, window.innerWidth - sidebar() - CHAT_MIN))
  setPanelWidthSignal(v)
  localStorage.setItem('corra_panel_w', String(v))
}
