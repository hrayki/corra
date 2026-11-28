import { For, Match, Show, Switch, createEffect, createSignal, onCleanup } from 'solid-js'
import GitMerge from 'lucide-solid/icons/git-merge'
import GitBranch from 'lucide-solid/icons/git-branch'
import ShieldAlert from 'lucide-solid/icons/shield-exclamation-point'
import { call } from '../lib/store'
import type { RequestView } from '../lib/timeline'
import type { RequestResponse } from '../proto/RequestResponse'
import { approvalBody, approvalHeading } from '../lib/approval'
import { Btn, inputClass } from './bits'
import { ToolDetail, canonicalToolName, toolSummary } from './tools'
import { Markdown } from './Markdown'

const FIELD_ORDER = ['title', 'repo', 'number', 'push', 'head', 'base', 'sha', 'method']

function hubFields(input: unknown): [string, unknown][] {
  const rank = (k: string) => {
    const i = FIELD_ORDER.indexOf(k)
    return i < 0 ? FIELD_ORDER.length : i
  }
  return Object.entries((input ?? {}) as Record<string, unknown>).sort(([a], [b]) => rank(a) - rank(b))
}

function resolvedLabel(r?: RequestResponse) {
  if (!r) return ''
  if (r.type === 'allow') return r.always ? 'Allowed for this session' : 'Allowed'
  if (r.type === 'deny') return 'Denied'
  if (r.type === 'answers') return 'Answered'
  return 'Cancelled'
}

function secretLabel(r?: RequestResponse) {
  if (!r) return ''
  if (r.type === 'allow') return 'Gave the secret'
  if (r.type === 'deny') return 'Declined to give'
  return 'Cancelled'
}

