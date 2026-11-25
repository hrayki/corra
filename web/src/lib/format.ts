export function ago(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000))
  if (s < 60) return 'now'
  const m = Math.round(s / 60)
  if (m < 60) return `${m}m`
  const h = Math.round(m / 60)
  if (h < 24) return `${h}h`
  const d = Math.round(h / 24)
  if (d < 30) return `${d}d`
  return `${Math.round(d / 30)}mo`
}

export function duration(ms: number): string {
  const s = Math.max(1, Math.round(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`
}

export function shortPath(p: unknown, cwd?: string): string {
  let s = String(p ?? '')
  if (cwd && s.startsWith(cwd + '/')) s = s.slice(cwd.length + 1)
  s = s.replace(/^\/(root|home\/[^/]+)\/projects\/[^/]+\//, '')
  return s
}

export function tokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1000) return `${Math.round(n / 1000)}k`
  return String(n)
}

export function modelLabel(id?: string): string {
  if (!id) return 'Default'
  const m = id.match(/claude-([a-z]+)-(\d+)-(\d+)/)
  if (m) return `${m[1][0].toUpperCase()}${m[1].slice(1)} ${m[2]}.${m[3]}`
  return id
}

export function clock(ms: number): string {
  return ms - Date.now() > 20 * 3600_000
    ? new Date(ms).toLocaleString([], { weekday: 'short', hour: 'numeric', minute: '2-digit' })
    : new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
}
