import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount, type JSX } from 'solid-js'
import ArrowUp from 'lucide-solid/icons/arrow-up'
import Square from 'lucide-solid/icons/square'
import X from 'lucide-solid/icons/x'
import Paperclip from 'lucide-solid/icons/paperclip'
import FoldVertical from 'lucide-solid/icons/chevrons-down-up'
import UnfoldVertical from 'lucide-solid/icons/chevrons-up-down'
import { addPending, call, dropPending, state, type ThreadX } from '../lib/store'
import type { InputPart } from '../proto/InputPart'
import type { AgentSettings } from '../proto/AgentSettings'
import type { AgentKind } from '../proto/AgentKind'
import type { Choice } from '../proto/Choice'
import { tokens } from '../lib/format'
import { AGENT_NAMES, fitEffort, pickerState } from '../lib/choices'
import { Menu, Picture } from './bits'
import { draftFor, setDraftFor } from './Timeline'
import { continueList, loadDraft, saveDraftImages, saveDraftText } from '../lib/drafts'
import { usageWindows } from '../lib/seen'
import { paired } from '../lib/catalog'

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

const pill =
  'inline-flex h-7 items-center gap-1 whitespace-nowrap rounded-lg border border-line px-2 text-[12.5px] text-mute hover:border-faint hover:text-ink max-md:h-9'

const remembered = (agent: AgentKind, what: 'models' | 'modes'): Choice[] | undefined => {
  try {
    return JSON.parse(localStorage.getItem(choiceKey(agent, what)) ?? 'null') ?? undefined
  } catch {
    return undefined
  }
}

const choiceKey = (agent: AgentKind, what: 'models' | 'modes' | 'default') =>
  agent === 'antigravity' ? `corra_agy_${what}` : `corra_${agent}_${what}`

export function rememberChoices(agent: AgentKind, v?: { model?: string; models?: Choice[]; modes?: Choice[] }) {
  if (v?.models?.length) localStorage.setItem(choiceKey(agent, 'models'), JSON.stringify(v.models))
  if (agent !== 'antigravity') return
  if (v?.modes?.length) localStorage.setItem(choiceKey(agent, 'modes'), JSON.stringify(v.modes))
  if (v?.model) localStorage.setItem(choiceKey(agent, 'default'), v.model)
}

