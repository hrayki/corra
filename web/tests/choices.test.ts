import { test } from 'node:test'
import assert from 'node:assert/strict'
import { pickerState } from '../src/lib/choices.ts'
import type { Choice } from '../src/proto/Choice.ts'

const efforts: Choice[] = [
  { id: 'low', name: 'Low' },
  { id: 'medium', name: 'Medium' },
  { id: 'high', name: 'High' },
  { id: 'xhigh', name: 'xHigh' },
  { id: 'max', name: 'Max' },
]
const claude: Choice[] = [
  {
    id: 'default',
    name: 'Default (recommended)',
    hint: 'Sonnet 5.5 · Efficient for routine tasks',
    resolved_model: 'claude-sonnet-5-5',
    supports_auto_mode: true,
    efforts,
  },
  { id: 'sonnet', name: 'Sonnet', resolved_model: 'claude-sonnet-5-5', supports_auto_mode: true, efforts },
  { id: 'opus', name: 'Opus', resolved_model: 'claude-opus-5-5', supports_auto_mode: true, efforts },
  { id: 'haiku', name: 'Haiku', resolved_model: 'claude-haiku-5-5', supports_auto_mode: false, efforts: efforts.slice(0, 3) },
]

test("a thread saved with a full model id shows Claude Code's own name for it", () => {
  const s = pickerState({
    agent: 'claude',
    settings: { model: 'claude-opus-5-5', effort: 'xhigh', permission_mode: 'default' },
    models: claude,
  })
  assert.equal(s.model?.id, 'opus')
  assert.equal(s.modelName, 'Opus')
  assert.equal(s.effortName, 'xHigh')
  assert.equal(s.mode.name, 'Manual')
  const sonnet = pickerState({ agent: 'claude', settings: { model: 'claude-sonnet-5-5' }, models: claude })
  assert.equal(sonnet.model?.id, 'sonnet')
})

test('before Claude Code has listed its models, a thread shows the id it was saved with', () => {
  const s = pickerState({ agent: 'claude', settings: { model: 'claude-opus-5-5', effort: 'high' }, models: [] })
  assert.equal(s.modelName, 'claude-opus-5-5')
  assert.equal(s.effortName, '')
  assert.ok(!s.modes.some((m) => m.id === 'auto'))
  assert.equal(pickerState({ agent: 'claude', settings: {}, models: [] }).modelName, 'Default model')
})

test('a thread with no model is on Claude Code’s default', () => {
  const s = pickerState({ agent: 'claude', settings: {}, models: claude })
  assert.equal(s.modelName, 'Default (recommended)')
})

test('auto is offered only with a model that supports it, and a thread in auto still says so', () => {
  const ids = (model: string) => pickerState({ agent: 'claude', settings: { model }, models: claude }).modes.map((m) => m.id)
  assert.deepEqual(ids('opus'), ['default', 'acceptEdits', 'plan', 'auto', 'bypassPermissions'])
  assert.deepEqual(ids('haiku'), ['default', 'acceptEdits', 'plan', 'bypassPermissions'])
  const s = pickerState({ agent: 'claude', settings: { model: 'haiku', permission_mode: 'auto' }, models: claude })
  assert.equal(s.mode.name, 'Auto')
})

test("an Antigravity thread whose saved mode agy doesn't offer shows agy's current mode", () => {
  const modes: Choice[] = [
    { id: 'default', name: 'Default' },
    { id: 'accept-edits', name: 'Accept edits' },
    { id: 'plan', name: 'Plan' },
  ]
  for (const saved of ['bypassPermissions', 'acceptEdits']) {
    const s = pickerState({ agent: 'antigravity', settings: { permission_mode: saved }, models: [], modes, mode: 'plan' })
    assert.equal(s.mode.id, 'plan', saved)
    assert.deepEqual(
      s.modes.map((m) => m.id),
      ['default', 'accept-edits', 'plan'],
    )
  }
})
