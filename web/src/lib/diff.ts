export type DiffLine = { kind: ' ' | '+' | '-'; text: string; old?: number; new?: number }
export type Hunk = { header: string; lines: DiffLine[] }
export type FileDiff = {
  path: string
  oldPath?: string
  status: 'modified' | 'added' | 'deleted' | 'renamed'
  binary: boolean
  hunks: Hunk[]
  added: number
  removed: number
}

export function parseDiff(text: string): FileDiff[] {
  const files: FileDiff[] = []
  let f: FileDiff | undefined
  let h: Hunk | undefined
  let oldN = 0
  let newN = 0
  for (const line of text.split('\n')) {
    if (line.startsWith('diff --git ')) {
      const m = line.match(/^diff --git a\/(.*) b\/(.*)$/)
      f = { path: m?.[2] ?? line, status: 'modified', binary: false, hunks: [], added: 0, removed: 0 }
      files.push(f)
      h = undefined
      continue
    }
    if (!f) continue
    if (line.startsWith('new file mode')) f.status = 'added'
    else if (line.startsWith('deleted file mode')) f.status = 'deleted'
    else if (line.startsWith('rename from ')) {
      f.status = 'renamed'
      f.oldPath = line.slice(12)
    } else if (line.startsWith('Binary files')) f.binary = true
    else if (line.startsWith('@@')) {
      const m = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/)
      oldN = Number(m?.[1] ?? 0)
      newN = Number(m?.[2] ?? 0)
      h = { header: m?.[3]?.trim() ?? '', lines: [] }
      f.hunks.push(h)
    } else if (h) {
      if (line.startsWith('+')) {
        h.lines.push({ kind: '+', text: line.slice(1), new: newN++ })
        f.added++
      } else if (line.startsWith('-')) {
        h.lines.push({ kind: '-', text: line.slice(1), old: oldN++ })
        f.removed++
      } else if (line.startsWith(' ')) h.lines.push({ kind: ' ', text: line.slice(1), old: oldN++, new: newN++ })
    }
  }
  return files
}
