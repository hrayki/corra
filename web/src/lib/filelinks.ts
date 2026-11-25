export type FileRef = { path: string; line?: number }

const KNOWN_EXT = new Set(
  (
    'ts tsx js jsx mjs cjs json jsonc rs py go rb java kt kts swift c h cc cpp hpp cs php lua sh bash zsh fish ' +
    'md mdx txt toml yaml yml ini cfg lock sql proto graphql css scss html htm vue svelte xml svg png jpg jpeg ' +
    'gif webp ico pdf csv gradle dart exs scala zig patch diff'
  ).split(' '),
)
const KNOWN_NAMES = new Set(['Dockerfile', 'Makefile', 'Justfile', 'LICENSE', 'Procfile', 'Gemfile', 'Rakefile', 'Caddyfile'])
const SHAPE = /^(~\/|\/|\.{1,2}\/)?[\w.@+-]+(\/[\w.@+-]+)*$/

function plausible(path: string, slash: boolean) {
  if (!SHAPE.test(path) || path.length > 300) return false
  const base = path.split('/').pop()!
  if (KNOWN_NAMES.has(base)) return true
  const dot = base.lastIndexOf('.')
  if (dot < 0 || base.length < 2) return false
  if (dot === 0) return !KNOWN_EXT.has(base.slice(1).toLowerCase())
  const ext = base.slice(dot + 1)
  if (KNOWN_EXT.has(ext.toLowerCase())) return true
  return slash && /^[a-z][\w-]{0,15}$/i.test(ext)
}

function splitLine(s: string): FileRef {
  const m = /^(.*?)(?::(\d+)(?:[-:]\d+)?)?$/.exec(s)!
  return { path: m[1], line: m[2] ? Number(m[2]) : undefined }
}

export function fileInCode(text: string): FileRef | null {
  const t = text.trim()
  if (!t || /\s/.test(t) || t.includes('://')) return null
  const ref = splitLine(t)
  return plausible(ref.path, ref.path.includes('/')) ? ref : null
}

export function fileInHref(href: string): FileRef | null {
  let h = href.trim()
  if (!h || h.startsWith('#') || h.startsWith('//')) return null
  if (h.startsWith('file://')) h = h.slice(7)
  else if (/^[a-z][a-z0-9+.-]*:/i.test(h)) return null
  let line: number | undefined
  const hash = h.indexOf('#')
  if (hash >= 0) {
    const m = /^L(\d+)/.exec(h.slice(hash + 1))
    if (m) line = Number(m[1])
    h = h.slice(0, hash)
  }
  h = h.split('?')[0]
  try {
    h = decodeURIComponent(h)
  } catch {
    return null
  }
  const ref = splitLine(h)
  if (!ref.path || ref.path.endsWith('/')) return null
  return { path: ref.path, line: line ?? ref.line }
}

export function joinPath(base: string, rel: string) {
  if (rel.startsWith('/') || rel.startsWith('~')) return rel
  const out = base ? base.split('/').filter(Boolean) : []
  const abs = base.startsWith('/')
  for (const part of rel.split('/')) {
    if (part === '' || part === '.') continue
    if (part === '..') out.pop()
    else out.push(part)
  }
  return (abs ? '/' : '') + out.join('/')
}

export function inside(cwd: string, path: string): string {
  const root = cwd.replace(/\/+$/, '')
  if (root && path.startsWith(root + '/')) return path.slice(root.length + 1)
  return path
}
