import { For, Show, createEffect, createMemo, createResource, createSignal } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import Folder from 'lucide-solid/icons/folder'
import FolderGit2 from 'lucide-solid/icons/folder-git-2'
import CornerLeftUp from 'lucide-solid/icons/corner-left-up'
import Laptop from 'lucide-solid/icons/laptop'
import Cloud from 'lucide-solid/icons/cloud'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import { call, state, type ProjectX } from '../lib/store'
import { folders } from '../lib/machines'
import { InputBox, SendButton, SettingsPickers, useInput } from './Composer'
import { Btn, Menu, inputClass, type MenuItem } from './bits'
import { providerLabel, providerOf } from '../lib/catalog'
import type { AgentSettings } from '../proto/AgentSettings'
import type { Thread } from '../proto/Thread'
import type { Project } from '../proto/Project'

type Target = { key: string; project: ProjectX; machine?: string; label: string; laptop: boolean }

export function NewThread(props: { project?: string }) {
  const names = createMemo(() => [...new Set(Object.values(state.projects).map((p) => p.name))].sort((a, b) => a.localeCompare(b)))
  const [name, setName] = createSignal<string | undefined>(props.project ?? localStorage.getItem('corra_last_project') ?? undefined)
  createEffect(() => {
    if (props.project) setName(props.project)
  })
  const recent = () => {
    const t = Object.values(state.threads).sort((a, b) => b.updated_at - a.updated_at)[0]
    return t ? state.projects[t.project]?.name : undefined
  }
  const current = () => (name() && names().includes(name()!) ? name()! : (recent() ?? names()[0]))
  const targets = createMemo<Target[]>(() => {
    const n = current()
    if (!n) return []
    const out: Target[] = []
    for (const p of Object.values(state.projects)) {
      if (p.name !== n) continue
      if (providerOf(p.provider)?.creates_machines)
        out.push({ key: `hub:${p.provider}`, project: p, label: `${providerLabel(p.provider)} machine`, laptop: false })
      else {
        const m = p.default_machine ? state.machines[p.default_machine] : undefined
        const here = !!m && m.id === state.thisMachine
        if (m) out.push({ key: `hub:${m.id}`, project: p, machine: m.id, label: here ? 'This computer' : m.name, laptop: here || !!p.path })
      }
    }
    return out.sort((a, b) => Number(b.machine === state.thisMachine) - Number(a.machine === state.thisMachine))
  })
  const [targetKey, setTargetKey] = createSignal<string>(localStorage.getItem('corra_last_target') ?? '')
  const target = () => targets().find((t) => t.key === targetKey()) ?? targets()[0]
  const [settings, setSettings] = createSignal<AgentSettings>(
    JSON.parse(localStorage.getItem('corra_last_settings') ?? 'null') ?? {
      model: 'claude-opus-5-5',
      effort: 'high',
      permission_mode: 'default',
    },
  )
  const [error, setError] = createSignal('')
  const [picking, setPicking] = createSignal(false)
  const [busy, setBusy] = createSignal(false)
  const input = useInput(() => 'new')

  const start = async () => {
    const parts = input.parts()
    const t = target()
    if (!parts.length || !t || busy()) return
    setError('')
    setBusy(true)
    try {
      localStorage.setItem('corra_last_project', t.project.name)
      localStorage.setItem('corra_last_target', t.key)
      localStorage.setItem('corra_last_settings', JSON.stringify(settings()))
      const thread = await call<Thread>({
        op: 'create_thread',
        project: t.project.id,
        agent: 'claude',
        machine: t.machine,
        settings: settings(),
        input: parts,
      })
      input.clear()
      location.hash = `#/t/${thread.id}`
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const openFolder = async (path: string) => {
    setError('')
    try {
      const machine = state.thisMachine!
      const existing = Object.values(state.projects).find((p) => p.default_machine === machine && p.path === path)
      let p: Project | undefined = existing
      if (!p) {
        let base = (path.split('/').filter(Boolean).pop() ?? 'project').replace(/[^A-Za-z0-9._-]/g, '-')
        if (Object.values(state.projects).some((x) => x.name === base)) base = `${base}-${Math.random().toString(36).slice(2, 5)}`
        p = await call<Project>({ op: 'create_project', name: base, path, machine, repos: [], env: [] })
      }
      setName(p.name)
      setTargetKey(`hub:${machine}`)
      setPicking(false)
    } catch (e) {
      setError((e as Error).message)
    }
  }

  const projectItems = createMemo<MenuItem[]>(() => {
    const items: MenuItem[] = names().map((n) => ({ label: n, checked: n === current(), onSelect: () => setName(n) }))
    if (state.thisMachine)
      items.push(
        ...(items.length ? (['sep'] as MenuItem[]) : []),
        { label: 'Open a folder…', onSelect: () => setPicking(true) },
        { label: 'Bring in a Claude Code session…', onSelect: () => (location.hash = '#/import') },
      )
    items.push({ label: 'New project…', onSelect: () => (location.hash = '#/new-project') })
    return items
  })
  const pill = 'inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-raised px-2.5 text-[13px] hover:border-faint'
  const pickers = SettingsPickers({
    get settings() {
      return settings()
    },
    onChange: (s) => setSettings({ ...settings(), ...s }),
  })
  const ready = () => state.loaded.hub

  return (
    <div class="relative flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-4 py-8 sm:px-6">
      <a
        href="#/"
        aria-label="Back to threads"
        class="absolute top-2.5 left-2 flex size-8 items-center justify-center rounded-lg text-mute hover:bg-ink/5 md:hidden"
      >
        <ArrowLeft size={18} />
      </a>
      <div class="w-full max-w-2xl">
        <Show
          when={!picking() || !state.thisMachine}
          fallback={<FolderPicker machine={state.thisMachine!} onPick={openFolder} onCancel={() => setPicking(false)} />}
        >
          <Show
            when={names().length > 0}
            fallback={
              <Show when={ready()}>
                <div class="text-center">
                  <h1 class="text-[20px] font-semibold tracking-tight text-balance">
                    {state.thisMachine ? 'Open a folder to start' : 'No projects yet'}
                  </h1>
                  <p class="mx-auto mt-2 max-w-sm text-[13.5px] text-mute">
                    {state.thisMachine
                      ? 'Corra runs Claude Code in that folder, on this computer.'
                      : 'Make one from a GitHub repo, or open a folder in the corra app on your computer.'}
                  </p>
                  <div class="mt-5 flex justify-center">
                    <Show
                      when={state.thisMachine}
                      fallback={
                        <a class="text-[13px] underline decoration-line underline-offset-4" href="#/new-project">
                          New project
                        </a>
                      }
                    >
                      <Btn kind="primary" onClick={() => setPicking(true)}>
                        <Folder size={15} /> Open a folder
                      </Btn>
                    </Show>
                  </div>
                </div>
              </Show>
            }
          >
            <div class="mb-3 flex flex-wrap items-center gap-2">
              <Menu label="Project" items={projectItems()} class={pill}>
                <span class="text-mute">Project</span>
                <span class="font-medium">{current()}</span>
                <ChevronDown size={13} class="text-faint" />
              </Menu>
              <Show
                when={targets().length > 0}
                fallback={<span class="text-[12.5px] text-mute">This project has nowhere to run yet.</span>}
              >
                <Menu
                  label="Where it runs"
                  class={pill}
                  items={targets().map((t) => ({ label: t.label, checked: t.key === target()?.key, onSelect: () => setTargetKey(t.key) }))}
                >
                  <span class="text-mute">Runs on</span>
                  {target()?.laptop ? <Laptop size={14} class="text-mute" /> : <Cloud size={14} class="text-mute" />}
                  <span class="font-medium">{target()?.label}</span>
                  <Show when={targets().length > 1}>
                    <ChevronDown size={13} class="text-faint" />
                  </Show>
                </Menu>
              </Show>
            </div>
            <InputBox
              input={input}
              autofocus
              placeholder="What should the agent do?"
              onSubmit={() => void start()}
              left={pickers.mode}
              right={
                <>
                  {pickers.model}
                  <SendButton label="Start" disabled={!input.parts().length || !target() || busy()} onClick={start} />
                </>
              }
            />
            <Show when={target()?.project.path}>
              <p class="mt-2 px-1 font-mono text-[11.5px] text-faint">{target()!.project.path}</p>
            </Show>
          </Show>
        </Show>
        <Show when={error()}>
          <div class="mt-3 text-[12.5px] text-bad">{error()}</div>
        </Show>
      </div>
    </div>
  )
}

export function FolderPicker(props: { machine: string; onPick: (path: string) => void; onCancel: () => void }) {
  const [path, setPath] = createSignal('')
  const [list] = createResource(path, (p) => folders(props.machine, p))
  createEffect(() => {
    const l = list()
    if (l && !path().trim()) setPath(l.path)
  })
  const go = (p: string) => setPath(p)
  const isHome = () => !path().trim() || path().replace(/\/$/, '') === (list()?.home ?? '').replace(/\/$/, '')
  return (
    <div class="overflow-hidden rounded-2xl border border-line bg-raised shadow-[var(--shadow)]">
      <div class="border-b border-line p-3">
        <input
          ref={(el) => requestAnimationFrame(() => el.focus())}
          class={`${inputClass} w-full font-mono text-[12.5px]`}
          spellcheck={false}
          value={path()}
          onInput={(e) => setPath(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && list()?.exists && !isHome()) props.onPick(path().replace(/\/$/, ''))
            if (e.key === 'Escape') props.onCancel()
          }}
        />
      </div>
      <div class="h-72 overflow-y-auto p-1">
        <Show when={list()?.parent && list()!.exists}>
          <button
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] text-mute hover:bg-ink/5"
            onClick={() => go(list()!.parent!)}
          >
            <CornerLeftUp size={14} /> Up
          </button>
        </Show>
        <For
          each={list()?.entries ?? []}
          fallback={
            <Show when={!list.loading}>
              <div class="px-2.5 py-3 text-[13px] text-faint">No folders here</div>
            </Show>
          }
        >
          {(e) => (
            <button
              class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] hover:bg-ink/5"
              onClick={() => go(e.path)}
              onDblClick={() => props.onPick(e.path)}
            >
              {e.git ? <FolderGit2 size={14} class="text-accent" /> : <Folder size={14} class="text-mute" />}
              <span class="truncate">{e.name}</span>
            </button>
          )}
        </For>
      </div>
      <div class="flex items-center gap-2 border-t border-line p-3">
        <span class="min-w-0 flex-1 truncate text-[12px] text-mute">
          <Show when={list()?.exists} fallback="Type a folder's path, or pick one above">
            {isHome() ? 'Pick the project folder' : list()?.git ? 'A git repository' : 'A folder'}
          </Show>
        </span>
        <Btn kind="ghost" onClick={props.onCancel}>
          Cancel
        </Btn>
        <Btn
          kind="primary"
          disabled={!list()?.exists || isHome()}
          onClick={() => props.onPick((list()?.path ?? path()).replace(/\/$/, ''))}
        >
          Open this folder
        </Btn>
      </div>
    </div>
  )
}