export function SettingsPickers(props: {
  settings: AgentSettings
  onChange: (s: AgentSettings) => void
  agent?: AgentKind
  model?: string
  models?: Choice[]
  efforts?: Choice[]
  modes?: Choice[]
  mode?: string
}) {
  const agent = () => props.agent ?? 'claude'
  const models = () => (props.models?.length ? props.models : (remembered(agent(), 'models') ?? []))
  const st = createMemo(() =>
    pickerState({
      agent: agent(),
      settings: props.settings,
      models: models(),
      modes: props.modes?.length ? props.modes : remembered(agent(), 'modes'),
      efforts: props.efforts,
      model: agent() === 'antigravity' ? (props.model ?? localStorage.getItem(choiceKey(agent(), 'default')) ?? undefined) : props.model,
      mode: props.mode,
    }),
  )
  const pickModel = (id: string) => {
    const list = models().find((c) => c.id === id)?.efforts ?? []
    const e = fitEffort(st().effort, list)
    props.onChange(e && e !== st().effort ? { model: id, effort: e } : { model: id })
  }
  return {
    mode: (
      <Menu
        label="Permissions"
        side="top"
        class={pill}
        items={st().modes.map((m) => ({
          label: m.name,
          detail: m.hint,
          checked: m.id === st().mode.id,
          onSelect: () => props.onChange({ permission_mode: m.id }),
        }))}
      >
        <span classList={{ 'text-warn': st().mode.id === 'bypassPermissions' }}>{st().mode.name}</span>
      </Menu>
    ),
    model: (
      <Menu
        label="Model and effort"
        side="top"
        align="end"
        class={pill}
        items={[
          { heading: 'Model' },
          ...(!models().length
            ? [
                {
                  label: st().modelName,
                  detail: `The list shows once ${AGENT_NAMES[agent()]} has started here.`,
                  checked: true,
                  onSelect: () => {},
                },
              ]
            : models().map((m) => ({
                label: m.name,
                detail: m.hint,
                checked: st().model?.id === m.id,
                onSelect: () => pickModel(m.id),
              }))),
          ...(st().efforts.length
            ? [
                'col' as const,
                { heading: 'Effort' },
                ...st().efforts.map((e) => ({
                  label: e.name,
                  checked: st().effort === e.id,
                  onSelect: () => props.onChange({ effort: e.id }),
                })),
              ]
            : []),
        ]}
      >
        <span>{st().modelName}</span>
        <Show when={st().effortName}>
          <span class="text-faint @max-xl:hidden">{st().effortName}</span>
        </Show>
      </Menu>
    ),
  }
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
    ta.style.height = Math.min(ta.scrollHeight, 320) + 'px'
    ta.scrollTop = document.activeElement === ta && ta.selectionEnd === ta.value.length ? ta.scrollHeight : top
  }
  createEffect(on(props.input.text, () => queueMicrotask(resize)))
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
        class="rounded-2xl border border-line bg-raised shadow-[0_1px_2px_rgb(0_0_0/0.04)] transition-colors duration-100 focus-within:border-faint"
        onDrop={props.input.onDrop}
        onDragOver={(e) => e.preventDefault()}
      >
        <Show when={props.input.images().length}>
          <div class="flex flex-wrap gap-2 px-3 pt-3">
            <For each={props.input.images()}>
              {(im, i) => (
                <div class="relative">
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
          class="block max-h-80 w-full resize-none bg-transparent px-4 pt-3.5 pb-1 text-[14px] leading-relaxed placeholder:text-faint"
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
        <div class="@container flex items-center gap-1.5 px-2 pt-0.5 pb-2">
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

const KEPT = 'corra:keep-history'
const [kept, setKeptRaw] = createSignal<Set<string>>(
  new Set(
    (() => {
      try {
        return JSON.parse(localStorage.getItem(KEPT) ?? '[]') as string[]
      } catch {
        return []
      }
    })(),
  ),
)

function setKept(key: string, on: boolean) {
  const next = new Set(kept())
  if (on) next.add(key)
  else next.delete(key)
  const list = [...next].slice(-50)
  setKeptRaw(new Set(list))
  try {
    localStorage.setItem(KEPT, JSON.stringify(list))
  } catch {}
}

function CompactChip(props: { tokens: number; keep: boolean; onToggle: () => void }) {
  const n = () => tokens(props.tokens)
  return (
    <button
      type="button"
      aria-pressed={!props.keep}
      aria-label={props.keep ? `Keep the full history, ${n()} tokens` : `Compact ${n()} tokens first`}
      title={
        props.keep
          ? `Sends with the full history, which reads ${n()} tokens again at full price. Click to summarize first.`
          : `The prompt cache expired, so this message would read ${n()} tokens again at full price. Sending summarizes the thread first. Click to keep the full history.`
      }
      onClick={props.onToggle}
      class="mr-0.5 flex h-7 shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-[12px] font-medium tabular-nums transition-colors duration-100 max-md:h-9 @max-lg:gap-1 @max-lg:px-2"
      classList={{
        'border-accent/40 bg-accent/10 text-accent hover:bg-accent/15': !props.keep,
        'border-line text-mute hover:border-faint hover:text-ink': props.keep,
      }}
    >
      <Show when={props.keep} fallback={<FoldVertical size={13} />}>
        <UnfoldVertical size={13} />
      </Show>
      <span class="@max-lg:hidden">{props.keep ? 'Keep' : 'Compact'}</span>
      <span classList={{ 'text-accent/70': !props.keep }}>{n()}</span>
    </button>
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

export function UsageRing() {
  const windows = createMemo(() => (state.usage ? usageWindows(state.usage.limits).filter((w) => w.used !== undefined || w.blocked) : []))
  const top = () => windows().reduce((m, w) => Math.max(m, w.blocked ? 100 : (w.used ?? 0)), 0)
  const blocked = () => windows().some((w) => w.blocked)
  const when = (ms: number) =>
    ms - Date.now() > 20 * 3600_000
      ? new Date(ms).toLocaleDateString([], { weekday: 'short' })
      : new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  const R = 7
  const C = 2 * Math.PI * R
  return (
    <Show when={windows().length}>
      <Menu
        label={`Usage: ${top()}% of your limit`}
        align="end"
        class="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-[12px] text-mute hover:bg-ink/5 hover:text-ink"
        items={[
          { heading: 'Usage' },
          ...windows().map((w) => ({
            label: w.blocked ? `${w.name} reached` : `${w.name} ${w.used ?? 0}%`,
            hint: w.resets && w.resets > Date.now() ? `resets ${when(w.resets)}` : '',
            onSelect: () => {},
          })),
        ]}
      >
        <svg viewBox="0 0 20 20" class="size-[18px] -rotate-90" aria-hidden="true">
          <circle cx="10" cy="10" r={R} fill="none" stroke="var(--line)" stroke-width="2.5" />
          <circle
            cx="10"
            cy="10"
            r={R}
            fill="none"
            stroke={blocked() || top() >= 90 ? 'var(--bad)' : top() >= 70 ? 'var(--warn)' : 'var(--mute)'}
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-dasharray={`${(C * Math.min(100, top())) / 100} ${C}`}
          />
        </svg>
        <span class="num max-sm:hidden" classList={{ 'text-bad': blocked() }}>
          {blocked() ? 'Limited' : `${top()}%`}
        </span>
      </Menu>
    </Show>
  )
}

export function Composer(props: { thread: ThreadX }) {
  const input = useInput(() => props.thread.id)
  const [error, setError] = createSignal('')
  const busy = () => props.thread.status === 'running' || props.thread.status === 'waiting' || props.thread.status === 'starting'
  createEffect(() => {
    const d = draftFor()
    if (d && d.thread === props.thread.id) {
      input.setText(d.text)
      setDraftFor(null)
    }
  })
  const [clock, setClock] = createSignal(Date.now())
  const ticker = setInterval(() => setClock(Date.now()), 30_000)
  onCleanup(() => clearInterval(ticker))
  const keepKey = () => `${props.thread.id}:${state.live[props.thread.id]?.cache?.until ?? 0}`
  const keep = () => kept().has(keepKey())
  const setKeep = (on: boolean) => setKept(keepKey(), on)
  const stale = () => {
    const live = state.live[props.thread.id]
    const c = live?.cache
    const limited = (live?.continue_at ?? 0) > clock()
    return c && c.compact && clock() > c.until && !busy() && !limited ? c.tokens : null
  }
  const send = async (now = false) => {
    const parts = input.parts()
    if (!parts.length) return
    input.clear()
    setError('')
    const compact = stale() !== null && !keep()
    const queued = (busy() && !now) || compact
    setKeep(false)
    const shown = addPending(props.thread.id, parts, queued)
    try {
      await call({
        op: 'send',
        thread: props.thread.id,
        input: parts,
        queue: queued && !compact,
        client_id: crypto.randomUUID(),
        ...(compact ? { compact: true } : {}),
      })
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
              <Show when={stale()}>{(n) => <CompactChip tokens={n()} keep={keep()} onToggle={() => setKeep(!keep())} />}</Show>
              <Show
                when={busy() && !input.parts().length}
                fallback={<SendButton disabled={!input.parts().length} onClick={() => void send()} label={busy() ? 'Queue' : 'Send'} />}
              >
                <button
                  aria-label="Stop"
                  title="Stop"
                  class="ml-1 flex size-8 shrink-0 items-center justify-center rounded-full border border-line text-ink hover:border-bad hover:text-bad"
                  onClick={() => void call({ op: 'interrupt', thread: props.thread.id }).catch((e) => setError((e as Error).message))}
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
