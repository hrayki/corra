import { createEffect, createRoot } from 'solid-js'
import { callOn, state } from './store'

const focused = () => document.visibilityState === 'visible' && document.hasFocus()
let reported: boolean | null = null

export function reportPresence(force = false) {
  if (state.conn.hub !== 'open') {
    reported = null
    return
  }
  const f = focused()
  if (!force && f === reported) return
  reported = f
  callOn('hub', { op: 'presence', focused: f }).catch(() => {
    reported = null
  })
}

window.addEventListener('focus', () => reportPresence())
window.addEventListener('blur', () => reportPresence())
document.addEventListener('visibilitychange', () => reportPresence())
setInterval(() => reportPresence(focused()), 60_000)
createRoot(() =>
  createEffect(() => {
    if (state.conn.hub !== 'open') return
    reported = null
    reportPresence()
    void refreshPush().catch(() => {})
  }),
)

export type PushState = 'unsupported' | 'blocked' | 'off' | 'on'

export const pushSupported = () =>
  'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window && window.isSecureContext

function keyBytes(b64url: string) {
  const s = atob(b64url.replace(/-/g, '+').replace(/_/g, '/'))
  return Uint8Array.from(s, (c) => c.charCodeAt(0))
}

function sameKey(sub: PushSubscription, key: Uint8Array) {
  const have = sub.options.applicationServerKey
  if (!have) return false
  const a = new Uint8Array(have)
  return a.length === key.length && a.every((v, i) => v === key[i])
}

async function register(key: string) {
  const reg = await navigator.serviceWorker.register('/sw.js')
  await navigator.serviceWorker.ready
  const want = keyBytes(key)
  let sub = await reg.pushManager.getSubscription()
  if (sub && !sameKey(sub, want)) {
    await sub.unsubscribe()
    sub = null
  }
  sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: want })
  const j = sub.toJSON()
  await callOn('hub', { op: 'push_subscribe', endpoint: j.endpoint!, p256dh: j.keys!.p256dh, auth: j.keys!.auth })
}

export async function pushState(): Promise<PushState> {
  if (!pushSupported()) return 'unsupported'
  if (Notification.permission === 'denied') return 'blocked'
  if (Notification.permission !== 'granted') return 'off'
  const { on } = await callOn<{ on: boolean }>('hub', { op: 'push_key' })
  const reg = await navigator.serviceWorker.getRegistration()
  const sub = await reg?.pushManager.getSubscription()
  return on && sub ? 'on' : 'off'
}

export async function turnOn() {
  if ((await Notification.requestPermission()) !== 'granted') throw new Error('The browser said no to notifications.')
  const { key } = await callOn<{ key: string }>('hub', { op: 'push_key' })
  await register(key)
}

export async function turnOff() {
  const reg = await navigator.serviceWorker.getRegistration()
  const sub = await reg?.pushManager.getSubscription()
  await sub?.unsubscribe()
  await callOn('hub', { op: 'push_unsubscribe' })
}

export async function refreshPush() {
  if (!pushSupported() || Notification.permission !== 'granted') return
  const reg = await navigator.serviceWorker.getRegistration()
  if (!(await reg?.pushManager.getSubscription())) return
  const { key } = await callOn<{ key: string }>('hub', { op: 'push_key' })
  await register(key)
}

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.addEventListener('message', (e) => {
    const open = (e.data as { open?: string } | null)?.open
    if (open?.startsWith('#/')) location.hash = open
  })
}
