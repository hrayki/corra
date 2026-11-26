import { For, Show, createEffect, createSignal, onCleanup, type JSX } from 'solid-js'
import { Portal } from 'solid-js/web'
import type { ThreadStatus } from '../proto/ThreadStatus'
import type { InputPart } from '../proto/InputPart'

export function StatusMark(props: { status: ThreadStatus; class?: string }) {
  return (
    <span class={`relative inline-flex size-3.5 shrink-0 items-center justify-center ${props.class ?? ''}`} aria-hidden="true">
      <Show when={props.status === 'running' || props.status === 'starting'}>
        <span class="size-2 rounded-full bg-accent pulse" />
      </Show>
      <Show when={props.status === 'waiting'}>
        <span class="size-2 rounded-full bg-warn" />
      </Show>
      <Show when={props.status === 'error'}>
        <span class="size-2 rounded-full bg-bad" />
      </Show>
      <Show when={props.status === 'idle'}>
        <span class="size-1.5 rounded-full bg-faint/60" />
      </Show>
    </span>
  )
}

export const statusWord: Record<ThreadStatus, string> = {
  idle: '',
  starting: 'Starting',
  running: 'Working',
  waiting: 'Needs you',
  error: 'Failed',
}

type BtnProps = {
  kind?: 'primary' | 'plain' | 'ghost' | 'danger'
  type?: 'button' | 'submit'
  small?: boolean
  disabled?: boolean
  title?: string
  class?: string
  onClick?: (e: MouseEvent) => void
  children: JSX.Element
}

export function Btn(props: BtnProps) {
  const kind = () => props.kind ?? 'plain'
  return (
    <button
      type={props.type ?? 'button'}
      title={props.title}
      disabled={props.disabled}
      onClick={props.onClick}
      class={`inline-flex shrink-0 items-center justify-center gap-1.5 rounded-lg font-medium whitespace-nowrap transition-colors duration-100 disabled:opacity-40 ${
        props.small ? 'h-7 px-2.5 text-[12.5px]' : 'h-8 px-3 text-[13px]'
      } ${props.class ?? ''}`}
      classList={{
        'bg-ink text-bg hover:opacity-90': kind() === 'primary',
        'border border-line bg-raised hover:border-faint': kind() === 'plain',
        'text-mute hover:bg-ink/5 hover:text-ink': kind() === 'ghost',
        'border border-bad/40 text-bad hover:bg-bad/10': kind() === 'danger',
      }}
    >
      {props.children}
    </button>
  )
}

export function IconBtn(props: {
  label: string
  active?: boolean
  onClick?: (e: MouseEvent) => void
  class?: string
  children: JSX.Element
}) {
  return (
    <button
      type="button"
      aria-label={props.label}
      title={props.label}
      onClick={props.onClick}
      class={`inline-flex size-8 shrink-0 items-center justify-center rounded-lg text-mute transition-colors duration-100 hover:bg-ink/5 hover:text-ink ${props.class ?? ''}`}
      classList={{ 'bg-ink/5 text-ink': props.active }}
    >
      {props.children}
    </button>
  )
}

export type MenuItem =
  | { label: string; hint?: string; detail?: string; danger?: boolean; checked?: boolean; disabled?: boolean; onSelect: () => void }
  | 'sep'
  | 'col'
  | { heading: string }

