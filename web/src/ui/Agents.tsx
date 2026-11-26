import { For, Show, createEffect, createMemo, createSignal, on } from 'solid-js'
import Bot from 'lucide-solid/icons/bot'
import SquareTerminal from 'lucide-solid/icons/square-terminal'
import ChevronRight from 'lucide-solid/icons/chevron-right'
import { call, state } from '../lib/store'
import { setPanel } from '../lib/panel'
import { duration } from '../lib/format'
import { agentInfo, findCall, isAgentCall, kidsOf, type AgentInfo } from '../lib/agents'
import type { ItemView } from '../lib/timeline'
import { toolSummary } from './tools'
import { EntryView, Markdown } from './Timeline'
import { Btn } from './bits'

const [now, setNow] = createSignal(Date.now())
setInterval(() => setNow(Date.now()), 1000)

function lastStep(it: ItemView | undefined, cwd?: string): string {
  if (!it) return ''
  const b = it.body
  if (b.type === 'tool_call') {
    const s = toolSummary(b.name, b.input, it.liveInput, cwd)
    return [s.verb, s.detail].filter(Boolean).join(' ')
  }
  if (b.type === 'agent_message') return (it.live ?? b.text).trim().split('\n')[0]
  if (b.type === 'reasoning') return 'Thinking'
  return ''
}

function stateLine(a: AgentInfo): string {
  const took = a.started ? duration((a.ended ?? now()) - a.started) : ''
  switch (a.state) {
    case 'running':
      return took ? `Working ${took}` : 'Working'
    case 'done':
      return took ? `Done in ${took}` : 'Done'
    case 'failed':
      return took ? `Failed after ${took}` : 'Failed'
    case 'stopped':
      return took ? `Stopped after ${took}` : 'Stopped'
  }
}

function Face(props: { agent: boolean; size?: 'sm' | 'lg' }) {
  return (
    <span
      class="flex shrink-0 items-center justify-center rounded-full"
      classList={{
        'size-6': props.size !== 'lg',
        'size-8': props.size === 'lg',
        'bg-accent/10 text-accent': props.agent,
        'bg-ink/[0.05] text-mute': !props.agent,
      }}
    >
      {props.agent ? <Bot size={props.size === 'lg' ? 16 : 13} /> : <SquareTerminal size={props.size === 'lg' ? 16 : 13} />}
    </span>
  )
}

export function AgentRow(props: { thread?: string; item: ItemView; kids?: Map<string, ItemView[]>; cwd?: string }) {
  const a = createMemo(() =>
    agentInfo(props.item, props.kids?.get(props.item.id) ?? [], props.thread ? state.live[props.thread]?.agents : undefined, now()),
  )
  const detail = () => (a().state === 'running' ? lastStep(a().latest, props.cwd) : (a().result ?? '').trim().split('\n')[0])
  return (
    <button
      class="group flex w-full items-start gap-2.5 text-left"
      onClick={() => setPanel({ kind: 'agent', tool: props.item.id })}
      title="Open this agent"
    >
      <Face agent />
      <span class="min-w-0 flex-1 leading-[1.45]">
        <span class="flex items-baseline gap-2 text-[13px]">
          <span class="min-w-0 truncate font-medium text-ink">{a().description}</span>
          <Show when={a().kind}>
            <span class="shrink-0 text-[12px] text-faint">{a().kind}</span>
          </Show>
        </span>
        <span class="flex items-baseline gap-1.5 text-[12.5px] text-mute">
          <Show when={a().state === 'running'}>
            <span class="size-1.5 shrink-0 translate-y-[-1px] self-center rounded-full bg-accent pulse" />
          </Show>
          <span class="num shrink-0" classList={{ 'text-bad': a().state === 'failed' }}>
            {stateLine(a())}
          </span>
          <Show when={detail()}>
            <span class="min-w-0 truncate text-faint">· {detail()}</span>
          </Show>
        </span>
      </span>
      <ChevronRight size={14} class="mt-1 shrink-0 text-faint transition-colors duration-100 group-hover:text-ink" />
    </button>
  )
}

