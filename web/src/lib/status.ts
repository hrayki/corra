import type { ThreadStatus } from '../proto/ThreadStatus'

export type Shown = ThreadStatus | 'done'

const URGENCY: Shown[] = ['waiting', 'error', 'done']

export function shownStatus(status: ThreadStatus, unread: boolean, active: boolean): Shown {
  return status === 'idle' && unread && !active ? 'done' : status
}

export function needsYou(s: Shown) {
  return URGENCY.includes(s)
}

export function mostUrgent(shown: Shown[]): Shown | undefined {
  return URGENCY.find((s) => shown.includes(s))
}
