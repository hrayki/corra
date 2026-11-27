import { test } from 'node:test'
import assert from 'node:assert/strict'
import { approvalBody, approvalHeading } from '../src/lib/approval.ts'

test('legacy acp:other with only a title shows it in the heading and omits an empty box', () => {
  const input = { title: 'Open local app in browser' }
  const heading = approvalHeading('acp:other', input)
  assert.equal(heading, 'Allow Open local app in browser?')
  const body = approvalBody(input, { verb: 'Open local app in browser', detail: '' }, heading)
  assert.equal(body, '')
})

test('acp:other with long title keeps short heading and puts full title in body', () => {
  const long = 'A'.repeat(90)
  const input = { title: long }
  const heading = approvalHeading('acp:other', input)
  assert.equal(heading, 'Allow this?')
  assert.equal(approvalBody(input, { verb: long, detail: '' }, heading), long)
})

test('unknown or ACP other tools format remaining input arguments instead of rendering blank', () => {
  const input = { title: 'Navigate browser', url: 'http://127.0.0.1:18767/#/t/1', toolSummary: 'Open thread' }
  const heading = approvalHeading('Navigate browser', input)
  assert.equal(heading, 'Use Navigate browser?')
  const body = approvalBody(input, { verb: 'Navigate browser', detail: '' }, heading)
  assert.equal(body, 'url: http://127.0.0.1:18767/#/t/1\ntoolSummary: Open thread')
})

test('empty input produces empty body so the UI hides the preview box', () => {
  const heading = approvalHeading('acp:other', {})
  assert.equal(heading, 'Allow this?')
  assert.equal(approvalBody({}, { verb: 'Tool', detail: '' }, heading), '')
})
