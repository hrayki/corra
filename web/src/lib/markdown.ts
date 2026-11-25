import { Marked } from 'marked'
import DOMPurify from 'dompurify'

const marked = new Marked({ gfm: true, breaks: false })

export function renderMarkdown(src: string): string {
  return DOMPurify.sanitize(marked.parse(src, { async: false }) as string)
}

const LANGS: Record<string, () => Promise<unknown>> = {
  typescript: () => import('@shikijs/langs/typescript'),
  tsx: () => import('@shikijs/langs/tsx'),
  javascript: () => import('@shikijs/langs/javascript'),
  jsx: () => import('@shikijs/langs/jsx'),
  json: () => import('@shikijs/langs/json'),
  rust: () => import('@shikijs/langs/rust'),
  python: () => import('@shikijs/langs/python'),
  bash: () => import('@shikijs/langs/bash'),
  go: () => import('@shikijs/langs/go'),
  html: () => import('@shikijs/langs/html'),
  css: () => import('@shikijs/langs/css'),
  toml: () => import('@shikijs/langs/toml'),
  yaml: () => import('@shikijs/langs/yaml'),
  markdown: () => import('@shikijs/langs/markdown'),
  sql: () => import('@shikijs/langs/sql'),
  diff: () => import('@shikijs/langs/diff'),
  kotlin: () => import('@shikijs/langs/kotlin'),
  java: () => import('@shikijs/langs/java'),
  swift: () => import('@shikijs/langs/swift'),
  c: () => import('@shikijs/langs/c'),
  cpp: () => import('@shikijs/langs/cpp'),
  dockerfile: () => import('@shikijs/langs/dockerfile'),
}
const ALIAS: Record<string, string> = {
  ts: 'typescript',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  rs: 'rust',
  py: 'python',
  sh: 'bash',
  shell: 'bash',
  zsh: 'bash',
  yml: 'yaml',
  md: 'markdown',
  kt: 'kotlin',
  kts: 'kotlin',
  h: 'c',
  hpp: 'cpp',
  cc: 'cpp',
  'c++': 'cpp',
  htm: 'html',
  svelte: 'html',
  vue: 'html',
  jsonc: 'json',
  docker: 'dockerfile',
  patch: 'diff',
}

export function langOf(nameOrExt: string): string | undefined {
  const k = nameOrExt.toLowerCase()
  const l = ALIAS[k] ?? k
  return LANGS[l] ? l : undefined
}

export function langOfPath(path: string): string | undefined {
  const base = path.split('/').pop() ?? ''
  if (/^dockerfile/i.test(base)) return 'dockerfile'
  const ext = base.includes('.') ? base.split('.').pop()! : ''
  return langOf(ext)
}

type Highlighter = {
  codeToHtml: (code: string, o: object) => string
  loadLanguage: (l: unknown) => Promise<void>
  getLoadedLanguages: () => string[]
}
let hl: Promise<Highlighter> | undefined

async function highlighter(): Promise<Highlighter> {
  hl ??= (async () => {
    const [{ createHighlighterCore }, { createJavaScriptRegexEngine }, light, dark] = await Promise.all([
      import('shiki/core'),
      import('shiki/engine/javascript'),
      import('@shikijs/themes/github-light'),
      import('@shikijs/themes/github-dark-dimmed'),
    ])
    return (await createHighlighterCore({
      themes: [light.default, dark.default],
      langs: [],
      engine: createJavaScriptRegexEngine(),
    })) as unknown as Highlighter
  })()
  return hl
}

export async function highlightCode(code: string, lang: string): Promise<string | undefined> {
  const l = langOf(lang)
  if (!l || code.length > 200_000) return undefined
  const h = await highlighter()
  if (!h.getLoadedLanguages().includes(l)) await h.loadLanguage(((await LANGS[l]()) as { default: unknown }).default)
  return h.codeToHtml(code, { lang: l, themes: { light: 'github-light', dark: 'github-dark-dimmed' }, defaultColor: false })
}

export async function highlightIn(root: HTMLElement) {
  const blocks = root.querySelectorAll<HTMLElement>('pre > code[class*="language-"]:not([data-hl])')
  for (const code of blocks) {
    code.dataset.hl = '1'
    const lang = [...code.classList].find((c) => c.startsWith('language-'))?.slice(9) ?? ''
    const html = await highlightCode(code.textContent ?? '', lang).catch(() => undefined)
    if (!html || !code.isConnected) continue
    const tmp = document.createElement('div')
    tmp.innerHTML = html
    const inner = tmp.querySelector('code')
    if (inner) {
      code.innerHTML = inner.innerHTML
      code.classList.add('shiki')
    }
  }
}
