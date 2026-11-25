import { Show, createEffect, createSignal, on, onCleanup, onMount } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import ArrowRight from 'lucide-solid/icons/arrow-right'
import RotateCw from 'lucide-solid/icons/rotate-cw'
import { call, listenBrowser, state } from '../lib/store'
import { IconBtn } from './bits'

const mods = (e: MouseEvent | KeyboardEvent) => (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0)
const BUTTONS = ['left', 'middle', 'right'] as const

export function BrowserPanel(props: { thread: string }) {
  let box!: HTMLDivElement
  let img!: HTMLImageElement
  const [frame, setFrame] = createSignal<{ w: number; h: number } | null>(null)
  const [url, setUrl] = createSignal('')
  const [typing, setTyping] = createSignal(false)
  const [error, setError] = createSignal('')
  const machine = () => state.threads[props.thread]?.machine
  let stop: (() => void) | undefined
  let opened = false
  let lastMove = 0
  let size = ''

  const send = (method: string, params: Record<string, unknown> = {}) =>
    void call({ op: 'browser_input', thread: props.thread, method, params }).catch((e) => setError((e as Error).message))

  const open = async () => {
    const m = machine()
    if (!m) return setError('This thread has no machine yet.')
    setError('')
    const r = box.getBoundingClientRect()
    size = `${Math.round(r.width)}x${Math.round(r.height)}`
    stop?.()
    stop = listenBrowser(m, (f) => {
      img.src = `data:image/jpeg;base64,${f.data}`
      setFrame({ w: f.width, h: f.height })
      if (!typing()) setUrl(f.url === 'about:blank' ? '' : f.url)
    })
    try {
      const info = await call<{ url: string }>({
        op: 'browser_open',
        thread: props.thread,
        width: Math.round(r.width),
        height: Math.round(r.height),
      })
      opened = true
      if (!typing()) setUrl(info.url === 'about:blank' ? '' : info.url)
    } catch (e) {
      setError((e as Error).message)
    }
  }

  onMount(() => {
    void open()
    let t: number | undefined
    const ro = new ResizeObserver(() => {
      clearTimeout(t)
      t = window.setTimeout(() => {
        const r = box.getBoundingClientRect()
        if (opened && size !== `${Math.round(r.width)}x${Math.round(r.height)}`) void open()
      }, 400)
    })
    ro.observe(box)
    onCleanup(() => ro.disconnect())
  })
  createEffect(on(machine, () => opened && void open(), { defer: true }))
  onCleanup(() => {
    stop?.()
    if (opened) void call({ op: 'browser_close', thread: props.thread }).catch(() => {})
  })

  const point = (e: MouseEvent) => {
    const r = img.getBoundingClientRect()
    const f = frame()
    if (!f || !r.width) return { x: 0, y: 0 }
    return { x: Math.round(((e.clientX - r.left) / r.width) * f.w), y: Math.round(((e.clientY - r.top) / r.height) * f.h) }
  }
  const mouse = (type: string, e: MouseEvent, extra: Record<string, unknown> = {}) =>
    send('Input.dispatchMouseEvent', {
      type,
      ...point(e),
      button: BUTTONS[e.button] ?? 'none',
      buttons: e.buttons,
      modifiers: mods(e),
      ...extra,
    })
  const key = (type: 'keyDown' | 'keyUp', e: KeyboardEvent) => {
    e.preventDefault()
    const printable = e.key.length === 1 && !e.ctrlKey && !e.metaKey
    send('Input.dispatchKeyEvent', {
      type: type === 'keyDown' && !printable ? 'rawKeyDown' : type,
      key: e.key,
      code: e.code,
      windowsVirtualKeyCode: e.keyCode,
      modifiers: mods(e),
      ...(type === 'keyDown' && printable ? { text: e.key } : {}),
      ...(type === 'keyDown' && e.key === 'Enter' ? { type: 'keyDown', text: '\r' } : {}),
    })
  }
  const go = () => {
    let u = url().trim()
    if (!u) return
    if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(u) && !/^about:/i.test(u))
      u = /^(localhost|127\.|\[::1\]|0\.0\.0\.0)/.test(u) ? `http://${u}` : `https://${u}`
    setTyping(false)
    send('Page.navigate', { url: u })
  }

  return (
    <div class="flex h-full flex-col">
      <div class="flex shrink-0 items-center gap-1 border-b border-line px-2 py-1.5">
        <IconBtn label="Back" onClick={() => send('back')}>
          <ArrowLeft size={15} />
        </IconBtn>
        <IconBtn label="Forward" onClick={() => send('forward')}>
          <ArrowRight size={15} />
        </IconBtn>
        <IconBtn label="Reload" onClick={() => send('Page.reload')}>
          <RotateCw size={14} />
        </IconBtn>
        <input
          class="h-8 min-w-0 flex-1 rounded-lg bg-ink/[0.05] px-3 font-mono text-[12px] placeholder:text-faint focus:bg-ink/[0.08]"
          placeholder="localhost:3000"
          spellcheck={false}
          value={url()}
          onFocus={() => setTyping(true)}
          onBlur={() => setTyping(false)}
          onInput={(e) => setUrl(e.currentTarget.value)}
          onKeyDown={(e) => e.key === 'Enter' && go()}
        />
      </div>
      <div
        ref={box}
        tabindex={0}
        class="relative min-h-0 flex-1 overflow-hidden bg-panel outline-none"
        onKeyDown={(e) => key('keyDown', e)}
        onKeyUp={(e) => key('keyUp', e)}
      >
        <img
          ref={img}
          alt=""
          draggable={false}
          class="block max-h-full max-w-full select-none"
          classList={{ invisible: !frame() }}
          onMouseDown={(e) => {
            box.focus()
            mouse('mousePressed', e, { clickCount: e.detail || 1 })
          }}
          onMouseUp={(e) => mouse('mouseReleased', e, { clickCount: e.detail || 1 })}
          onMouseMove={(e) => {
            const now = performance.now()
            if (now - lastMove < 40) return
            lastMove = now
            mouse('mouseMoved', e)
          }}
          onWheel={(e) => {
            e.preventDefault()
            mouse('mouseWheel', e, { deltaX: e.deltaX, deltaY: e.deltaY })
          }}
          onContextMenu={(e) => e.preventDefault()}
        />
        <Show when={error() || !frame()}>
          <div class="absolute inset-0 flex items-center justify-center p-6 text-center text-[13px] text-mute">
            <Show when={error()} fallback="Starting the browser">
              <div>
                <div>
                  {error().includes('no Chromium') ? 'This machine has no browser. Remote machines corra makes come with one.' : error()}
                </div>
                <button class="mt-2 text-ink underline decoration-line underline-offset-4" onClick={() => void open()}>
                  Try again
                </button>
              </div>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  )
}
