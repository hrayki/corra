import { Show, batch, createEffect, createSignal, on, onCleanup } from 'solid-js'
import Search from 'lucide-solid/icons/search'
import ChevronUp from 'lucide-solid/icons/chevron-up'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import X from 'lucide-solid/icons/x'
import { eventsOf, findIn, loadEarlier, loadingEarlier, state } from '../lib/store'
import { indexAt, itemAt, keep, label, occurrences, shows, spotAt, step, total } from '../lib/find'
import type { FindHit } from '../proto/FindHit'
import { IconBtn } from './bits'

export const [findTarget, setFindTarget] = createSignal<{ thread: string; item: string } | null>(null, { equals: false })

const ALL = 'corra-find'
const NOW = 'corra-find-now'
const painting = typeof Highlight !== 'undefined' && !!CSS.highlights
const frame = () => new Promise<void>((r) => requestAnimationFrame(() => r()))

function rangesIn(el: Element, query: string): Range[] {
  const nodes: Text[] = []
  const starts: number[] = []
  let text = ''
  const walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.parentElement?.closest('button') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  })
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    nodes.push(n as Text)
    starts.push(text.length)
    text += (n as Text).data
  }
  const node = (at: number, end: boolean) => {
    let lo = 0
    let hi = nodes.length - 1
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1
      if (end ? starts[mid] < at : starts[mid] <= at) lo = mid
      else hi = mid - 1
    }
    return [nodes[lo], at - starts[lo]] as const
  }
  return occurrences(text, query).map(([a, b]) => {
    const r = document.createRange()
    r.setStart(...node(a, false))
    r.setEnd(...node(b, true))
    return r
  })
}

const messageEl = (root: Element, item: string) => root.querySelector(`[data-find="${CSS.escape(item)}"]`)

export type Finder = ReturnType<typeof createFind>