export function AgentPanel(props: { thread: string; tool: string; cwd?: string }) {
  const found = createMemo(() => findCall(state.views[props.thread], props.tool))
  const kids = createMemo(() => (found() ? kidsOf(found()!.turn) : new Map<string, ItemView[]>()))
  const a = createMemo(() =>
    found() ? agentInfo(found()!.item, kids().get(props.tool) ?? [], state.live[props.thread]?.agents, now()) : undefined,
  )
  const steps = () => (kids().get(props.tool) ?? []).filter((k) => !k.id.startsWith('task:'))
  const count = () => steps().filter((s) => s.body.type === 'tool_call').length
  const [whole, setWhole] = createSignal(false)
  const [clipped, setClipped] = createSignal(false)
  let asked: HTMLDivElement | undefined
  createEffect(
    on(
      () => a()?.prompt,
      () => requestAnimationFrame(() => asked && setClipped(asked.scrollHeight > asked.clientHeight + 1)),
    ),
  )
  const [stopping, setStopping] = createSignal(false)
  const stop = () => {
    setStopping(true)
    void call({ op: 'stop_agent', thread: props.thread, tool: props.tool }).catch(() => setStopping(false))
  }
  return (
    <Show
      when={a()}
      fallback={<div class="px-4 py-6 text-[13px] text-mute">This agent isn't loaded. Scroll up in the thread to load its turn.</div>}
    >
      {(info) => (
        <div class="space-y-6 px-4 py-5">
          <div class="flex items-start gap-3">
            <Face agent size="lg" />
            <div class="min-w-0 flex-1">
              <div class="text-[15px] leading-snug font-medium text-balance">{info().description}</div>
              <div class="mt-0.5 flex items-center gap-1.5 text-[12.5px] text-mute">
                <Show when={info().state === 'running'}>
                  <span class="size-1.5 shrink-0 rounded-full bg-accent pulse" />
                </Show>
                <span class="num" classList={{ 'text-bad': info().state === 'failed' }}>
                  {[info().kind, info().background ? 'in the background' : '', stateLine(info())].filter(Boolean).join(' · ')}
                </span>
              </div>
            </div>
            <Show when={info().state === 'running'}>
              <Btn small kind="danger" disabled={stopping()} onClick={stop}>
                {stopping() ? 'Stopping…' : 'Stop'}
              </Btn>
            </Show>
          </div>
          <Show when={info().prompt}>
            <section>
              <div class="mb-1.5 text-[12px] text-faint">Asked</div>
              <div class="rounded-xl bg-panel px-3.5 py-2.5 text-[13px] leading-relaxed whitespace-pre-wrap">
                <div ref={asked} classList={{ 'line-clamp-6': !whole() }}>
                  {info().prompt}
                </div>
                <Show when={clipped() || whole()}>
                  <button class="mt-1 text-[12.5px] font-medium text-mute hover:text-ink" onClick={() => setWhole(!whole())}>
                    {whole() ? 'Show less' : 'Show more'}
                  </button>
                </Show>
              </div>
            </section>
          </Show>
          <section>
            <div class="mb-2 text-[12px] text-faint">
              {count() === 0 ? (info().state === 'running' ? 'Starting' : 'No steps') : `${count()} ${count() === 1 ? 'step' : 'steps'}`}
            </div>
            <div class="space-y-2.5 border-l border-line pl-4">
              <For each={steps()}>
                {(s) => <EntryView thread={props.thread} entry={{ type: 'item', item: s }} kids={kids()} cwd={props.cwd} />}
              </For>
              <Show when={info().state === 'running'}>
                <div class="flex items-center gap-2 text-[13px] text-mute">
                  <span class="size-2 rounded-full bg-accent pulse" />
                  Working
                </div>
              </Show>
            </div>
          </section>
          <Show when={info().result}>
            <section>
              <div class="mb-1.5 text-[12px] text-faint">Result</div>
              <Markdown text={info().result!} />
            </section>
          </Show>
        </div>
      )}
    </Show>
  )
}

export function RunningTasks(props: { thread: string; cwd?: string }) {
  const rows = createMemo(() =>
    (state.live[props.thread]?.agents ?? []).flatMap((id) => {
      const f = findCall(state.views[props.thread], id)
      return f ? [{ ...f, kids: kidsOf(f.turn) }] : []
    }),
  )
  return (
    <Show when={rows().length > 0}>
      <div class="space-y-3 pb-4">
        <div class="text-[12px] text-faint">In the background</div>
        <For each={rows()}>
          {(r) => (
            <div class="flex items-center gap-2">
              <div class="min-w-0 flex-1">
                <Show when={isAgentCall(r.item)} fallback={<CommandRow item={r.item} cwd={props.cwd} />}>
                  <AgentRow thread={props.thread} item={r.item} kids={r.kids} cwd={props.cwd} />
                </Show>
              </div>
              <Btn
                small
                kind="ghost"
                onClick={() => void call({ op: 'stop_agent', thread: props.thread, tool: r.item.id }).catch(() => {})}
              >
                Stop
              </Btn>
            </div>
          )}
        </For>
      </div>
    </Show>
  )
}

function CommandRow(props: { item: ItemView; cwd?: string }) {
  const b = () => props.item.body as Extract<ItemView['body'], { type: 'tool_call' }>
  const s = () => toolSummary(b().name, b().input, undefined, props.cwd)
  return (
    <div class="flex items-start gap-2.5">
      <Face agent={false} />
      <span class="min-w-0 flex-1 leading-[1.45]">
        <span class="block truncate text-[13px] font-medium text-ink">{s().detail}</span>
        <span class="flex items-center gap-1.5 text-[12.5px] text-mute">
          <span class="size-1.5 shrink-0 rounded-full bg-accent pulse" />
          <span class="num">{props.item.at ? `Running ${duration(now() - props.item.at)}` : 'Running'}</span>
        </span>
      </span>
    </div>
  )
}
