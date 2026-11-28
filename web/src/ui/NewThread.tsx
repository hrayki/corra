import { Show, createEffect, createMemo, createSignal } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import Laptop from 'lucide-solid/icons/laptop'
import Cloud from 'lucide-solid/icons/cloud'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import { addPending, call, state } from '../lib/store'
import { InputBox, SendButton, SettingsPickers, useInput } from './Composer'
import { Menu, type MenuItem } from './bits'
import { paired, providerLabel, providerOf } from '../lib/catalog'
import type { AgentSettings } from '../proto/AgentSettings'
import type { AgentKind } from '../proto/AgentKind'
import type { Thread } from '../proto/Thread'
import type { Project } from '../proto/Project'

type Target = { key: string; project: Project; machine?: string; label: string; laptop: boolean }

const AGENTS: { id: AgentKind; name: string; hint: string }[] = [
  { id: 'claude', name: 'Claude Code', hint: 'Anthropic' },
  { id: 'antigravity', name: 'Antigravity', hint: 'Google, Gemini' },
]

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
        if (m) out.push({ key: `hub:${m.id}`, project: p, machine: m.id, label: m.name, laptop: true })
      }
    }
    return out
  })
  const [targetKey, setTargetKey] = createSignal<string>(localStorage.getItem('corra_last_target') ?? '')
  const target = () => targets().find((t) => t.key === targetKey()) ?? targets()[0]
  const [agent, setAgentRaw] = createSignal<AgentKind>(
    localStorage.getItem('corra_last_agent') === 'antigravity' ? 'antigravity' : 'claude',
  )
  const defaults = (a: AgentKind): AgentSettings =>
    JSON.parse(localStorage.getItem(a === 'claude' ? 'corra_last_settings' : `corra_last_settings_${a}`) ?? 'null') ?? {
      effort: 'high',
      permission_mode: 'default',
    }
  const [settings, setSettings] = createSignal<AgentSettings>(defaults(agent()))
  const setAgent = (a: AgentKind) => {
    setAgentRaw(a)
    setSettings(defaults(a))
  }
  const [error, setError] = createSignal('')
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
      localStorage.setItem('corra_last_agent', agent())
      localStorage.setItem(agent() === 'claude' ? 'corra_last_settings' : `corra_last_settings_${agent()}`, JSON.stringify(settings()))
      const thread = await call<Thread>({
        op: 'create_thread',
        project: t.project.id,
        agent: agent(),
        machine: t.machine,
        settings: settings(),
        input: parts,
      })
      addPending(thread.id, parts, false)
      input.clear()
      location.hash = `#/t/${thread.id}`
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const hasPaired = createMemo(() => Object.values(state.machines).some((m) => paired(m.provider) && m.status === 'connected'))
  const projectItems = createMemo<MenuItem[]>(() => {
    const items: MenuItem[] = names().map((n) => ({ label: n, checked: n === current(), onSelect: () => setName(n) }))
    if (hasPaired())
      items.push(...(items.length ? (['sep'] as MenuItem[]) : []), {
        label: 'Bring in a Claude Code session…',
        onSelect: () => (location.hash = '#/import'),
      })
    items.push({ label: 'New project…', onSelect: () => (location.hash = '#/new-project') })
    return items
  })
  const pill = 'inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-raised px-2.5 text-[13px] hover:border-faint'
  const pickers = SettingsPickers({
    get settings() {
      return settings()
    },
    get agent() {
      return agent()
    },
    onChange: (s) => setSettings({ ...settings(), ...s }),
  })
  const ready = () => state.loaded

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
          when={names().length > 0}
          fallback={
            <Show when={ready()}>
              <div class="text-center">
                <h1 class="text-[20px] font-semibold tracking-tight text-balance">No projects yet</h1>
                <p class="mx-auto mt-2 max-w-sm text-[13.5px] text-mute">Make one from a GitHub repo to start a thread.</p>
                <div class="mt-5 flex justify-center">
                  <a class="text-[13px] underline decoration-line underline-offset-4" href="#/new-project">
                    New project
                  </a>
                </div>
              </div>
            </Show>
          }
        >
          <div class="mb-3 flex flex-wrap items-center gap-2">
            <Menu
              label="Agent"
              class={pill}
              items={AGENTS.map((a) => ({ label: a.name, hint: a.hint, checked: agent() === a.id, onSelect: () => setAgent(a.id) }))}
            >
              <span class="text-mute">Agent</span>
              <span class="font-medium">{AGENTS.find((a) => a.id === agent())!.name}</span>
              <ChevronDown size={13} class="text-faint" />
            </Menu>
            <Menu label="Project" items={projectItems()} class={pill}>
              <span class="text-mute">Project</span>
              <span class="font-medium">{current()}</span>
              <ChevronDown size={13} class="text-faint" />
            </Menu>
            <Show when={targets().length > 0} fallback={<span class="text-[12.5px] text-mute">This project has nowhere to run yet.</span>}>
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
        <Show when={error()}>
          <div class="mt-3 text-[12.5px] text-bad">{error()}</div>
        </Show>
      </div>
    </div>
  )
}
