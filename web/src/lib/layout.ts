import { createSignal } from 'solid-js'

const WIDTH = 'corra:sidebar-width'
const COLLAPSED = 'corra:sidebar-collapsed'
export const SIDEBAR_MIN = 216
export const SIDEBAR_MAX = 440

const stored = Number(localStorage.getItem(WIDTH))
const [width, setWidthRaw] = createSignal(stored >= SIDEBAR_MIN && stored <= SIDEBAR_MAX ? stored : 268)
const [collapsed, setCollapsedRaw] = createSignal(localStorage.getItem(COLLAPSED) === '1')

export const sidebarWidth = width
export const sidebarCollapsed = collapsed

export function setSidebarWidth(w: number) {
  const v = Math.round(Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, w)))
  setWidthRaw(v)
  localStorage.setItem(WIDTH, String(v))
}

export function toggleSidebar(on?: boolean) {
  const v = on ?? !collapsed()
  setCollapsedRaw(v)
  localStorage.setItem(COLLAPSED, v ? '1' : '0')
}

window.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'b') {
    e.preventDefault()
    toggleSidebar()
  }
})
