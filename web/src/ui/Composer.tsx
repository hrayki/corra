import { For, Show, createEffect, createMemo, createSignal, on, onMount, untrack, type JSX } from 'solid-js'
import ArrowUp from 'lucide-solid/icons/arrow-up'
import Square from 'lucide-solid/icons/square'
import X from 'lucide-solid/icons/x'
import Paperclip from 'lucide-solid/icons/paperclip'
import { addPending, call, dropPending, settlePending, state, stopThread, takeRestored } from '../lib/store'
import type { InputPart } from '../proto/InputPart'
import type { Thread } from '../proto/Thread'
import { Picture } from './bits'
import { SettingsPickers, rememberChoices } from './Pickers'
import { continueList, loadDraft, saveDraftImages, saveDraftText } from '../lib/drafts'
import { paired } from '../lib/catalog'

export { SettingsPickers }

type Image = { media_type: string; data: string }

export function useInput(key?: () => string) {
  const [text, setTextRaw] = createSignal('')
  const [images, setImagesRaw] = createSignal<Image[]>([])
  if (key)
    createEffect(
      on(key, (k) => {
        const d = loadDraft(k)
        setTextRaw(d.text)
        setImagesRaw(d.images)
      }),
    )
  const setText = (t: string) => {
    setTextRaw(t)
    if (key) saveDraftText(key(), t)
  }
  const setImages = (next: Image[] | ((xs: Image[]) => Image[])) => {
    setImagesRaw(next)
    if (key) saveDraftImages(key(), images())
  }
  const addFile = (f: File) => {
    const r = new FileReader()
    r.onload = () => {
      const s = String(r.result)
      setImages((xs) => [...xs, { media_type: f.type, data: s.slice(s.indexOf(',') + 1) }])
    }
    r.readAsDataURL(f)
  }
  const onPaste = (e: ClipboardEvent) => {
    for (const item of e.clipboardData?.items ?? []) {
      if (!item.type.startsWith('image/')) continue
      const f = item.getAsFile()
      if (!f) continue
      e.preventDefault()
      addFile(f)
    }
  }
  const onDrop = (e: DragEvent) => {
    const files = [...(e.dataTransfer?.files ?? [])].filter((f) => f.type.startsWith('image/'))
    if (!files.length) return
    e.preventDefault()
    files.forEach(addFile)
  }
  const parts = (): InputPart[] => [
    ...images().map((i) => ({ type: 'image' as const, ...i })),
    ...(text().trim() ? [{ type: 'text' as const, text: text().trimEnd() }] : []),
  ]
  const clear = () => {
    setText('')
    setImages([])
  }
  return { text, setText, images, setImages, onPaste, onDrop, parts, clear, addFile }
}

