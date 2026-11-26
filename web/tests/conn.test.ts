import { test, mock } from 'node:test'
import assert from 'node:assert/strict'

class FakeSocket {
  static all: FakeSocket[] = []
  static OPEN = 1
  readyState = 0
  sent: string[] = []
  onopen?: () => void
  onclose?: () => void
  onmessage?: (ev: { data: string }) => void
  constructor() {
    FakeSocket.all.push(this)
  }
  send(m: string) {
    this.sent.push(m)
  }
  close() {}
  open() {
    this.readyState = 1
    this.onopen?.()
  }
  drop() {
    this.readyState = 3
    this.onclose?.()
  }
  reply(id: number, ok: unknown) {
    this.onmessage?.({ data: JSON.stringify({ t: 'reply', id, ok }) })
  }
}

Object.assign(globalThis, { WebSocket: FakeSocket, location: { href: 'http://hub/' } })
const { Conn } = await import('../src/lib/conn.ts')

const latest = () => FakeSocket.all[FakeSocket.all.length - 1]
const bodies = (s: FakeSocket) => s.sent.map((m) => JSON.parse(m) as { id: number; op: string; client_id?: string })

test('a send made while the connection drops goes once, with its id, after the reconnect', async () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  FakeSocket.all = []
  const conn = new Conn(
    () => '/app',
    () => {},
    () => {},
  )
  let failed: Error | undefined
  const sending = conn.call({ op: 'send', thread: 't', input: [], queue: false, client_id: 'c1' } as never)
  sending.catch((e) => (failed = e))
  latest().drop()
  mock.timers.tick(500)
  latest().drop()
  mock.timers.tick(1000)
  await Promise.resolve()
  assert.equal(failed, undefined, 'the send was given up while it still went out later')
  latest().open()
  const sent = bodies(latest())
  assert.deepEqual(
    sent.map((b) => b.client_id),
    ['c1'],
  )
  latest().reply(sent[0].id, { queued: false })
  assert.deepEqual(await sending, { queued: false })
  conn.close()
  mock.timers.reset()
})

test('a send in flight when the connection drops is sent again with the same id; other calls fail', async () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  FakeSocket.all = []
  const conn = new Conn(
    () => '/app',
    () => {},
    () => {},
  )
  latest().open()
  const sending = conn.call({ op: 'send', thread: 't', input: [], queue: false, client_id: 'c2' } as never)
  const stopping = conn.call({ op: 'interrupt', thread: 't' } as never)
  await assert.rejects(
    (async () => {
      latest().drop()
      await stopping
    })(),
    /Lost the connection/,
  )
  mock.timers.tick(500)
  latest().open()
  const again = bodies(latest())
  assert.deepEqual(
    again.map((b) => [b.op, b.client_id]),
    [['send', 'c2']],
  )
  latest().reply(again[0].id, { queued: true })
  assert.deepEqual(await sending, { queued: true })
  conn.close()
  mock.timers.reset()
})
