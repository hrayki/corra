const LEGACY_ACP: Record<string, string> = {
  'acp:read': 'Read',
  'acp:edit': 'Edit',
  'acp:delete': 'Delete',
  'acp:move': 'Move',
  'acp:search': 'Grep',
  'acp:execute': 'Bash',
  'acp:think': 'Think',
  'acp:fetch': 'WebFetch',
  'acp:switch_mode': 'SwitchMode',
}

function asObject(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
}

function nonEmptyString(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

export function canonicalApprovalTool(rawTool: string): string {
  return LEGACY_ACP[rawTool] ?? rawTool
}

export function approvalHeading(rawTool: string, input: unknown): string {
  const tool = canonicalApprovalTool(rawTool)
  const obj = asObject(input)
  const title = nonEmptyString(obj.title)
  switch (tool) {
    case 'Bash':
      return 'Run this command?'
    case 'Write':
      return 'Write this file?'
    case 'Edit':
    case 'MultiEdit':
    case 'NotebookEdit':
      return 'Edit this file?'
    case 'Delete':
      return 'Delete this?'
    case 'Move':
      return 'Move this?'
    case 'WebFetch':
      return 'Fetch this page?'
    case 'Read':
      return 'Read this file?'
    default: {
      if (tool === 'acp:other' || tool === 'Tool' || tool === '') {
        return title && title.length <= 72 ? `Allow ${title}?` : 'Allow this?'
      }
      const m = tool.match(/^mcp__(.+?)__(.+)$/)
      return m ? `Use ${m[2].replace(/_/g, ' ')} from ${m[1]}?` : `Use ${tool}?`
    }
  }
}

export function approvalBody(input: unknown, summary: { verb: string; detail: string; full?: string }, heading: string): string {
  const primary = (summary.full ?? summary.detail).trim()
  if (primary) return primary

  const obj = asObject(input)
  const rest: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(obj)) {
    if (v === null || v === undefined) continue
    if (typeof v === 'string') {
      const trimmed = v.trim()
      if (!trimmed) continue
      if (k === 'title' && heading.includes(trimmed)) continue
      rest[k] = trimmed
      continue
    }
    if (Array.isArray(v) && v.length === 0) continue
    if (typeof v === 'object' && !Array.isArray(v) && Object.keys(v as object).length === 0) continue
    rest[k] = v
  }

  const keys = Object.keys(rest)
  if (keys.length === 0) return ''
  if (keys.length === 1 && typeof rest[keys[0]] === 'string') {
    return rest[keys[0]] as string
  }
  const allScalar = keys.every((k) => {
    const v = rest[k]
    return typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean'
  })
  if (allScalar) {
    return keys.map((k) => `${k}: ${String(rest[k])}`).join('\n')
  }
  return JSON.stringify(rest, null, 2)
}