export function InputBox(props: {
  input: ReturnType<typeof useInput>
  placeholder: string
  onSubmit: (now: boolean) => void
  left: JSX.Element
  right: JSX.Element
  autofocus?: boolean
  commands?: string[]
}) {
  let ta!: HTMLTextAreaElement
  const resize = () => {
    if (!ta) return
    const top = ta.scrollTop
    ta.style.height = 'auto'
    ta.style.height = ta.scrollHeight + 'px'
    ta.scrollTop = document.activeElement === ta && ta.selectionEnd === ta.value.length ? ta.scrollHeight : top
  }
  createEffect(on([props.input.text, props.input.images], () => queueMicrotask(resize)))
  onMount(() => props.autofocus && requestAnimationFrame(() => ta?.focus()))
  const matches = createMemo(() => {
    const t = props.input.text()
    if (!props.commands?.length || !t.startsWith('/') || /\s/.test(t)) return []
    const q = t.slice(1).toLowerCase()
    return props.commands.filter((c) => c.toLowerCase().startsWith(q)).slice(0, 8)
  })
  return (
    <div class="relative">
      <Show when={matches().length > 0}>
        <div class="absolute right-0 bottom-full left-0 mb-2 overflow-hidden rounded-xl border border-line bg-raised p-1 shadow-[var(--shadow)]">
          <For each={matches()}>
            {(c) => (
              <button
                class="block w-full rounded-lg px-2.5 py-1.5 text-left font-mono text-[12.5px] hover:bg-ink/5"
                onClick={() => {
                  props.input.setText(`/${c} `)
                  ta.focus()
                }}
              >
                /{c}
              </button>
            )}
          </For>
        </div>
      </Show>
      <div
        class="flex max-h-[40dvh] flex-col rounded-2xl border border-line bg-raised shadow-[0_1px_2px_rgb(0_0_0/0.04)] transition-colors duration-100 focus-within:border-faint"
        onDrop={props.input.onDrop}
        onDragOver={(e) => e.preventDefault()}
      >
        <Show when={props.input.images().length}>
          <div class="flex shrink-0 gap-2 overflow-x-auto px-3 pt-3">
            <For each={props.input.images()}>
              {(im, i) => (
                <div class="relative shrink-0">
                  <Picture class="size-14 rounded-lg border border-line object-cover" src={`data:${im.media_type};base64,${im.data}`} />
                  <button
                    aria-label="Remove image"
                    class="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full bg-ink text-bg"
                    onClick={() => props.input.setImages((xs) => xs.filter((_, j) => j !== i()))}
                  >
                    <X size={11} />
                  </button>
                </div>
              )}
            </For>
          </div>
        </Show>
        <textarea
          ref={ta}
          data-composer
          autofocus={props.autofocus}
          rows={1}
          class="block min-h-0 w-full resize-none bg-transparent px-4 pt-3.5 pb-1 text-[14px] leading-relaxed placeholder:text-faint"
          placeholder={props.placeholder}
          value={props.input.text()}
          onInput={(e) => props.input.setText(e.currentTarget.value)}
          onPaste={props.input.onPaste}
          onKeyDown={(e) => {
            if (e.key !== 'Enter' || e.isComposing) return
            if (e.ctrlKey || e.metaKey) {
              e.preventDefault()
              props.onSubmit(true)
              return
            }
            if (!e.shiftKey) {
              e.preventDefault()
              props.onSubmit(false)
              return
            }
            const list = continueList(ta.value, ta.selectionStart, ta.selectionEnd)
            if (!list) return
            e.preventDefault()
            const old = ta.value
            let from = 0
            while (from < old.length && from < list.value.length && old[from] === list.value[from]) from++
            let tail = 0
            while (
              tail < old.length - from &&
              tail < list.value.length - from &&
              old[old.length - 1 - tail] === list.value[list.value.length - 1 - tail]
            )
              tail++
            ta.setSelectionRange(from, old.length - tail)
            if (!document.execCommand('insertText', false, list.value.slice(from, list.value.length - tail))) {
              props.input.setText(list.value)
              ta.value = list.value
            }
            ta.selectionStart = ta.selectionEnd = list.caret
          }}
        />
        <div class="@container flex shrink-0 items-center gap-1.5 px-2 pt-0.5 pb-2">
          <label
            class="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-lg text-mute hover:bg-ink/5 hover:text-ink max-md:size-9"
            title="Attach an image"
          >
            <Paperclip size={15} />
            <input
              type="file"
              accept="image/*"
              multiple
              class="sr-only"
              aria-label="Attach an image"
              onChange={(e) => {
                for (const f of e.currentTarget.files ?? []) props.input.addFile(f)
                e.currentTarget.value = ''
              }}
            />
          </label>
          {props.left}
          <div class="flex-1" />
          {props.right}
        </div>
      </div>
    </div>
  )
}

export function SendButton(props: { disabled?: boolean; onClick: () => void; label?: string }) {
  return (
    <button
      aria-label={props.label ?? 'Send'}
      title={props.label ?? 'Send'}
      disabled={props.disabled}
      onClick={props.onClick}
      class="ml-1 flex size-8 shrink-0 items-center justify-center rounded-full bg-ink text-bg transition-opacity duration-100 hover:opacity-90 disabled:opacity-25"
    >
      <ArrowUp size={16} stroke-width={2.4} />
    </button>
  )
}