function resolvedDetail(req: RequestView['request'], cwd?: string): string {
  if (req.type === 'tool_approval') {
    const s = toolSummary(req.tool, req.input, undefined, cwd)
    return s.detail || (s.verb !== 'Tool' ? s.verb : '')
  }
  if (req.type === 'hub_action') {
    if (req.action === 'agent.sign_in') return ''
    const inp = (req.input ?? {}) as Record<string, unknown>
    if (req.action === 'secret.request') return String(inp.name ?? '')
    if (req.action === 'github.pr.merge') {
      const repo = String(inp.repo ?? '')
      const num = inp.number != null ? `#${inp.number}` : ''
      return [repo, num].filter(Boolean).join(' ')
    }
    if (req.action === 'git.push') {
      const repo = String(inp.repo ?? '')
      const refs = Array.isArray(inp.push) ? inp.push.map((r) => String(r).replace(/^refs\/heads\//, '')).join(', ') : ''
      return [repo, refs].filter(Boolean).join(' ')
    }
    return req.action
  }
  if (req.type === 'plan_approval') return 'the plan'
  return ''
}

export function RequestCard(props: { thread: string; r: RequestView; cwd?: string }) {
  const req = () => props.r.request
  const pending = () => !props.r.response
  const isSecret = () => req().type === 'hub_action' && (req() as { action: string }).action === 'secret.request'
  const isSignIn = () => req().type === 'hub_action' && (req() as { action: string }).action === 'agent.sign_in'
  const [error, setError] = createSignal('')
  const [sending, setSending] = createSignal(false)
  let stuck: number | undefined
  onCleanup(() => clearTimeout(stuck))
  const respond = (response: RequestResponse) => {
    if (sending()) return
    setSending(true)
    setError('')
    stuck = window.setTimeout(() => setSending(false), 15_000)
    void call({ op: 'respond', thread: props.thread, request: props.r.id, response }).catch((e) => {
      clearTimeout(stuck)
      setSending(false)
      setError((e as Error).message)
    })
  }
  let card: HTMLDivElement | undefined
  const allowEnter = () => pending() && req().type === 'tool_approval'
  const allowEscape = () => pending() && (req().type === 'tool_approval' || (req().type === 'hub_action' && !isSecret() && !isSignIn()))
  createEffect(() => {
    if (!allowEscape()) return
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (el && (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT' || el.isContentEditable)) return
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat || sending()) return
      if (card !== document.querySelector('[data-approval]')) return
      if (e.key === 'Enter' && allowEnter()) {
        e.preventDefault()
        respond({ type: 'allow', always: false })
      } else if (e.key === 'Escape') {
        e.preventDefault()
        respond({ type: 'deny' })
      }
    }
    window.addEventListener('keydown', onKey)
    onCleanup(() => window.removeEventListener('keydown', onKey))
  })
  return (
    <Show
      when={pending()}
      fallback={
        <div class="text-[13px] text-mute">
          <span class="font-medium">
            {isSignIn()
              ? props.r.response?.type === 'allow'
                ? 'Sent a sign-in code'
                : 'Sign-in cancelled'
              : isSecret()
                ? secretLabel(props.r.response)
                : resolvedLabel(props.r.response)}
          </span>{' '}
          <span class="font-mono text-[12px] text-faint">{resolvedDetail(req(), props.cwd)}</span>
        </div>
      }
    >
      <div ref={card} data-approval={allowEscape() ? '' : undefined} class="rounded-2xl border border-warn/50 bg-warn/[0.06] p-4">
        <Switch>
          <Match when={req().type === 'tool_approval' && req()}>
            {(r) => {
              const x = r() as Extract<RequestView['request'], { type: 'tool_approval' }>
              const tool = canonicalToolName(x.tool)
              const s = toolSummary(x.tool, x.input, undefined, props.cwd)
              const heading = approvalHeading(x.tool, x.input)
              const body = approvalBody(x.input, s, heading)
              return (
                <>
                  <div class="text-[14px] font-medium">{heading}</div>
                  <Show
                    when={tool === 'Edit' || tool === 'MultiEdit' || tool === 'Write'}
                    fallback={
                      <Show when={body}>
                        <div class="mt-2 max-h-40 overflow-auto rounded-lg bg-bg/70 px-3 py-2 font-mono text-[12px] break-all whitespace-pre-wrap">
                          {body}
                        </div>
                      </Show>
                    }
                  >
                    <div class="mt-1 mb-2 font-mono text-[12px] text-mute">{s.detail}</div>
                    <ToolDetail name={x.tool} input={x.input} />
                  </Show>
                  <Show when={x.reason}>
                    <div class="mt-1.5 text-[12.5px] text-mute">{x.reason}</div>
                  </Show>
                  <div class="mt-3 flex flex-wrap gap-2">
                    <Btn kind="primary" disabled={sending()} onClick={() => respond({ type: 'allow', always: false })}>
                      Allow once
                    </Btn>
                    <Btn disabled={sending()} onClick={() => respond({ type: 'allow', always: true })}>
                      Allow for this session
                    </Btn>
                    <Btn kind="ghost" disabled={sending()} onClick={() => respond({ type: 'deny' })}>
                      Deny
                    </Btn>
                    <span class="self-center text-[12px] text-faint max-md:hidden">Enter allows once, Esc denies</span>
                  </div>
                </>
              )
            }}
          </Match>
          <Match when={req().type === 'question' && req()}>
            {(r) => (
              <QuestionForm
                r={r() as Extract<RequestView['request'], { type: 'question' }>}
                disabled={sending()}
                onAnswer={(a) => respond({ type: 'answers', answers: a })}
              />
            )}
          </Match>
          <Match when={req().type === 'plan_approval' && req()}>
            {(r) => (
              <>
                <div class="mb-2 text-[12.5px] font-medium text-mute">Plan</div>
                <Markdown text={(r() as { plan: string }).plan} />
                <div class="mt-4 flex flex-wrap gap-2">
                  <Btn kind="primary" disabled={sending()} onClick={() => respond({ type: 'allow', always: false })}>
                    Approve and start
                  </Btn>
                  <Btn
                    kind="ghost"
                    disabled={sending()}
                    onClick={() => respond({ type: 'deny', message: 'The user wants to keep planning. Wait for their feedback.' })}
                  >
                    Keep planning
                  </Btn>
                </div>
              </>
            )}
          </Match>
          <Match when={isSignIn() && req()}>
            {(r) => {
              const x = r() as { input: { agent?: string; url?: string } }
              return (
                <SignInForm
                  agent={String(x.input?.agent ?? 'the agent')}
                  url={String(x.input?.url ?? '')}
                  disabled={sending()}
                  onSend={(code) =>
                    call({ op: 'answer_secret', thread: props.thread, request: props.r.id, value: code }).catch((e) =>
                      setError((e as Error).message),
                    )
                  }
                  onCancel={() => respond({ type: 'deny' })}
                />
              )
            }}
          </Match>
          <Match when={isSecret() && req()}>
            {(r) => {
              const x = r() as { input: { name?: string; why?: string } }
              return (
                <SecretForm
                  name={String(x.input?.name ?? '')}
                  why={String(x.input?.why ?? '')}
                  onSend={(value) =>
                    call({ op: 'answer_secret', thread: props.thread, request: props.r.id, value }).catch((e) =>
                      setError((e as Error).message),
                    )
                  }
                  disabled={sending()}
                  onDecline={() => respond({ type: 'deny' })}
                />
              )
            }}
          </Match>
          <Match when={req().type === 'hub_action' && req()}>
            {(r) => {
              const x = r() as Extract<RequestView['request'], { type: 'hub_action' }>
              return <HubActionCard action={x.action} input={x.input} sending={sending()} onRespond={respond} />
            }}
          </Match>
        </Switch>
        <Show when={error()}>
          <div class="mt-2 text-[12.5px] text-bad">{error()}</div>
        </Show>
      </div>
    </Show>
  )
}

function HubActionCard(props: { action: string; input: unknown; sending: boolean; onRespond: (r: RequestResponse) => void }) {
  const inp = () => (props.input && typeof props.input === 'object' ? props.input : {}) as Record<string, unknown>

  if (props.action === 'github.pr.merge') {
    const repo = String(inp().repo ?? '')
    const num = inp().number != null ? `#${inp().number}` : ''
    const title = String(inp().title ?? '')
    const head = String(inp().head ?? '')
    const base = String(inp().base ?? '')
    const sha = String(inp().sha ?? '').slice(0, 7)
    const method = String(inp().method ?? 'squash')
    return (
      <>
        <div class="flex items-center gap-2 text-[14px] font-medium">
          <GitMerge size={16} class="shrink-0 text-warn" />
          <span>Merge pull request {num}?</span>
        </div>
        <div class="mt-2.5 space-y-2 rounded-xl border border-line bg-bg/70 px-3.5 py-2.5 text-[13px]">
          <Show when={title}>
            <div class="font-medium text-ink">{title}</div>
          </Show>
          <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px] text-mute">
            <Show when={repo}>
              <span class="font-mono text-ink/85">{repo}</span>
            </Show>
            <Show when={head && base}>
              <span class="inline-flex items-center gap-1 font-mono">
                <span class="rounded bg-ink/[0.06] px-1.5 py-0.5 text-ink">{head}</span>
                <span>→</span>
                <span class="rounded bg-ink/[0.06] px-1.5 py-0.5 text-ink">{base}</span>
              </span>
            </Show>
            <span class="rounded bg-ink/[0.06] px-1.5 py-0.5">{method}</span>
            <Show when={sha}>
              <span class="font-mono text-faint">{sha}</span>
            </Show>
          </div>
        </div>
        <div class="mt-3 flex flex-wrap items-center gap-2">
          <Btn kind="primary" disabled={props.sending} onClick={() => props.onRespond({ type: 'allow', always: false })}>
            Merge PR
          </Btn>
          <Btn kind="ghost" disabled={props.sending} onClick={() => props.onRespond({ type: 'deny' })}>
            Deny
          </Btn>
          <span class="text-[12px] text-faint">Click to confirm · Enter disabled</span>
        </div>
      </>
    )
  }

  if (props.action === 'git.push') {
    const repo = String(inp().repo ?? '')
    const rawPush = inp().push
    const refs = Array.isArray(rawPush) ? rawPush.map((v) => String(v)) : []
    const formatRef = (r: string) => {
      if (r.startsWith('refs/heads/')) return { kind: 'branch', name: r.slice(11) }
      if (r.startsWith('refs/tags/')) return { kind: 'tag', name: r.slice(10) }
      return { kind: 'ref', name: r }
    }
    return (
      <>
        <div class="flex items-center gap-2 text-[14px] font-medium">
          <ShieldAlert size={16} class="shrink-0 text-warn" />
          <span>Push to protected branch or tag?</span>
        </div>
        <div class="mt-2.5 space-y-2 rounded-xl border border-line bg-bg/70 px-3.5 py-2.5 text-[13px]">
          <Show when={repo}>
            <div class="font-mono text-[12.5px] font-medium text-ink">{repo}</div>
          </Show>
          <Show when={refs.length}>
            <div class="flex flex-wrap gap-1.5">
              <For each={refs}>
                {(r) => {
                  const info = formatRef(r)
                  return (
                    <span class="inline-flex items-center gap-1 rounded-md bg-ink/[0.06] px-2 py-0.5 font-mono text-[12px] text-ink">
                      <GitBranch size={11} class="text-mute" />
                      <span class="text-faint">{info.kind}</span>
                      <span>{info.name}</span>
                    </span>
                  )
                }}
              </For>
            </div>
          </Show>
        </div>
        <div class="mt-3 flex flex-wrap items-center gap-2">
          <Btn kind="primary" disabled={props.sending} onClick={() => props.onRespond({ type: 'allow', always: false })}>
            Allow push
          </Btn>
          <Btn kind="ghost" disabled={props.sending} onClick={() => props.onRespond({ type: 'deny' })}>
            Deny
          </Btn>
          <span class="text-[12px] text-faint">Click to confirm · Enter disabled</span>
        </div>
      </>
    )
  }

  return (
    <>
      <div class="text-[14px] font-medium">Allow {props.action.replace(/\./g, ' ')}?</div>
      <div class="mt-2 max-h-80 overflow-auto rounded-lg bg-bg/70 px-3 py-2 font-mono text-[12px]">
        <For each={hubFields(props.input)}>
          {([k, v]) => (
            <div class="flex gap-3">
              <span class="w-16 shrink-0 text-mute">{k}</span>
              <span class="min-w-0 break-words whitespace-pre-wrap">
                {Array.isArray(v) ? v.join('\n') : typeof v === 'object' ? JSON.stringify(v) : String(v)}
              </span>
            </div>
          )}
        </For>
      </div>
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <Btn kind="primary" disabled={props.sending} onClick={() => props.onRespond({ type: 'allow', always: false })}>
          Allow
        </Btn>
        <Btn kind="ghost" disabled={props.sending} onClick={() => props.onRespond({ type: 'deny' })}>
          Deny
        </Btn>
        <span class="text-[12px] text-faint">Click to confirm · Enter disabled</span>
      </div>
    </>
  )
}

function SignInForm(props: {
  agent: string
  url: string
  disabled?: boolean
  onSend: (code: string) => Promise<unknown>
  onCancel: () => void
}) {
  const [code, setCode] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const send = async () => {
    if (!code().trim() || busy()) return
    setBusy(true)
    await props.onSend(code().trim())
    setBusy(false)
  }
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      <div class="text-[14px] font-medium">Sign in to {props.agent} on this machine</div>
      <p class="mt-1 text-[13px] text-mute">Sign in with Google, then paste the code the page shows you, or the address it lands on.</p>
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <a
          href={props.url}
          target="_blank"
          rel="noopener noreferrer"
          class="inline-flex h-8 items-center rounded-lg border border-line bg-raised px-3 text-[13px] font-medium hover:border-faint"
        >
          Open Google sign-in
        </a>
      </div>
      <input
        class={`${inputClass} mt-3 w-full font-mono`}
        autocomplete="off"
        spellcheck={false}
        placeholder="Paste the code here"
        value={code()}
        onInput={(e) => setCode(e.currentTarget.value)}
      />
      <p class="mt-2 text-[12.5px] text-faint">The code goes straight to this machine and isn't kept in the conversation.</p>
      <div class="mt-3 flex gap-2">
        <Btn kind="primary" type="submit" disabled={!code().trim() || busy() || props.disabled}>
          {busy() ? 'Signing in…' : 'Sign in'}
        </Btn>
        <Btn kind="ghost" disabled={busy() || props.disabled} onClick={props.onCancel}>
          Cancel
        </Btn>
      </div>
    </form>
  )
}

