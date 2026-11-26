import { createSignal, type Accessor } from 'solid-js'
import type { AppCall } from '../proto/AppCall'
import type { HubToApp } from '../proto/HubToApp'

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void; text: string; sent: boolean; again: boolean }
export type ConnState = 'connecting' | 'open' | 'closed'

export class Conn {
  private ws?: WebSocket
  private nextId = 1
  private pending = new Map<number, Pending>()
  private backoff = 400
  private queue: number[] = []
  private stopped = false
  private fails = 0
  readonly state: Accessor<ConnState>
  private setState: (s: ConnState) => void
  private url: () => string
  private onMessage: (m: HubToApp) => void
  private onOpen: () => void
  private onRefused?: () => void

  constructor(url: () => string, onMessage: (m: HubToApp) => void, onOpen: () => void, onRefused?: () => void) {
    const [s, set] = createSignal<ConnState>('connecting')
    this.state = s
    this.setState = set
    this.url = url
    this.onMessage = onMessage
    this.onOpen = onOpen
    this.onRefused = onRefused
    this.open()
  }

  private open() {
    if (this.stopped) return
    this.setState('connecting')
    const u = new URL(this.url(), location.href)
    const token = u.searchParams.get('token')
    u.searchParams.delete('token')
    const ws = token ? new WebSocket(u.toString(), ['corra.v1', token]) : new WebSocket(u.toString())
    this.ws = ws
    let opened = false
    ws.onopen = () => {
      opened = true
      this.backoff = 400
      this.fails = 0
      this.setState('open')
      for (const id of this.queue.splice(0)) {
        const p = this.pending.get(id)
        if (!p) continue
        ws.send(p.text)
        p.sent = true
      }
      this.onOpen()
    }
    ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data as string) as HubToApp
      if (msg.t === 'reply') {
        const p = this.pending.get(msg.id)
        if (!p) return
        this.pending.delete(msg.id)
        if (msg.err) p.reject(new Error(msg.err))
        else p.resolve(msg.ok)
        return
      }
      this.onMessage(msg)
    }
    ws.onclose = () => {
      if (this.ws !== ws) return
      this.setState('closed')
      for (const [id, p] of [...this.pending]) {
        if (!p.sent && !this.stopped) continue
        if (p.again && !this.stopped) {
          p.sent = false
          this.queue.push(id)
        } else {
          this.pending.delete(id)
          p.reject(new Error('Lost the connection'))
        }
      }
      this.queue.sort((a, b) => a - b)
      if (!opened && ++this.fails === 3) this.onRefused?.()
      if (this.stopped) return
      setTimeout(() => this.open(), this.backoff)
      this.backoff = Math.min(this.backoff * 2, 8000)
    }
  }

  close() {
    this.stopped = true
    this.ws?.close()
  }

  call<T = unknown>(body: AppCall): Promise<T> {
    const id = this.nextId++
    const text = JSON.stringify({ id, ...body })
    const again = body.op === 'send' && !!body.client_id
    return new Promise<T>((resolve, reject) => {
      const p: Pending = { resolve: resolve as (v: unknown) => void, reject, text, sent: false, again }
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.pending.set(id, p)
        this.ws.send(text)
        p.sent = true
      } else if (this.queue.length < 200) {
        this.pending.set(id, p)
        this.queue.push(id)
      } else reject(new Error('Not connected'))
    })
  }
}