const TOUCH = typeof matchMedia !== 'undefined' && matchMedia('(pointer: coarse)').matches
const MOD = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl'

export function Composer(props: { thread: Thread }) {
  const input = useInput(() => props.thread.id)
  const [error, setError] = createSignal('')
  const busy = () => props.thread.status === 'running' || props.thread.status === 'waiting' || props.thread.status === 'starting'
  createEffect(() => {
    if (!state.restore[props.thread.id]?.texts.length) return
    const back = takeRestored(props.thread.id)
    if (!back) return
    if (back.replace) {
      input.setText(back.texts.join('\n\n'))
      return
    }
    const now = untrack(input.text).trimEnd()
    input.setText([...(now ? [now] : []), ...back.texts].join('\n\n'))
  })
  const send = async (now = false) => {
    const parts = input.parts()
    if (!parts.length) return
    input.clear()
    setError('')
    const queued = busy() && !now
    const client = crypto.randomUUID()
    const shown = addPending(props.thread.id, parts, queued, client)
    try {
      const r = await call<{ queued?: boolean } | null>({
        op: 'send',
        thread: props.thread.id,
        input: parts,
        queue: queued,
        client_id: client,
      })
      settlePending(props.thread.id, shown, r?.queued ?? queued)
    } catch (e) {
      dropPending(props.thread.id, shown)
      setError((e as Error).message)
      const t = parts.find((p) => p.type === 'text')
      if (t && t.type === 'text') input.setText(t.text)
    }
  }
  createEffect(() => {
    const v = state.views[props.thread.id]
    rememberChoices(props.thread.agent, v)
  })
  const pickers = SettingsPickers({
    get settings() {
      return props.thread.settings
    },
    get agent() {
      return props.thread.agent
    },
    get model() {
      return state.views[props.thread.id]?.model
    },
    get models() {
      return state.views[props.thread.id]?.models
    },
    get efforts() {
      return state.views[props.thread.id]?.efforts
    },
    get modes() {
      return state.views[props.thread.id]?.modes
    },
    get mode() {
      return state.views[props.thread.id]?.permissionMode
    },
    onChange: (s) => void call({ op: 'update_thread', thread: props.thread.id, settings: s }).catch((e) => setError((e as Error).message)),
  })
  const offline = () => {
    const m = props.thread.machine ? state.machines[props.thread.machine] : undefined
    return m && paired(m.provider) && m.status !== 'connected' ? m.name : null
  }
  return (
    <div class="px-4 pt-2 pb-4 sm:px-6">
      <div class="mx-auto max-w-3xl">
        <Show when={error()}>
          <div class="mb-2 text-[12.5px] text-bad">{error()}</div>
        </Show>
        <Show when={offline()}>
          <div class="mb-2 text-[12.5px] text-mute">{offline()} is offline. The thread carries on when it's back.</div>
        </Show>
        <InputBox
          input={input}
          placeholder={
            TOUCH
              ? busy()
                ? 'Queue a message'
                : 'Ask anything'
              : busy()
                ? `Enter queues it for when the agent is done, ${MOD}+Enter sends it now`
                : 'Ask anything. Shift+Enter for a new line, / for commands'
          }
          onSubmit={(now) => void send(now)}
          commands={state.views[props.thread.id]?.slashCommands}
          left={pickers.mode}
          right={
            <>
              {pickers.model}
              <Show
                when={busy() && !input.parts().length}
                fallback={<SendButton disabled={!input.parts().length} onClick={() => void send()} label={busy() ? 'Queue' : 'Send'} />}
              >
                <button
                  aria-label="Stop"
                  title="Stop"
                  class="ml-1 flex size-8 shrink-0 items-center justify-center rounded-full border border-line text-ink hover:border-bad hover:text-bad"
                  onClick={() => void stopThread(props.thread.id).catch((e) => setError((e as Error).message))}
                >
                  <Square size={12} fill="currentColor" />
                </button>
              </Show>
            </>
          }
        />
      </div>
    </div>
  )
}
