import { For, Show, createEffect, createMemo, createSignal, onCleanup } from 'solid-js'
import { Portal } from 'solid-js/web'
import Search from 'lucide-solid/icons/search'
import SquarePen from 'lucide-solid/icons/square-pen'
import FolderPlus from 'lucide-solid/icons/folder-plus'
import SettingsIcon from 'lucide-solid/icons/settings'
import BellRing from 'lucide-solid/icons/bell-ring'
import Folder from 'lucide-solid/icons/folder'
import SunMoon from 'lucide-solid/icons/sun-moon'
import PanelLeft from 'lucide-solid/icons/panel-left'
import { state } from '../lib/store'
import { paletteOpen, pushToast, setPaletteOpen } from '../lib/toast'
import { route } from '../lib/route'
import { unread } from '../lib/seen'
import { shownStatus } from '../lib/status'
import { ago } from '../lib/format'
import { toggleSidebar } from '../lib/layout'
import { choice, setChoice, type ThemeChoice } from '../lib/theme'
import { StatusMark, statusWord } from './bits'
import type { Thread } from '../proto/Thread'

export function attentionThreads(): Thread[] {
  const rank = (t: Thread) => {
    if (t.status === 'waiting') return 0
    if (t.status === 'error') return 1
    if (unread(t)) return 2
    return 9
  }
  return Object.values(state.threads)
    .filter((t) => !t.hidden_at && !t.settled_at && rank(t) < 9)
    .sort((a, b) => rank(a) - rank(b) || b.updated_at - a.updated_at)
}

export function jumpToNextAttention(): boolean {
  const list = attentionThreads()
  if (!list.length) {
    pushToast('No threads waiting on you.', 'info')
    return false
  }
  const cur = route().kind === 'thread' ? (route() as { id: string }).id : undefined
  const idx = cur ? list.findIndex((t) => t.id === cur) : -1
  const next = list[(idx + 1) % list.length]
  location.hash = `#/t/${next.id}`
  return true
}

type PaletteEntry = {
  id: string
  section: string
  label: string
  hint?: string
  status?: Thread['status'] | 'done'
  icon?: 'attention' | 'new' | 'project-new' | 'project' | 'settings' | 'sidebar' | 'theme'
  run: () => void
}

