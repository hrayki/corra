import { test } from 'node:test'
import assert from 'node:assert/strict'
import { dayKey, dayLabel } from '../src/lib/format.ts'

test('dayLabel names today, yesterday, and earlier dates cleanly', () => {
  const now = new Date(2026, 10, 28, 14, 0, 0).getTime()
  const todayMorning = new Date(2026, 10, 28, 8, 15, 0).getTime()
  const yesterdayEvening = new Date(2026, 10, 27, 23, 55, 0).getTime()
  const lastMonth = new Date(2026, 9, 15, 12, 0, 0).getTime()
  const lastYear = new Date(2025, 4, 3, 12, 0, 0).getTime()

  assert.equal(dayLabel(todayMorning, now), 'Today')
  assert.equal(dayLabel(yesterdayEvening, now), 'Yesterday')
  assert.notEqual(dayKey(todayMorning), dayKey(yesterdayEvening))
  assert.match(dayLabel(lastMonth, now), /Oct/)
  assert.match(dayLabel(lastYear, now), /2025/)
})
