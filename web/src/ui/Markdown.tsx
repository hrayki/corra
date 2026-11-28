import { Index, Show, createEffect, createMemo, createSignal, on, onCleanup } from 'solid-js'
import { highlightIn, renderMarkdown } from '../lib/markdown'
import { splitMarkdownBlocks } from '../lib/markdownBlocks'
import { fileInCode, fileInHref, joinPath, type FileRef } from '../lib/filelinks'
import { openFile } from '../lib/panel'

function copyButton(text: () => string) {
  const b = document.createElement('button')
  b.className = 'copy'
  b.type = 'button'
  b.textContent = 'Copy'
  b.onclick = () => {
    void navigator.clipboard?.writeText(text())
    b.textContent = 'Copied'
    setTimeout(() => (b.textContent = 'Copy'), 1200)
  }
  return b
}

function decorate(el: HTMLElement, base?: string, skipHighlight = false) {
  if (!skipHighlight) void highlightIn(el)
  markFiles(el, base)
  for (const pre of el.querySelectorAll('pre')) {
    if (pre.parentElement?.classList.contains('codewrap')) continue
    const wrap = document.createElement('div')
    wrap.className = 'codewrap'
    pre.replaceWith(wrap)
    wrap.append(
      pre,
      copyButton(() => pre.innerText),
    )
  }
}

function hasUnclosedFence(text: string): boolean {
  let openChar = ''
  let openLen = 0
  for (const line of text.split('\n')) {
    const m = line.match(/^\s{0,3}(`{3,}|~{3,})/)
    if (!openChar) {
      if (m) {
        openChar = m[1][0]
        openLen = m[1].length
      }
    } else if (line.match(new RegExp(`^\\s{0,3}${openChar}{${openLen},}\\s*$`))) {
      openChar = ''
      openLen = 0
    }
  }
  return openChar !== ''
}

function MarkdownBlock(props: { text: string; base?: string; streaming?: boolean }) {
  let el!: HTMLDivElement
  const html = createMemo(() => renderMarkdown(props.text))
  createEffect(
    on(
      () => [html(), !!props.streaming] as const,
      ([, streaming]) => {
        const skip = streaming && hasUnclosedFence(props.text)
        queueMicrotask(() => {
          if (!el) return
          decorate(el, props.base, skip)
        })
      },
    ),
  )
  return <div ref={el} innerHTML={html()} />
}

export function Markdown(props: { text: string; base?: string; find?: string; streaming?: boolean }) {
  const [shown, setShown] = createSignal(props.text)
  let raf = 0
  createEffect(() => {
    const next = props.text
    if (!props.streaming) {
      if (raf) {
        cancelAnimationFrame(raf)
        raf = 0
      }
      setShown(next)
      return
    }
    if (!raf) {
      raf = requestAnimationFrame(() => {
        raf = 0
        setShown(props.text)
      })
    }
  })
  onCleanup(() => {
    if (raf) cancelAnimationFrame(raf)
  })

  const blocks = createMemo(() => {
    const t = shown()
    if (!props.streaming) return t ? [t] : []
    return splitMarkdownBlocks(t)
  })

  return (
    <div data-find={props.find} class="md text-[14px] leading-[1.65]" onClick={clickFile} onKeyDown={clickFile}>
      <Show when={blocks().length}>
        <Index each={blocks()}>
          {(block, i) => <MarkdownBlock text={block()} base={props.base} streaming={props.streaming && i === blocks().length - 1} />}
        </Index>
      </Show>
    </div>
  )
}

function markFiles(root: HTMLElement, base?: string) {
  const at = (ref: FileRef | null) => (ref && base !== undefined ? { ...ref, path: joinPath(base, ref.path) } : ref)
  for (const a of root.querySelectorAll<HTMLAnchorElement>('a[href]')) {
    const ref = at(fileInHref(a.getAttribute('href')!))
    if (ref) {
      a.dataset.file = ref.path
      if (ref.line) a.dataset.line = String(ref.line)
      a.title = `Open ${ref.path}`
    } else {
      a.target = '_blank'
      a.rel = 'noopener noreferrer'
    }
  }
  for (const code of root.querySelectorAll<HTMLElement>('code')) {
    if (code.closest('pre, a')) continue
    const ref = at(fileInCode(code.textContent ?? ''))
    if (!ref) continue
    code.dataset.file = ref.path
    if (ref.line) code.dataset.line = String(ref.line)
    code.classList.add('filelink')
    code.role = 'link'
    code.tabIndex = 0
    code.title = `Open ${ref.path}`
  }
}

function clickFile(e: MouseEvent | KeyboardEvent) {
  if (e instanceof KeyboardEvent && e.key !== 'Enter') return
  if (e instanceof MouseEvent && e.button !== 0) return
  const t = (e.target as HTMLElement).closest<HTMLElement>('[data-file]')
  const sel = window.getSelection()
  if (!t || (sel && !sel.isCollapsed && sel.containsNode(t, true))) return
  e.preventDefault()
  openFile({ path: t.dataset.file!, line: t.dataset.line ? Number(t.dataset.line) : undefined })
}
