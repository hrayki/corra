import { createStore } from 'solid-js/store'
import { call } from './store'

export type ChangedFile = { path: string; added: number; removed: number; binary: boolean; untracked: boolean }
export type RepoChanges = { repo: string; base: string; files: ChangedFile[] }
export type Changes = { repos: RepoChanges[]; at: number; error?: string }

const KEY = 'corra:changes:'
const [changes, setChanges] = createStore<Record<string, Changes | undefined>>({})
const inFlight = new Set<string>()

export function changesOf(thread: string): Changes | undefined {
  if (!(thread in changes)) {
    try {
      const saved = localStorage.getItem(KEY + thread)
      if (saved) setChanges(thread, JSON.parse(saved) as Changes)
    } catch {}
  }
  return changes[thread]
}

export function totals(thread: string) {
  let files = 0
  let added = 0
  let removed = 0
  for (const r of changesOf(thread)?.repos ?? [])
    for (const f of r.files) {
      files++
      added += f.added
      removed += f.removed
    }
  return { files, added, removed }
}

export async function refreshChanges(thread: string) {
  if (inFlight.has(thread)) return
  inFlight.add(thread)
  try {
    const repos = await call<RepoChanges[]>({ op: 'changes', thread })
    const v: Changes = { repos, at: Date.now() }
    setChanges(thread, v)
    try {
      localStorage.setItem(KEY + thread, JSON.stringify(v))
    } catch {}
  } catch (e) {
    const prev = changesOf(thread)
    setChanges(thread, { repos: prev?.repos ?? [], at: prev?.at ?? 0, error: (e as Error).message })
  } finally {
    inFlight.delete(thread)
  }
}
