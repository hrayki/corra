import { persistDrafts } from './drafts'

const RELOADED = 'corra:reloaded-at'

const own = (() => {
  const assets = [...document.querySelectorAll('script[src], link[href]')]
    .map((x) => new URL((x as HTMLScriptElement).src || (x as HTMLLinkElement).href, location.href).pathname)
    .filter((p) => p.startsWith('/assets/'))
  return assets.length ? assets.join(' ') : null
})()

export function reloadForUpdate(): boolean {
  try {
    const last = Number(sessionStorage.getItem(RELOADED) ?? 0)
    if (Date.now() - last < 30_000) return false
    sessionStorage.setItem(RELOADED, String(Date.now()))
  } catch {}
  persistDrafts()
  location.reload()
  return true
}

export function checkBuild(theirs: string | undefined) {
  if (own && theirs && own !== theirs) reloadForUpdate()
}

export function lazyPart<T>(load: () => Promise<T>): () => Promise<T> {
  return () =>
    load().catch((e) => {
      reloadForUpdate()
      throw e
    })
}

window.addEventListener('vite:preloadError', (e) => {
  if (reloadForUpdate()) e.preventDefault()
})
