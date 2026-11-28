import { test } from 'node:test'
import assert from 'node:assert/strict'
import { apply, buildView, emptyView, folds, patchItemDetail, type Entry } from '../src/lib/timeline.ts'
import type { StoredEvent } from '../src/proto/StoredEvent.ts'

let seq = 0
const ev = (ts: number, e: object) => ({ seq: ++seq, ts, ...e }) as unknown as StoredEvent
const tool = (turn: string, id: string, ts: number) =>
  ev(ts, { kind: 'item_completed', turn, item: { id, type: 'tool_call', name: 'Bash', input: {}, status: 'completed' } })
const said = (turn: string, id: string, ts: number, text: string) =>
  ev(ts, { kind: 'item_completed', turn, item: { id, type: 'user_message', content: [{ type: 'text', text }] } })
const answer = (turn: string, id: string, ts: number, text: string) =>
  ev(ts, { kind: 'item_completed', turn, item: { id, type: 'agent_message', text } })

const ids = (es: Entry[]) => es.map((e) => (e.type === 'item' ? e.item.id : e.type))

function turnOf(events: StoredEvent[]) {
  const v = emptyView()
  for (const e of events) apply(v, e)
  return v.turns[0]
}

test('a message sent mid-turn stays out of the folded work, which splits around it', () => {
  const t = turnOf([
    ev(1000, { kind: 'turn_started', turn: 't1', input: [{ type: 'text', text: 'fix it' }], settings: {} }),
    said('t1', 't1:input', 1000, 'fix it'),
    tool('t1', 'a', 2000),
    tool('t1', 'b', 3000),
    said('t1', 't2:input', 4000, 'stop a sec'),
    tool('t1', 'c', 5000),
    answer('t1', 'm', 6000, "I've stopped."),
    ev(6000, { kind: 'turn_completed', turn: 't1', outcome: 'completed' }),
  ])
  const f = folds(t.id, t.entries)
  assert.deepEqual(
    f.map((x) => ({ lead: x.lead && ids([x.lead]), work: ids(x.work), rest: ids(x.rest) })),
    [
      { lead: undefined, work: ['a', 'b'], rest: [] },
      { lead: ['t2:input'], work: ['c'], rest: ['m'] },
    ],
  )
  const lead = f[1].lead!
  assert.equal(lead.type === 'item' && lead.item.at, 4000)
})

test('two messages in a row sit together, with no empty fold between them', () => {
  const t = turnOf([
    ev(1000, { kind: 'turn_started', turn: 't1', input: [{ type: 'text', text: 'go' }], settings: {} }),
    tool('t1', 'a', 2000),
    said('t1', 't2:input', 3000, 'one'),
    said('t1', 't3:input', 3500, 'two'),
    tool('t1', 'b', 4000),
    ev(5000, { kind: 'turn_completed', turn: 't1', outcome: 'completed' }),
  ])
  const f = folds(t.id, t.entries)
  assert.deepEqual(
    f.map((x) => [x.lead ? ids([x.lead])[0] : null, ids(x.work).length]),
    [
      [null, 1],
      ['t2:input', 0],
      ['t3:input', 1],
    ],
  )
})

test('a turn without one folds its work once, as before', () => {
  const t = turnOf([
    ev(1000, { kind: 'turn_started', turn: 't1', input: [{ type: 'text', text: 'go' }], settings: {} }),
    tool('t1', 'a', 2000),
    answer('t1', 'm', 3000, 'done'),
    ev(3000, { kind: 'turn_completed', turn: 't1', outcome: 'completed' }),
  ])
  const f = folds(t.id, t.entries)
  assert.equal(f.length, 1)
  assert.deepEqual([ids(f[0].work), ids(f[0].rest)], [['a'], ['m']])
})

test('a thought saved after the reply folds with the work and leaves the reply out', () => {
  const thought = (turn: string, id: string, ts: number) =>
    ev(ts, { kind: 'item_completed', turn, item: { id, type: 'reasoning', text: 'hmm' } })
  const t = turnOf([
    ev(1000, { kind: 'turn_started', turn: 't1', input: [{ type: 'text', text: 'go' }], settings: {} }),
    tool('t1', 'a', 2000),
    answer('t1', 'm', 3000, 'done'),
    thought('t1', 'r', 3000),
    ev(3000, { kind: 'turn_completed', turn: 't1', outcome: 'completed' }),
  ])
  const f = folds(t.id, t.entries)
  assert.deepEqual([ids(f[0].work), ids(f[0].rest)], [['a', 'r'], ['m']])
})

test('a thread opened on its last turns ignores steps of earlier turns until those load', () => {
  const start = (turn: string, ts: number) => ev(ts, { kind: 'turn_started', turn, input: [{ type: 'text', text: turn }], settings: {} })
  const done = (turn: string, ts: number) => ev(ts, { kind: 'turn_completed', turn, outcome: 'completed' })
  const older = [start('t1', 1), tool('t1', 'agent', 2), done('t1', 3)]
  const window = [start('t2', 4), answer('t2', 'a2', 5, 'two'), done('t2', 6), answer('t1', 'late', 7, 'the agent of turn 1 finished')]
  const partial = buildView(window, true)
  assert.deepEqual(
    partial.turns.map((t) => t.id),
    ['t2'],
    'a step of an unloaded turn showed up as a turn of its own',
  )
  const whole = buildView([...older, ...window], false)
  assert.deepEqual(
    whole.turns.map((t) => [t.id, ids(t.entries)]),
    [
      ['t1', ['agent', 'late']],
      ['t2', ['a2']],
    ],
  )
})

