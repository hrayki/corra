import { createSignal } from 'solid-js'
import type { Thread } from '../proto/Thread'

export type ToastItem = {
  id: number
  tone: 'error' | 'info'
  message: string
}

let nextToast = 1
const [toasts, setToasts] = createSignal<ToastItem[]>([])
const [renaming, setRenaming] = createSignal<Thread | null>(null)
const [paletteOpen, setPaletteOpen] = createSignal(false)

export { toasts, renaming, paletteOpen, setPaletteOpen }

export function pushToast(message: string, tone: 'error' | 'info' = 'error') {
  const id = nextToast++
  setToasts((list) => [...list.slice(-3), { id, tone, message }])
  window.setTimeout(() => dismissToast(id), tone === 'error' ? 6000 : 3500)
}

export function dismissToast(id: number) {
  setToasts((list) => list.filter((t) => t.id !== id))
}

export function openRename(thread: Thread) {
  setRenaming(thread)
}

export function closeRename() {
  setRenaming(null)
}