export function CommandPalette() {
  const [query, setQuery] = createSignal('')
  const [selected, setSelected] = createSignal(0)
  let inputRef: HTMLInputElement | undefined

  createEffect(() => {
    const onGlobalKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setPaletteOpen((v) => !v)
        return
      }
      const nextAttentionShortcut =
        (e.altKey && !e.metaKey && !e.ctrlKey && e.key.toLowerCase() === 'n') || ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === '.')
      if (nextAttentionShortcut) {
        const el = e.target as HTMLElement | null
        if (el && (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT' || el.isContentEditable)) return
        e.preventDefault()
        jumpToNextAttention()
      }
    }
    window.addEventListener('keydown', onGlobalKey)
    onCleanup(() => window.removeEventListener('keydown', onGlobalKey))
  })

  createEffect(() => {
    if (!paletteOpen()) return
    setQuery('')
    setSelected(0)
    queueMicrotask(() => inputRef?.focus())
  })

  const entries = createMemo<PaletteEntry[]>(() => {
    const q = query().trim().toLowerCase()
    const matches = (...parts: (string | undefined)[]) => !q || parts.some((p) => p?.toLowerCase().includes(q))
    const out: PaletteEntry[] = []

    const urgent = attentionThreads()
    const actions: PaletteEntry[] = [
      ...(urgent.length
        ? [
            {
              id: 'act:next',
              section: 'Actions',
              label: 'Next thread needing you',
              hint: `${urgent.length} waiting · Alt+N`,
              icon: 'attention' as const,
              run: () => jumpToNextAttention(),
            },
          ]
        : []),
      {
        id: 'act:new',
        section: 'Actions',
        label: 'New thread',
        hint: 'Start a conversation',
        icon: 'new',
        run: () => (location.hash = '#/new'),
      },
      {
        id: 'act:new-project',
        section: 'Actions',
        label: 'New project',
        icon: 'project-new',
        run: () => (location.hash = '#/new-project'),
      },
      {
        id: 'act:settings',
        section: 'Actions',
        label: 'Settings',
        icon: 'settings',
        run: () => (location.hash = '#/settings'),
      },
      {
        id: 'act:sidebar',
        section: 'Actions',
        label: 'Toggle sidebar',
        hint: 'Ctrl+B',
        icon: 'sidebar',
        run: () => toggleSidebar(),
      },
      {
        id: 'act:theme',
        section: 'Actions',
        label: 'Cycle theme',
        hint: choice(),
        icon: 'theme',
        run: () => {
          const order: ThemeChoice[] = ['auto', 'light', 'dark']
          setChoice(order[(order.indexOf(choice()) + 1) % order.length])
        },
      },
    ]
    for (const a of actions) if (matches(a.label, a.hint)) out.push(a)

    const cur = route().kind === 'thread' ? (route() as { id: string }).id : undefined
    const threads = Object.values(state.threads)
      .filter((t) => !t.hidden_at)
      .sort((a, b) => Number(b.status === 'waiting') - Number(a.status === 'waiting') || b.updated_at - a.updated_at)
    for (const t of threads) {
      const proj = state.projects[t.project]?.name ?? ''
      if (!matches(t.title, proj, t.agent)) continue
      const shown = shownStatus(t.status, unread(t), t.id === cur)
      const word = statusWord[shown]
      out.push({
        id: `t:${t.id}`,
        section: 'Threads',
        label: t.title,
        hint: [proj, word || ago(t.updated_at)].filter(Boolean).join(' · '),
        status: shown,
        run: () => (location.hash = `#/t/${t.id}`),
      })
      if (out.length >= 40) break
    }

    const projects = Object.values(state.projects).sort((a, b) => a.name.localeCompare(b.name))
    for (const p of projects) {
      if (!matches(p.name, ...p.repos.map((r) => r.name))) continue
      out.push({
        id: `p:${p.id}`,
        section: 'Projects',
        label: p.name,
        hint: p.repos.length ? p.repos.map((r) => r.name).join(', ') : 'Project settings',
        icon: 'project',
        run: () => (location.hash = `#/project/${p.id}`),
      })
    }

    return out
  })

  createEffect(() => {
    const len = entries().length
    if (selected() >= len) setSelected(Math.max(0, len - 1))
  })

  const runSelected = (item?: PaletteEntry) => {
    const target = item ?? entries()[selected()]
    if (!target) return
    setPaletteOpen(false)
    target.run()
  }

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      setPaletteOpen(false)
    } else if (e.key === 'ArrowDown') {
      e.preventDefault()
      setSelected((i) => (entries().length ? (i + 1) % entries().length : 0))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setSelected((i) => (entries().length ? (i - 1 + entries().length) % entries().length : 0))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      runSelected()
    }
  }

  const renderIcon = (e: PaletteEntry) => {
    if (e.status) return <StatusMark status={e.status} />
    switch (e.icon) {
      case 'attention':
        return <BellRing size={14} class="shrink-0 text-warn" />
      case 'new':
        return <SquarePen size={14} class="shrink-0 text-mute" />
      case 'project-new':
        return <FolderPlus size={14} class="shrink-0 text-mute" />
      case 'project':
        return <Folder size={14} class="shrink-0 text-mute" />
      case 'settings':
        return <SettingsIcon size={14} class="shrink-0 text-mute" />
      case 'sidebar':
        return <PanelLeft size={14} class="shrink-0 text-mute" />
      case 'theme':
        return <SunMoon size={14} class="shrink-0 text-mute" />
      default:
        return null
    }
  }

  return (
    <Show when={paletteOpen()}>
      <Portal>
        <div
          class="fixed inset-0 z-50 flex items-start justify-center bg-ink/25 px-4 pt-[12vh] backdrop-blur-[1px]"
          onClick={() => setPaletteOpen(false)}
        >
          <div
            role="dialog"
            aria-label="Quick switcher"
            class="flex max-h-[65vh] w-full max-w-lg flex-col overflow-hidden rounded-2xl border border-line bg-raised shadow-[var(--shadow)]"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={onKeyDown}
          >
            <label class="flex h-11 shrink-0 items-center gap-2.5 border-b border-line px-3.5 text-mute">
              <Search size={15} class="shrink-0" />
              <input
                ref={inputRef}
                class="min-w-0 flex-1 bg-transparent text-[14px] text-ink placeholder:text-faint"
                placeholder="Jump to a thread, project, or action…"
                value={query()}
                onInput={(e) => {
                  setQuery(e.currentTarget.value)
                  setSelected(0)
                }}
              />
              <kbd class="rounded border border-line px-1.5 py-0.5 font-mono text-[11px] text-faint">Esc</kbd>
            </label>
            <div class="min-h-0 flex-1 overflow-y-auto p-1.5">
              <For each={entries()} fallback={<div class="px-3 py-6 text-center text-[13px] text-faint">Nothing matches.</div>}>
                {(item, idx) => {
                  const showHeading = () => idx() === 0 || entries()[idx() - 1]?.section !== item.section
                  return (
                    <>
                      <Show when={showHeading()}>
                        <div class="px-2.5 pt-2 pb-1 text-[11px] font-medium text-faint">{item.section}</div>
                      </Show>
                      <button
                        type="button"
                        class="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px]"
                        classList={{
                          'bg-ink/[0.07] text-ink': selected() === idx(),
                          'text-ink/85 hover:bg-ink/[0.04]': selected() !== idx(),
                        }}
                        onMouseEnter={() => setSelected(idx())}
                        onClick={() => runSelected(item)}
                      >
                        {renderIcon(item)}
                        <span class="min-w-0 flex-1 truncate font-medium">{item.label}</span>
                        <Show when={item.hint}>
                          <span class="shrink-0 truncate text-[12px] text-faint">{item.hint}</span>
                        </Show>
                      </button>
                    </>
                  )
                }}
              </For>
            </div>
            <div class="flex items-center justify-between border-t border-line px-3.5 py-2 text-[11.5px] text-faint max-sm:hidden">
              <span>↑↓ navigate · Enter open</span>
              <span>Alt+N next needing you</span>
            </div>
          </div>
        </div>
      </Portal>
    </Show>
  )
}
