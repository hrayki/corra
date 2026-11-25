import { call } from './store'

export type DirList = {
  path: string
  exists: boolean
  git: boolean
  parent: string | null
  home: string
  entries: { name: string; path: string; git: boolean }[]
}

export type ClaudeSession = { id: string; cwd: string; title: string; updated_at: number; size: number; project: string | null }

export const folders = (machine: string, path: string) => call<DirList>({ op: 'list_folders', machine, path })

export const claudeSessions = (machine: string) => call<ClaudeSession[]>({ op: 'claude_sessions', machine })

export const importSession = (machine: string, session: string, project?: string) =>
  call<{ thread: string; project: string }>({ op: 'import_session', machine, session, ...(project ? { project } : {}) })
