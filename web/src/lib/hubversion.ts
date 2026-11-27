import type { HubVersion } from '../proto/HubVersion'
import { ago } from './format.ts'

export const when = (ms: number) => (ago(ms) === 'now' ? 'just now' : `${ago(ms)} ago`)

export function runningLine(i: HubVersion): { text: string; bad?: boolean } {
  if (!i.managed) return { text: "Running. This hub isn't run by corra's launcher, so it's updated by deploying it again." }
  if (i.trial && i.previous) return { text: "Just updated. If it stops in its first minute, it's rolled back." }
  if (i.check_error) return { text: `Running. Couldn't check for a newer version: ${i.check_error}`, bad: true }
  if (i.update) return { text: 'Running' }
  if (i.latest && i.checked_at) return { text: `Running · up to date, checked ${when(i.checked_at)}` }
  return { text: "Running · hasn't checked for a newer version yet" }
}