export function Menu(props: {
  items: MenuItem[]
  align?: 'start' | 'end'
  side?: 'top' | 'bottom'
  label: string
  class?: string
  children: JSX.Element
}) {
  const [open, setOpen] = createSignal(false)
  const [pos, setPos] = createSignal<{ x: number; y: number; w: number; h: number }>({ x: 0, y: 0, w: 0, h: 0 })
  let btn!: HTMLButtonElement
  const close = () => setOpen(false)
  const columns = () => {
    const out: Exclude<MenuItem, 'col'>[][] = [[]]
    for (const it of props.items) {
      if (it === 'col') out.push([])
      else out[out.length - 1].push(it)
    }
    return out
  }
  const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close()
  const toggle = () => {
    if (open()) return close()
    const r = btn.getBoundingClientRect()
    setPos({ x: r.left, y: r.top, w: r.width, h: r.height })
    setOpen(true)
    document.addEventListener('keydown', onKey)
  }
  onCleanup(() => document.removeEventListener('keydown', onKey))
  const style = (): JSX.CSSProperties => {
    const p = pos()
    const s: JSX.CSSProperties = { position: 'fixed' }
    const above = p.y - 12
    const below = window.innerHeight - p.y - p.h - 12
    const want = props.side ?? 'bottom'
    const room = want === 'top' ? above : below
    const other = want === 'top' ? below : above
    const side = room < 320 && other > room ? (want === 'top' ? 'bottom' : 'top') : want
    if (side === 'bottom') s.top = `${p.y + p.h + 4}px`
    else s.bottom = `${window.innerHeight - p.y + 4}px`
    s['max-height'] = `${Math.max(120, Math.min(window.innerHeight * 0.7, side === 'top' ? above : below))}px`
    if ((props.align ?? 'start') === 'start') {
      const left = Math.min(p.x, window.innerWidth - 240)
      s.left = `${left}px`
      s['max-width'] = `${window.innerWidth - left - 8}px`
    } else {
      const right = Math.max(8, window.innerWidth - p.x - p.w)
      s.right = `${right}px`
      s['max-width'] = `${window.innerWidth - right - 8}px`
    }
    return s
  }
  return (
    <>
      <button
        ref={btn}
        type="button"
        aria-label={props.label}
        title={props.label}
        aria-haspopup="menu"
        aria-expanded={open()}
        class={props.class}
        onClick={toggle}
      >
        {props.children}
      </button>
      <Show when={open()}>
        <Portal>
          <div class="fixed inset-0 z-40" onClick={close} />
          <div
            role="menu"
            style={style()}
            class="z-50 flex min-w-44 flex-col rounded-xl border border-line bg-raised p-1 text-[13px]"
            classList={{ 'shadow-[var(--shadow)]': true }}
          >
            <div class="flex min-h-0 gap-1">
              {columns().map((items, i) => (
                <div
                  class="min-w-28 flex-auto overflow-y-auto overscroll-contain sm:min-w-36"
                  classList={{ 'border-l border-line pl-1': i > 0 }}
                >
                  {items.map((it) =>
                    it === 'sep' ? (
                      <div class="my-1 h-px bg-line" />
                    ) : 'heading' in it ? (
                      <div class="px-2.5 pt-1.5 pb-1 text-[11px] font-medium text-faint">{it.heading}</div>
                    ) : (
                      <button
                        role="menuitem"
                        disabled={it.disabled}
                        class="flex w-full items-center gap-3 rounded-lg px-2.5 py-1.5 text-left hover:bg-ink/5 disabled:opacity-40"
                        classList={{ 'text-bad': it.danger }}
                        onClick={() => {
                          close()
                          it.onSelect()
                        }}
                      >
                        <span class={it.hint ? 'shrink-0' : 'min-w-0 flex-1 truncate'}>
                          {it.label}
                          <Show when={it.detail}>
                            <span class="mt-0.5 mb-px block max-w-64 text-[11.5px] leading-snug whitespace-normal text-faint">
                              {it.detail}
                            </span>
                          </Show>
                        </span>
                        <Show when={it.hint}>
                          <span class="min-w-0 flex-1 truncate text-right text-[11.5px] text-faint">{it.hint}</span>
                        </Show>
                        <Show when={it.checked}>
                          <span class="text-accent">✓</span>
                        </Show>
                      </button>
                    ),
                  )}
                </div>
              ))}
            </div>
          </div>
        </Portal>
      </Show>
    </>
  )
}

export function Field(props: { label: string; hint?: string; children: JSX.Element }) {
  return (
    <label class="block">
      <div class="mb-1 text-[12.5px] font-medium">{props.label}</div>
      {props.children}
      <Show when={props.hint}>
        <div class="mt-1 text-[12px] text-mute">{props.hint}</div>
      </Show>
    </label>
  )
}

