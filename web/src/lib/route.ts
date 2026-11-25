import { createSignal } from 'solid-js'

export type Route =
  | { kind: 'home' }
  | { kind: 'new'; project?: string }
  | { kind: 'new-project' }
  | { kind: 'import'; machine?: string }
  | { kind: 'project'; id: string }
  | { kind: 'thread'; id: string }
  | { kind: 'settings' }

function read(): Route {
  const h = location.hash
  let m = h.match(/^#\/t\/([0-9a-f-]+)/)
  if (m) return { kind: 'thread', id: m[1] }
  if (h.startsWith('#/settings')) return { kind: 'settings' }
  if (h.startsWith('#/new-project')) return { kind: 'new-project' }
  m = h.match(/^#\/import(?:\/([0-9a-f-]+))?/)
  if (m) return { kind: 'import', machine: m[1] }
  m = h.match(/^#\/project\/([0-9a-f-]+)/)
  if (m) return { kind: 'project', id: m[1] }
  m = h.match(/^#\/new(?:\/(.+))?$/)
  if (m) return { kind: 'new', project: m[1] ? decodeURIComponent(m[1]) : undefined }
  return { kind: 'home' }
}

const [route, setRoute] = createSignal<Route>(read())
window.addEventListener('hashchange', () => setRoute(read()))
export { route }