export function createFind(opts: { thread: string; scroller: () => HTMLElement | undefined; unstick: () => void }) {
  const thread = opts.thread
  const [open, setOpen] = createSignal(false)
  const [query, setQuery] = createSignal('')
  const [result, setResult] = createSignal<{ query: string; hits: FindHit[]; capped: boolean } | null>(null)
  const [index, setIndex] = createSignal(-1)
  let input: HTMLInputElement | undefined
  let searches = 0
  let moves = 0

  const spot = () => {
    const r = result()
    const s = r && spotAt(r.hits, index())
    if (!r || !s) return null
    const at = itemAt(eventsOf(thread), r.hits[s.hit].seq)
    return at ? { ...at, at: s.at } : null
  }

  const currentRange = () => {
    const sc = opts.scroller()
    const r = result()
    const s = spot()
    if (!sc || !r || !s) return null
    const el = messageEl(sc, s.item)
    const ranges = el ? rangesIn(el, r.query) : []
    return ranges[Math.min(s.at, ranges.length - 1)] ?? null
  }

  const clear = () => {
    if (!painting) return
    CSS.highlights.delete(ALL)
    CSS.highlights.delete(NOW)
  }

  const paint = () => {
    const sc = opts.scroller()
    const r = result()
    if (!painting || !open() || !sc || !r || !r.hits.length) return clear()
    const all = new Highlight()
    for (const el of sc.querySelectorAll('[data-find]')) for (const range of rangesIn(el, r.query)) all.add(range)
    CSS.highlights.set(ALL, all)
    const now = currentRange()
    if (now) {
      const h = new Highlight(now)
      h.priority = 1
      CSS.highlights.set(NOW, h)
    } else CSS.highlights.delete(NOW)
  }

  let repaint: number | undefined
  const paintSoon = () => {
    if (repaint === undefined) repaint = window.setTimeout(() => ((repaint = undefined), paint()), 120)
  }

  const anchorOf = (seq: number) => {
    const sc = opts.scroller()
    const at = itemAt(eventsOf(thread), seq)
    if (!sc || !at) return null
    return messageEl(sc, at.item) ?? sc.querySelector(`[data-turn="${CSS.escape(at.turn)}"]`)
  }

  const nearest = (hits: FindHit[]) => {
    const sc = opts.scroller()
    if (!hits.length) return -1
    const top = sc?.getBoundingClientRect().top ?? 0
    const i = hits.findIndex((h) => {
      const el = anchorOf(h.seq)
      return !!el && el.getBoundingClientRect().bottom > top
    })
    return i < 0 ? total(hits) - 1 : indexAt(hits, { hit: i, at: 0 })
  }

  const settled = async (g: number) => {
    for (let busy = loadingEarlier(thread); busy; busy = loadingEarlier(thread)) {
      await busy.catch(() => {})
      await frame()
      await frame()
      if (g !== moves) return false
    }
    return true
  }

  const bring = async (g: number) => {
    const sc = opts.scroller()
    let steady = 0
    for (let i = 0; i < 8 && sc && steady < 2; i++) {
      if (!(await settled(g))) return
      const range = currentRange()
      if (!range) return
      const box = sc.getBoundingClientRect()
      const r = range.getBoundingClientRect()
      if (r.top >= box.top + 8 && r.bottom <= box.bottom - 8) steady++
      else {
        steady = 0
        opts.unstick()
        sc.scrollTop += r.top - (box.top + box.height / 3)
      }
      await frame()
      if (g !== moves) return
    }
  }

  const go = async () => {
    const g = ++moves
    const r = result()
    const s = r && spotAt(r.hits, index())
    if (!r || !s) return paint()
    const seq = r.hits[s.hit].seq
    let at = itemAt(eventsOf(thread), seq)
    if ((!at || !shows(state.views[thread], at.item)) && state.earlier[thread] != null) {
      opts.unstick()
      await loadEarlier(thread, seq).catch(() => {})
      if (g !== moves) return
      at = itemAt(eventsOf(thread), seq)
    }
    if (!at) return paint()
    const sc = opts.scroller()
    if (sc && !messageEl(sc, at.item)) opts.unstick()
    setFindTarget({ thread, item: at.item })
    await frame()
    if (g !== moves) return
    const range = currentRange()
    const clamp = range?.startContainer.parentElement?.closest('[data-clamp]')
    if (range && clamp && range.getBoundingClientRect().bottom > clamp.getBoundingClientRect().bottom) {
      opts.unstick()
      clamp.dispatchEvent(new Event('unclamp'))
      await frame()
      if (g !== moves) return
    }
    paint()
    await bring(g)
  }

  const search = async (q: string, jump: boolean) => {
    const g = ++searches
    if (!q) {
      moves++
      batch(() => {
        setResult(null)
        setIndex(-1)
      })
      return clear()
    }
    let found
    try {
      found = await findIn(thread, q)
    } catch {
      return
    }
    if (g !== searches || !open()) return
    const before = result()
    batch(() => {
      setIndex(!jump && before?.query === q ? keep(before.hits, index(), found.hits) : nearest(found.hits))
      setResult({ query: q, hits: found.hits, capped: found.capped })
    })
    if (jump) void go()
    else paint()
  }

  let typing: number | undefined
  const type = (q: string) => {
    setQuery(q)
    clearTimeout(typing)
    typing = window.setTimeout(() => void search(q, true), 150)
  }

  const move = (by: 1 | -1) => {
    clearTimeout(typing)
    const r = result()
    if (!r || r.query !== query()) return void search(query(), true)
    setIndex(step(index(), total(r.hits), by))
    void go()
  }

  const show = () => {
    const was = open()
    setOpen(true)
    queueMicrotask(() => {
      input?.focus()
      input?.select()
    })
    if (!was && query()) void search(query(), true)
  }

  const close = () => {
    searches++
    moves++
    clearTimeout(typing)
    setOpen(false)
    setResult(null)
    setIndex(-1)
    clear()
  }

  const onKey = (e: KeyboardEvent) => {
    if (e.defaultPrevented || e.altKey || e.shiftKey || !(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== 'f') return
    if (open() && document.activeElement === input) return
    e.preventDefault()
    show()
  }
  window.addEventListener('keydown', onKey)

  let seen = 0
  let refresh: number | undefined
  createEffect(
    on(
      () => state.views[thread]?.lastSeq ?? 0,
      (last) => {
        const from = seen
        seen = last
        if (!open() || !query() || refresh !== undefined) return
        const arrived =
          last < from ||
          eventsOf(thread).some(
            (e) =>
              e.seq > from &&
              (e.kind === 'turn_started' ||
                (e.kind === 'item_completed' && !e.item.parent && (e.item.type === 'agent_message' || e.item.type === 'user_message'))),
          )
        if (arrived)
          refresh = window.setTimeout(() => {
            refresh = undefined
            void search(query(), false)
          }, 500)
      },
    ),
  )

  createEffect(() => {
    const sc = opts.scroller()
    if (!open() || !sc) return
    const watch = new MutationObserver(paintSoon)
    watch.observe(sc, { childList: true, subtree: true, characterData: true })
    onCleanup(() => watch.disconnect())
  })

  onCleanup(() => {
    window.removeEventListener('keydown', onKey)
    clearTimeout(typing)
    clearTimeout(refresh)
    clearTimeout(repaint)
    clear()
  })

  return {
    open,
    query,
    show,
    close,
    toggle: () => (open() ? close() : show()),
    type,
    move,
    label: () => {
      const r = result()
      return r && r.query === query() ? label(index(), r.hits, r.capped) : ''
    },
    some: () => !!result()?.hits.length,
    setInput: (el: HTMLInputElement) => (input = el),
  }
}

export function FindBar(props: { find: Finder }) {
  const f = props.find
  return (
    <div role="search" class="flex h-11 shrink-0 items-center gap-1 border-b border-line pr-2 pl-4 sm:pr-3 sm:pl-6 max-md:h-12">
      <Search size={15} class="shrink-0 text-faint" aria-hidden="true" />
      <input
        ref={f.setInput}
        class="h-full min-w-0 flex-1 bg-transparent px-2 text-[13.5px] placeholder:text-faint max-md:text-[16px]"
        placeholder="Find in thread"
        aria-label="Find in thread"
        autocomplete="off"
        spellcheck={false}
        enterkeyhint="search"
        value={f.query()}
        onInput={(e) => f.type(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.isComposing) return
          if (e.key === 'Enter') {
            e.preventDefault()
            f.move(e.shiftKey ? -1 : 1)
          } else if (e.key === 'Escape') {
            e.preventDefault()
            f.close()
          }
        }}
      />
      <Show when={f.label()}>
        <span class="num shrink-0 px-1 text-[12px] whitespace-nowrap text-mute" aria-live="polite">
          {f.label()}
        </span>
      </Show>
      <IconBtn label="Previous match (Shift+Enter)" class="max-md:size-10" disabled={!f.some()} onClick={() => f.move(-1)}>
        <ChevronUp size={16} />
      </IconBtn>
      <IconBtn label="Next match (Enter)" class="max-md:size-10" disabled={!f.some()} onClick={() => f.move(1)}>
        <ChevronDown size={16} />
      </IconBtn>
      <IconBtn label="Close (Esc)" class="max-md:size-10" onClick={f.close}>
        <X size={16} />
      </IconBtn>
    </div>
  )
}
