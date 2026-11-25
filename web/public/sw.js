self.addEventListener('install', () => self.skipWaiting())
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()))

function resetTime(ms) {
  const d = new Date(ms)
  const today = new Date().toDateString() === d.toDateString()
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  return today ? time : `${d.toLocaleDateString([], { weekday: 'short' })} ${time}`
}

self.addEventListener('push', (e) => {
  let n = {}
  try {
    n = e.data ? e.data.json() : {}
  } catch {
    n = { body: e.data ? e.data.text() : '' }
  }
  let body = n.body || ''
  if (n.resets_at) body += `. Continues at ${resetTime(n.resets_at)}.`
  const thread = n.thread && n.kind !== 'test' ? n.thread : null
  e.waitUntil(
    self.registration.showNotification(n.title || 'Corra', {
      body,
      tag: thread || 'corra',
      renotify: true,
      icon: '/icon.png',
      data: { hash: thread ? `#/t/${thread}` : '#/' },
    }),
  )
})

self.addEventListener('notificationclick', (e) => {
  e.notification.close()
  const hash = (e.notification.data && e.notification.data.hash) || '#/'
  e.waitUntil(
    (async () => {
      const all = await self.clients.matchAll({ type: 'window', includeUncontrolled: true })
      const mine = all.find((c) => new URL(c.url).origin === self.location.origin)
      if (mine) {
        mine.postMessage({ open: hash })
        return mine.focus()
      }
      return self.clients.openWindow(`/${hash}`)
    })(),
  )
})