test('a notice sent between turns shows after the turn before it, opened whole or from its end', () => {
  const start = (turn: string, ts: number) => ev(ts, { kind: 'turn_started', turn, input: [{ type: 'text', text: turn }], settings: {} })
  const done = (turn: string, ts: number) => ev(ts, { kind: 'turn_completed', turn, outcome: 'completed' })
  const note = (ts: number, text: string) => ev(ts, { kind: 'notice', level: 'warning', text })
  const older = [
    note(1, 'Before any turn'),
    start('t1', 2),
    answer('t1', 'a1', 3, 'one'),
    done('t1', 4),
    note(5, 'The machine running this thread stopped.'),
  ]
  const window = [start('t2', 6), answer('t2', 'a2', 7, 'two'), done('t2', 8), note(9, "Couldn't send: the machine is asleep")]
  const text = (e: Entry) => (e.type === 'notice' ? e.text : e.type)
  const where = (v: ReturnType<typeof buildView>) => ({
    top: v.preface.map(text),
    turns: v.turns.map((t) => [t.id, t.after.map(text)]),
  })
  const partial = buildView(window, true)
  assert.deepEqual(where(partial), { top: [], turns: [['t2', ["Couldn't send: the machine is asleep"]]] })
  apply(partial, note(10, 'The machine restarted.'))
  assert.deepEqual(partial.turns[0].after.map(text), ["Couldn't send: the machine is asleep", 'The machine restarted.'])
  assert.deepEqual(where(buildView([...older, ...window], false)), {
    top: ['Before any turn'],
    turns: [
      ['t1', ['The machine running this thread stopped.']],
      ['t2', ["Couldn't send: the machine is asleep"]],
    ],
  })
})

test('a thread cached before session events carried models still opens', () => {
  const v = buildView(
    [
      ev(1000, { kind: 'session', session_id: 's', cwd: '~', model: 'opus', tools: [], slash_commands: [] }),
      ev(1000, { kind: 'turn_started', turn: 't1', input: [{ type: 'text', text: 'hi' }], settings: {} }),
      answer('t1', 'm', 2000, 'hello'),
      ev(2000, { kind: 'turn_completed', turn: 't1', outcome: 'completed' }),
    ],
    false,
  )
  assert.equal(v.model, 'opus')
  assert.deepEqual(ids(v.turns[0].entries), ['m'])
})

test('a message the agent never read is marked on its own bubble, whether it was sent mid-turn or began the turn', () => {
  const v = emptyView()
  for (const e of [
    ev(1000, { kind: 'turn_started', turn: 't1', input: [{ type: 'text', text: 'go' }], settings: {} }),
    said('t1', 't1:input', 1000, 'go'),
    tool('t1', 'a', 2000),
    said('t1', 't2:input', 3000, 'and this'),
    ev(4000, { kind: 'unread', item: 't2:input' }),
    ev(4000, { kind: 'turn_completed', turn: 't1', outcome: 'interrupted' }),
    ev(5000, { kind: 'turn_started', turn: 't3', input: [{ type: 'text', text: 'again' }], settings: {} }),
    said('t3', 't3:input', 5000, 'again'),
    ev(5100, { kind: 'unread', item: 't3:input' }),
    ev(5100, { kind: 'turn_completed', turn: 't3', outcome: 'interrupted' }),
  ])
    apply(v, e)
  const [t1, t3] = v.turns
  const mid = t1.entries.find((e) => e.type === 'item' && e.item.id === 't2:input')
  assert.ok(mid && mid.type === 'item' && mid.item.unread)
  assert.ok(!t1.unread)
  assert.ok(t3.unread)
})

test('deferred tool calls keep their completed seq and patch full detail cleanly', () => {
  const start = ev(1000, { kind: 'turn_started', turn: 't1', input: [{ type: 'text', text: 'read' }], settings: {} })
  const toolStart = ev(1100, {
    kind: 'item_started',
    turn: 't1',
    item: { id: 'tool-1', type: 'tool_call', name: 'Read', input: { file_path: 'a.rs' }, status: 'running' },
  })
  const toolDone = ev(1200, {
    kind: 'item_completed',
    turn: 't1',
    item: {
      id: 'tool-1',
      type: 'tool_call',
      name: 'Read',
      input: { file_path: 'a.rs' },
      output: { text: '', is_error: false, images: [], deferred: true },
      status: 'completed',
    },
  })
  const v = buildView([start, toolStart, toolDone], false)
  const entry = v.turns[0].entries[0]
  assert.equal(entry.type, 'item')
  assert.equal(entry.item.seq, toolDone.seq)
  assert.equal(entry.item.body.type, 'tool_call')
  assert.equal(entry.item.body.output?.deferred, true)

  const ok = patchItemDetail(v, 'tool-1', {
    type: 'tool_call',
    name: 'Read',
    input: { file_path: 'a.rs' },
    output: { text: 'full file body', is_error: false, images: [] },
    status: 'completed',
  })
  assert.equal(ok, true)
  const updated = v.turns[0].entries[0]
  assert.equal(updated.type, 'item')
  assert.equal(updated.item.body.type, 'tool_call')
  assert.equal(updated.item.body.output?.text, 'full file body')
  assert.equal(updated.item.body.output?.deferred, undefined)
})