function SecretForm(props: {
  name: string
  why: string
  disabled?: boolean
  onSend: (value: string) => Promise<unknown>
  onDecline: () => void
}) {
  const [value, setValue] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const send = async () => {
    if (!value() || busy()) return
    setBusy(true)
    await props.onSend(value())
    setValue('')
    setBusy(false)
  }
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      <div class="text-[14px] font-medium">
        Share <span class="font-mono">{props.name}</span>?
      </div>
      <Show when={props.why}>
        <p class="mt-1 text-[13px] text-mute">{props.why}</p>
      </Show>
      <input
        class={`${inputClass} mt-3 w-full font-mono`}
        type="password"
        autocomplete="off"
        spellcheck={false}
        placeholder="Paste it here"
        value={value()}
        onInput={(e) => setValue(e.currentTarget.value)}
      />
      <p class="mt-2 text-[12.5px] text-faint">
        It goes to a file on the thread's machine that only the agent reads, for a day. It isn't kept in the conversation.
      </p>
      <div class="mt-3 flex gap-2">
        <Btn kind="primary" type="submit" disabled={!value() || busy() || props.disabled}>
          Send
        </Btn>
        <Btn kind="ghost" disabled={busy() || props.disabled} onClick={props.onDecline}>
          Decline
        </Btn>
      </div>
    </form>
  )
}

