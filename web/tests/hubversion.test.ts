import { test } from 'node:test'
import assert from 'node:assert/strict'
import { runningLine } from '../src/lib/hubversion.ts'
import type { HubVersion } from '../src/proto/HubVersion.ts'

const hub = (o: Partial<HubVersion>): HubVersion => ({
  version: '2026.11.26-150000-1111111',
  managed: true,
  trial: false,
  previous_too_old: false,
  ...o,
})

test('a hub that has never heard back from the release server is not called up to date', () => {
  assert.doesNotMatch(runningLine(hub({})).text, /up to date/)
  assert.doesNotMatch(runningLine(hub({ checked_at: Date.now() })).text, /up to date/)
  const failed = runningLine(
    hub({ checked_at: Date.now(), check_error: 'http://x/latest/download/VERSION didn’t answer within 8 seconds' }),
  )
  assert.equal(failed.bad, true)
  assert.match(failed.text, /Couldn't check/)
  assert.match(runningLine(hub({ latest: '2026.11.26-150000-1111111', checked_at: Date.now() })).text, /up to date, checked just now/)
})
