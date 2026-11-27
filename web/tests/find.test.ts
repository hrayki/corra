import { test } from 'node:test'
import assert from 'node:assert/strict'
import { holds, itemAt, keep, label, occurrences, shows, spotAt, step } from '../src/lib/find.ts'
import { buildView, folds } from '../src/lib/timeline.ts'
import type { StoredEvent } from '../src/proto/StoredEvent.ts'

const found = (text: string, query: string) => occurrences(text, query).map(([a, b]) => text.slice(a, b))

test('find ignores case beyond ASCII and maps each match back to the text as written', () => {
  assert.deepEqual(found("L'ÉCOLE est fermée, école, École", 'école'), ['ÉCOLE', 'école', 'École'])
  assert.deepEqual(found('FERMÉE', 'fermée'), ['FERMÉE'])
  assert.deepEqual(found('İstanbul, istanbul', 'stanbul'), ['stanbul', 'stanbul'])
  assert.deepEqual(found('İ', 'i'), ['İ'], 'a match inside a letter that lowercases to two should cover the letter')
  assert.deepEqual(found('🚀 Deploy, then deploy', 'DEPLOY'), ['Deploy', 'deploy'])
  assert.deepEqual(found('ΣΊΣΥΦΟΣ', 'σίσυφοσ'), ['ΣΊΣΥΦΟΣ'], 'should fold letter by letter, as the hub does')
  assert.equal(found('aaaa', 'aa').length, 2, 'matches should not overlap, as the hub counts them')
  assert.deepEqual(found('anything', ''), [])
})

test('the count reads as the place of the current match', () => {
  const hits = [
    { seq: 10, n: 2 },
    { seq: 40, n: 1 },
  ]
  assert.equal(label(0, hits, false), '1 of 3')
  assert.equal(label(2, hits, true), '3 of 3+')
  assert.equal(label(-1, [], false), 'No results')
  assert.deepEqual(spotAt(hits, 2), { hit: 1, at: 0 })
  assert.equal(step(2, 3, 1), 0, 'next after the last wraps to the first')
  assert.equal(step(0, 3, -1), 2, 'previous before the first wraps to the last')
})

test('new messages keep the current match where it was', () => {
  const before = [
    { seq: 10, n: 2 },
    { seq: 40, n: 1 },
  ]
  assert.equal(keep(before, 1, [...before, { seq: 90, n: 3 }]), 1)
  assert.equal(
    keep(before, 2, [
      { seq: 40, n: 1 },
      { seq: 90, n: 3 },
    ]),
    0,
    'when older matches fall off the cap',
  )
  assert.equal(
    keep(before, 1, [
      { seq: 5, n: 1 },
      { seq: 40, n: 1 },
    ]),
    1,
    'when its message is gone, the next one',
  )
  assert.equal(keep(before, 1, []), -1)
})

let seq = 0
const ev = (e: object) => ({ seq: ++seq, ts: seq * 1000, ...e }) as unknown as StoredEvent
const ask = (turn: string, text: string) => [
  ev({ kind: 'turn_started', turn, input: [{ type: 'text', text }], settings: {} }),
  ev({ kind: 'item_completed', turn, item: { id: `${turn}:input`, type: 'user_message', content: [{ type: 'text', text }] } }),
]
const say = (turn: string, id: string, text: string) => ev({ kind: 'item_completed', turn, item: { id, type: 'agent_message', text } })
const bash = (turn: string, id: string) =>
  ev({
    kind: 'item_completed',
    turn,
    item: { id, type: 'tool_call', name: 'Bash', input: { command: 'ls' }, status: 'completed', output: { text: 'src', is_error: false } },
  })
const done = (turn: string) => ev({ kind: 'turn_completed', turn, outcome: 'completed' })

test('a match the window hasnt loaded is found once the turns down to it are', () => {
  seq = 0
  const all: StoredEvent[] = []
  for (const t of ['t1', 't2', 't3', 't4']) all.push(...ask(t, `ask ${t}`), bash(t, `${t}-b`), say(t, `${t}-m`, 'done'), done(t))
  const late = say('t1', 'late', 'the narwhal job finished')
  all.push(late)
  const window = all.filter((e) => e.seq >= all.find((x) => x.kind === 'turn_started' && x.turn === 't3')!.seq)

  assert.deepEqual(itemAt(window, late.seq), { turn: 't1', item: 'late' })
  assert.equal(shows(buildView(window, true), 'late'), false, 'its turn is not loaded, so the thread cannot show it yet')
  assert.equal(itemAt(window, all[0].seq), undefined)
  assert.deepEqual(itemAt(all, all[0].seq), { turn: 't1', item: 't1:input' })
  assert.equal(shows(buildView(all, false), 'late'), true)
  assert.equal(shows(buildView(all, false), 't2:input'), true)
})

test('only the fold that holds a message opens for it', () => {
  seq = 0
  const v = buildView(
    [
      ...ask('t1', 'fix it'),
      bash('t1', 'a'),
      say('t1', 'interim', 'the zebra test fails'),
      bash('t1', 'b'),
      say('t1', 'final', 'fixed'),
      done('t1'),
    ],
    false,
  )
  const t = v.turns[0]
  const [only] = folds(t.id, t.entries)
  assert.equal(holds(only.work, 'interim'), true)
  assert.equal(holds(only.work, 'final'), false, 'the final answer is shown outside the fold')
  assert.equal(shows(v, 'interim'), true)
})