export const inputClass = 'h-8 min-w-0 rounded-lg border border-line bg-raised px-2.5 text-[13px] placeholder:text-faint focus:border-faint'

const [viewing, setViewing] = createSignal<string | null>(null)

export function Picture(props: { src: string; class?: string }) {
  return (
    <img
      class={`cursor-zoom-in ${props.class ?? ''}`}
      src={props.src}
      role="button"
      tabindex="0"
      aria-label="View image"
      onClick={() => setViewing(props.src)}
      onKeyDown={(e) => e.key === 'Enter' && setViewing(props.src)}
    />
  )
}

export function Thumbs(props: { parts: InputPart[]; class?: string }) {
  const images = () => props.parts.filter((p) => p.type === 'image')
  return (
    <Show when={images().length}>
      <div class={`flex max-w-[85%] flex-wrap justify-end gap-2 ${props.class ?? ''}`}>
        <For each={images()}>
          {(p) => <Picture class="size-14 rounded-lg border border-line object-cover" src={`data:${p.media_type};base64,${p.data}`} />}
        </For>
      </div>
    </Show>
  )
}

export function PictureViewer() {
  const close = () => setViewing(null)
  createEffect(() => {
    if (!viewing()) return
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close()
    window.addEventListener('keydown', onKey)
    onCleanup(() => window.removeEventListener('keydown', onKey))
  })
  return (
    <Show when={viewing()}>
      <Portal>
        <div
          class="fixed inset-0 z-50 flex cursor-zoom-out items-center justify-center bg-black/80 p-4 sm:p-10"
          role="dialog"
          aria-label="Image"
          onClick={close}
        >
          <img class="max-h-full max-w-full rounded-lg object-contain shadow-2xl" src={viewing()!} />
          <button
            aria-label="Close"
            class="absolute top-4 right-4 flex size-9 items-center justify-center rounded-full bg-white/10 text-[18px] leading-none text-white hover:bg-white/20"
            onClick={close}
          >
            ×
          </button>
        </div>
      </Portal>
    </Show>
  )
}

const [ctx, setCtx] = createSignal<{ x: number; y: number; items: MenuItem[] } | null>(null)

export function openContextMenu(e: MouseEvent, items: MenuItem[]) {
  e.preventDefault()
  setCtx({ x: e.clientX, y: e.clientY, items })
}

export function ContextMenuHost() {
  const close = () => setCtx(null)
  createEffect(() => {
    if (!ctx()) return
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close()
    window.addEventListener('keydown', onKey)
    window.addEventListener('resize', close)
    window.addEventListener('scroll', close, true)
    onCleanup(() => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('resize', close)
      window.removeEventListener('scroll', close, true)
    })
  })
  return (
    <Show when={ctx()}>
      {(c) => (
        <Portal>
          <div class="fixed inset-0 z-50" onPointerDown={close} onContextMenu={(e) => (e.preventDefault(), close())}>
            <div
              role="menu"
              class="absolute min-w-48 rounded-xl border border-line bg-raised p-1 shadow-[var(--shadow)]"
              style={{
                left: `${Math.min(c().x, window.innerWidth - 216)}px`,
                top: `${Math.min(c().y, window.innerHeight - 16 - c().items.length * 34)}px`,
              }}
              onPointerDown={(e) => e.stopPropagation()}
            >
              <For each={c().items}>
                {(it) =>
                  it === 'sep' || it === 'col' ? (
                    <div class="my-1 h-px bg-line" />
                  ) : 'heading' in it ? (
                    <div class="px-2.5 py-1 text-[11.5px] text-faint">{it.heading}</div>
                  ) : (
                    <button
                      role="menuitem"
                      disabled={it.disabled}
                      class="flex w-full items-center rounded-lg px-2.5 py-1.5 text-left text-[13px] hover:bg-ink/5 disabled:opacity-40 max-md:py-2.5"
                      classList={{ 'text-bad': !!it.danger }}
                      onClick={() => {
                        close()
                        it.onSelect()
                      }}
                    >
                      {it.label}
                    </button>
                  )
                }
              </For>
            </div>
          </div>
        </Portal>
      )}
    </Show>
  )
}
