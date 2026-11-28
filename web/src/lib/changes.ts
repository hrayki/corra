import { createStore } from 'solid-js/store'
import { call } from './store'

export type ChangedFile = { path: string; added: number; removed: number; binary: boolean; untracked: boolean }
export type RepoChanges = { repo: string; base: string; branch?: string; files: ChangedFile[] }
export type Changes = { repos: RepoChanges[]; at: number; error?: string }

export type BranchPr = {
  repo: string
  number: number
  title: string
  url: string
  draft: boolean
  state: string
  head: string
  base: string
  checks?: { total: number; failing: number; pending: number } | null
}

const [changes, setChanges] = createStore<Record<string, Changes | undefined>>({})
const [prs, setPrs] = createStore<Record<string, BranchPr | null | undefined>>({})
const inFlight = new Set<string>()
const prInFlight = new Map<string, string>()

export function changesOf(thread: string): Changes | undefined {
  return changes[thread]
}

export function primaryBranch(thread: string): { repo?: string; branch: string } | undefined {
  const repos = changes[thread]?.repos ?? []
  for (const r of repos) {
    if (r.branch) return { repo: r.repo === '.' ? undefined : r.repo, branch: r.branch }
  }
  return undefined
}

export function prOf(thread: string): BranchPr | null | undefined {
  return prs[thread]
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
    const b = repos.find((r) => r.branch)
    if (b?.branch) {
      void refreshPr(thread, b.repo === '.' ? undefined : b.repo, b.branch)
    } else {
      setPrs(thread, null)
    }
  } catch (e) {
    const prev = changesOf(thread)
    setChanges(thread, { repos: prev?.repos ?? [], at: prev?.at ?? 0, error: (e as Error).message })
  } finally {
    inFlight.delete(thread)
  }
}

export async function refreshPr(thread: string, repo: string | undefined, branch: string) {
  if (!branch || branch === 'HEAD' || branch === 'main' || branch === 'master') {
    setPrs(thread, null)
    return
  }
  const key = `${repo ?? ''}:${branch}`
  if (prInFlight.get(thread) === key) return
  prInFlight.set(thread, key)
  try {
    const r = await call<BranchPr | null>({ op: 'thread_pr', thread, ...(repo ? { repo } : {}), branch })
    setPrs(thread, r ?? null)
  } catch {
    setPrs(thread, null)
  } finally {
    if (prInFlight.get(thread) === key) prInFlight.delete(thread)
  }
}
