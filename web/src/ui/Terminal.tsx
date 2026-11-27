import { createEffect, createSignal, on, onCleanup, onMount, Show } from 'solid-js'
import { init, Terminal, FitAddon } from 'ghostty-web'
import { b64bytes, call, listenTerm, state } from '../lib/store'
import { providerOf } from '../lib/catalog'
import { WakeNotice } from './Panel'

let ready: Promise<void> | undefined

const key = (thread: string) => `corra_term:${thread}`

export function TerminalPanel(props: { thread: string }) {
  let host!: HTMLDivElement
  const [status, setStatus] = createSignal<'starting' | 'open' | 'closed' | 'error'>('starting')
  const [error, setError] = createSignal('')
  let term: Terminal | undefined
  let id: string | undefined
  let stop: (() => void) | undefined
  let observer: ResizeObserver | undefined

  const start = async (fresh: boolean) => {
    setStatus('starting')
    setError('')
    ready ??= init()
    await ready
    term?.dispose()
    host.replaceChildren()
    const css = getComputedStyle(document.documentElement)
    const v = (n: string) => css.getPropertyValue(n).trim()
    const probe = document.createElement('span')
    document.body.append(probe)
    const hex = (color: string) => {
      probe.style.color = color
      const c = getComputedStyle(probe).color
      const m = c.match(/[\d.]+/g) ?? ['0', '0', '0']
      const k = c.startsWith('color(') ? 255 : 1
      return (
        '#' +
        m
          .slice(0, 3)
          .map((x) =>
            Math.round(Number(x) * k)
              .toString(16)
              .padStart(2, '0'),
          )
          .join('')
      )
    }
    const theme = {
      background: hex(v('--bg')),
      foreground: hex(v('--ink')),
      cursor: hex(v('--accent')),
      selectionBackground: hex(v('--line')),
    }
    probe.remove()
    term = new Terminal({
      fontSize: 13,
      fontFamily: '"JetBrains Mono Variable", ui-monospace, Menlo, monospace',
      theme,
    } as never)
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(host)
    fit.fit()
    const cols = term.cols
    const rows = term.rows
    try {
      id = fresh ? undefined : (sessionStorage.getItem(key(props.thread)) ?? undefined)
      if (id) {
        try {
          const r = await call<{ data: string }>({ op: 'term_attach', thread: props.thread, term: id })
          term.write(b64bytes(r.data))
          void call({ op: 'term_resize', thread: props.thread, term: id, cols, rows })
        } catch {
          id = undefined
        }
      }
      if (!id) {
        const r = await call<{ term: string }>({ op: 'term_open', thread: props.thread, cols, rows })
        id = r.term
        sessionStorage.setItem(key(props.thread), id)
      }
    } catch (e) {
      setStatus('error')
      setError((e as Error).message)
      return
    }
    const t = id
    stop?.()
    stop = listenTerm(t, {
      data: (b) => term?.write(b),
      exit: () => {
        setStatus('closed')
        sessionStorage.removeItem(key(props.thread))
      },
    })
    term.onData((d: string) => void call({ op: 'term_input', thread: props.thread, term: t, data: d }))
    observer?.disconnect()
    observer = new ResizeObserver(() => {
      fit.fit()
      if (term) void call({ op: 'term_resize', thread: props.thread, term: t, cols: term.cols, rows: term.rows })
    })
    observer.observe(host)
    setStatus('open')
    term.focus()
  }

  const needsWake = () => {
    const t = state.threads[props.thread]
    const p = t ? state.projects[t.project] : undefined
    if (!p || !providerOf(p.provider)?.creates_machines) return false
    const m =
      (t?.machine && state.machines[t.machine]) ||
      Object.values(state.machines)
        .filter((x) => x.project === p.id && x.status !== 'destroyed')
        .sort((a, b) => b.created_at - a.created_at)[0]
    return m?.status !== 'connected'
  }

  onMount(() => {
    if (!needsWake()) void start(false)
  })
  createEffect(
    on(needsWake, (sleeping, prev) => {
      if (prev && !sleeping) void start(false)
    }),
  )
  onCleanup(() => {
    stop?.()
    observer?.disconnect()
    term?.dispose()
  })

  return (
    <div class="flex h-full flex-col">
      <WakeNotice thread={props.thread} />
      <Show when={!needsWake() && (status() === 'error' || status() === 'closed')}>
        <div class="flex items-center gap-3 border-b border-line px-3 py-2 text-[12px]">
          <span class={status() === 'error' ? 'text-bad' : 'text-mute'}>{status() === 'error' ? error() : 'The shell exited.'}</span>
          <button class="h-7 rounded-lg border border-line px-2.5 hover:border-faint" onClick={() => void start(true)}>
            New shell
          </button>
        </div>
      </Show>
      <div ref={host} class="min-h-0 flex-1 overflow-hidden p-2" classList={{ hidden: needsWake() }} />
    </div>
  )
}
