import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mostUrgent, shownStatus } from '../src/lib/status.ts'

test('a finished thread shows as done until it is opened', () => {
  assert.equal(shownStatus('idle', true, false), 'done')
  assert.equal(shownStatus('idle', true, true), 'idle')
  assert.equal(shownStatus('idle', false, false), 'idle')
})

test('a live status wins over unread', () => {
  assert.equal(shownStatus('waiting', true, false), 'waiting')
  assert.equal(shownStatus('error', true, false), 'error')
  assert.equal(shownStatus('running', true, false), 'running')
  assert.equal(shownStatus('starting', false, true), 'starting')
})

test('a collapsed project flags its most urgent thread', () => {
  assert.equal(mostUrgent(['idle', 'done', 'running', 'error']), 'error')
  assert.equal(mostUrgent(['done', 'waiting', 'error']), 'waiting')
  assert.equal(mostUrgent(['idle', 'done']), 'done')
  assert.equal(mostUrgent(['idle', 'running', 'starting']), undefined)
  assert.equal(mostUrgent([]), undefined)
})
