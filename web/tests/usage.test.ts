import { test } from 'node:test'
import assert from 'node:assert/strict'
import { activeWindows, formatReset, formatUpdatedAgo, limitedNow, topUsagePct } from '../src/lib/usage.ts'
import type { AgentUsage } from '../src/proto/AgentUsage.ts'

const now = 1_775_880_000_000

const sample = (overrides: Partial<AgentUsage> = {}): AgentUsage => ({
  agent: 'claude',
  updated_at: now - 120_000,
  windows: [
    { key: 'five_hour', label: '5-hour', used_pct: 42, resets_at: now + 2 * 3600_000 + 15 * 60_000, blocked: false },
    { key: 'seven_day', label: 'Weekly', used_pct: 18, resets_at: now + 3 * 86_400_000, blocked: false },
  ],
  refreshable: true,
  ...overrides,
})

test('expired windows auto-reset instead of staying blocked or stale', () => {
  const usage = sample({
    windows: [
      { key: 'five_hour', label: '5-hour', used_pct: 100, resets_at: now - 1000, blocked: true },
      { key: 'seven_day', label: 'Weekly', used_pct: 25, resets_at: now + 86_400_000, blocked: false },
      { key: 'quota', label: 'Quota', used_pct: 100, resets_at: now - 1000, blocked: true, detail: 'Out of quota' },
    ],
  })
  const active = activeWindows(usage, now)
  assert.equal(active.length, 2)
  assert.equal(active[0].key, 'five_hour')
  assert.equal(active[0].used_pct, 0)
  assert.equal(active[0].blocked, false)
  assert.equal(active[0].resets_at, undefined)
  assert.equal(limitedNow(usage, now), false)
  assert.equal(topUsagePct(usage, now), 25)
})

test('limitedNow reflects only active blocked windows for that agent', () => {
  const blocked = sample({
    windows: [{ key: 'five_hour', label: '5-hour', used_pct: 100, resets_at: now + 600_000, blocked: true }],
  })
  assert.equal(limitedNow(blocked, now), true)
  assert.equal(topUsagePct(blocked, now), 100)
  assert.equal(limitedNow(undefined, now), false)
})

test('formatReset formats minutes, hours, and multi-day resets cleanly', () => {
  assert.equal(formatReset(now - 500, now), 'Resetting')
  assert.equal(formatReset(now + 30_000, now), 'Resets in <1m')
  assert.equal(formatReset(now + 24 * 60_000, now), 'Resets in 24m')
  assert.match(formatReset(now + 2 * 3600_000 + 15 * 60_000, now), /^Resets in 2h 15m · /)
  assert.match(formatReset(now + 3 * 86_400_000, now), /^Resets /)
})

test('formatUpdatedAgo handles never checked and recent timestamps', () => {
  assert.equal(formatUpdatedAgo(0, now), 'Not checked yet')
  assert.equal(formatUpdatedAgo(now - 10_000, now), 'Updated just now')
  assert.equal(formatUpdatedAgo(now - 45_000, now), 'Updated 45s ago')
})
