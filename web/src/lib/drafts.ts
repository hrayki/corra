const KEY = 'corra:draft:'

type Image = { media_type: string; data: string }
const IMAGES = 'corra:draft-images'
const images = new Map<string, Image[]>(
  (() => {
    try {
      const saved = sessionStorage.getItem(IMAGES)
      sessionStorage.removeItem(IMAGES)
      return saved ? (JSON.parse(saved) as [string, Image[]][]) : []
    } catch {
      return []
    }
  })(),
)

export function persistDrafts() {
  try {
    if (images.size) sessionStorage.setItem(IMAGES, JSON.stringify([...images]))
  } catch {}
}

export function loadDraft(key: string): { text: string; images: Image[] } {
  let text = ''
  try {
    text = localStorage.getItem(KEY + key) ?? ''
  } catch {}
  return { text, images: images.get(key) ?? [] }
}

export function saveDraftText(key: string, text: string) {
  try {
    if (text) localStorage.setItem(KEY + key, text)
    else localStorage.removeItem(KEY + key)
  } catch {}
}

export function saveDraftImages(key: string, list: Image[]) {
  if (list.length) images.set(key, list)
  else images.delete(key)
}

const ITEM = /^(\s*)(?:(\d+)([.)])|([-*+]))(\s+)(.*)$/

export function continueList(value: string, start: number, end: number): { value: string; caret: number } | null {
  if (start !== end) return null
  const lineStart = value.lastIndexOf('\n', start - 1) + 1
  const nl = value.indexOf('\n', start)
  const lineEnd = nl < 0 ? value.length : nl
  const m = ITEM.exec(value.slice(lineStart, lineEnd))
  if (!m) return null
  const [, indent, num, punct, bullet, gap, content] = m
  const markerEnd = lineStart + indent.length + (num ? num.length + 1 : 1) + gap.length
  if (start < markerEnd) return null
  if (!content.trim()) {
    return { value: value.slice(0, lineStart) + value.slice(lineEnd), caret: lineStart }
  }
  const next = `\n${indent}${num ? `${Number(num) + 1}${punct}` : bullet} `
  return { value: value.slice(0, start) + next + value.slice(start), caret: start + next.length }
}