function QuestionForm(props: {
  r: Extract<RequestView['request'], { type: 'question' }>
  disabled?: boolean
  onAnswer: (a: string[][]) => void
}) {
  const [picked, setPicked] = createSignal<string[][]>(props.r.questions.map(() => []))
  const [other, setOther] = createSignal<string[]>(props.r.questions.map(() => ''))
  const toggle = (qi: number, label: string, multi: boolean) =>
    setPicked((p) => p.map((x, i) => (i !== qi ? x : multi ? (x.includes(label) ? x.filter((l) => l !== label) : [...x, label]) : [label])))
  const answers = () => picked().map((p, i) => (other()[i].trim() ? [...p, other()[i].trim()] : p))
  const ready = () => answers().every((a) => a.length > 0)
  return (
    <div class="space-y-4">
      <For each={props.r.questions}>
        {(q, qi) => (
          <div>
            <div class="text-[14px] font-medium">{q.question}</div>
            <div class="mt-2 grid gap-1.5">
              <For each={q.options}>
                {(o) => {
                  const on = () => picked()[qi()].includes(o.label)
                  return (
                    <button
                      class="rounded-xl border bg-bg/60 px-3 py-2 text-left text-[13px] transition-colors duration-100"
                      classList={{ 'border-accent': on(), 'border-line hover:border-faint': !on() }}
                      aria-pressed={on()}
                      onClick={() => toggle(qi(), o.label, q.multi_select)}
                    >
                      <div class="font-medium">{o.label}</div>
                      <Show when={o.description}>
                        <div class="text-[12.5px] text-mute">{o.description}</div>
                      </Show>
                    </button>
                  )
                }}
              </For>
              <input
                class="h-9 rounded-xl border border-line bg-bg/60 px-3 text-[13px] placeholder:text-faint focus:border-faint"
                placeholder="Something else"
                value={other()[qi()]}
                onInput={(e) => setOther((o) => o.map((x, i) => (i === qi() ? e.currentTarget.value : x)))}
              />
            </div>
          </div>
        )}
      </For>
      <Btn kind="primary" disabled={!ready() || props.disabled} onClick={() => props.onAnswer(answers())}>
        Answer
      </Btn>
    </div>
  )
}
