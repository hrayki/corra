import { For, Show, createResource, createSignal } from 'solid-js'
import ChevronDown from 'lucide-solid/icons/chevron-down'
import { call, state, updateSettings } from '../../lib/store'
import { providers, tools } from '../../lib/catalog'
import type { KindInfo } from '../../proto/KindInfo'
import type { ProviderInfo } from '../../proto/ProviderInfo'
import { Btn, inputClass } from '../bits'
import { RepoPicker } from '../RepoPicker'
import { Card, InlineConfirm, KeyRow, Section, Status, secretNames } from './shared'

export function SecretsTab() {
  return (
    <>
      <Keys />
      <Providers />
      <Tools />
      <EnvVars />
      <MachineSetup />
    </>
  )
}

function Keys() {
  const known = new Set([
    'claude.token',
    'anthropic.api_key',
    'gemini.api_key',
    'railway.token',
    'cloudflare.token',
    'cloudflare.account_id',
    'github.token',
  ])
  const others = () =>
    (secretNames() ?? []).filter((n) => !known.has(n) && !providers().some((p) => p.credentials.some((c) => c.name === n)))
  return (
    <>
      <Section
        title="Agent credentials"
        hint="Kept encrypted on the hub; never read back. Claude uses the OAuth token first and falls back to the API key."
      >
        <Card>
          <KeyRow name="claude.token" label="Claude token" hint="From claude setup-token: your subscription." />
          <KeyRow name="anthropic.api_key" label="Anthropic API key" hint="Billed per use, from the Anthropic Console." />
          <KeyRow
            name="gemini.api_key"
            label="Gemini API key"
            hint="Optional for Antigravity. Without it, each machine signs in with Google and only you can prompt it."
          />
        </Card>
      </Section>
      <Section title="Manager project keys" hint="Only projects marked as a manager get these, as standard environment variables.">
        <Card>
          <KeyRow name="railway.token" label="Railway" hint="An account token, given as RAILWAY_API_TOKEN." />
          <KeyRow name="cloudflare.token" label="Cloudflare" hint="An API token, given as CLOUDFLARE_API_TOKEN." />
          <KeyRow name="cloudflare.account_id" label="Cloudflare account ID" hint="Given with the token as CLOUDFLARE_ACCOUNT_ID." />
        </Card>
      </Section>
      <Show when={others().length}>
        <Section title="Other keys" hint="Saved earlier; nothing in corra reads them.">
          <Card>
            <For each={others()}>{(n) => <KeyRow name={n} label={n} />}</For>
          </Card>
        </Section>
      </Show>
    </>
  )
}

const range = (xs: number[]) => {
  const lo = Math.min(...xs)
  const hi = Math.max(...xs)
  return lo === hi ? `${lo}` : `${lo}–${hi}`
}

function kindLine(k: KindInfo) {
  const parts = [k.sizes.length ? `${range(k.sizes.map((s) => s.cpu))} cores, ${range(k.sizes.map((s) => s.memory_gb))} GB` : 'Any size']
  parts.push(k.kvm ? 'Has KVM' : 'No KVM')
  if (k.max_hours) parts.push(`${k.max_hours} hours at most`)
  return parts.join(' · ')
}

function ProviderRow(props: { p: ProviderInfo }) {
  const [open, setOpen] = createSignal<boolean>(!props.p.connected)
  return (
    <div>
      <button
        class="flex w-full items-start gap-3 px-3.5 py-3 text-left hover:bg-ink/[0.025]"
        onClick={() => setOpen(!open())}
        aria-expanded={open()}
      >
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-center gap-x-2.5 gap-y-0.5">
            <span class="text-[13.5px] font-medium">{props.p.label}</span>
            <Status ok={props.p.connected} text={props.p.connected ? 'Connected' : 'Not connected'} />
          </div>
          <div class="mt-0.5 text-[12.5px] text-mute">{props.p.about}</div>
        </div>
        <ChevronDown size={15} class="mt-1 shrink-0 text-faint transition-transform" classList={{ 'rotate-180': open() }} />
      </button>
      <Show when={open()}>
        <div class="space-y-3 px-3.5 pb-3.5">
          <div class="divide-y divide-line">
            <For each={props.p.credentials}>{(c) => <KeyRow inset name={c.name} label={c.label} hint={c.hint} />}</For>
          </div>
          <div>
            <div class="mb-1 text-[12px] font-medium text-mute">{props.p.kinds.length > 1 ? 'Kinds of machine' : 'Machines'}</div>
            <div class="divide-y divide-line rounded-lg border border-line">
              <For each={props.p.kinds}>
                {(k) => (
                  <div class="px-3 py-2">
                    <div class="flex flex-wrap items-baseline gap-x-2">
                      <span class="text-[13px] font-medium">{k.label}</span>
                      <span class="text-[12px] text-faint">{kindLine(k)}</span>
                    </div>
                    <div class="text-[12.5px] text-mute">{k.about}</div>
                    <div class="text-[12.5px] text-mute">Asleep: {k.sleeps}</div>
                  </div>
                )}
              </For>
            </div>
          </div>
        </div>
      </Show>
    </div>
  )
}

function Providers() {
  const list = () => providers().filter((p) => p.creates_machines)
  return (
    <Section title="Cloud providers" hint="Where corra creates cloud machines. Projects pick a provider and size on their page.">
      <Card>
        <For each={list()}>{(p) => <ProviderRow p={p} />}</For>
      </Card>
    </Section>
  )
}

function Tools() {
  return (
    <Section title="GitHub & hub tools" hint="Tools the hub runs on behalf of agents.">
      <Card>
        <For each={tools()}>
          {(t) => (
            <div class="px-3.5 py-3">
              <div class="flex flex-wrap items-center gap-x-2.5 gap-y-0.5">
                <span class="text-[13.5px] font-medium">{t.label}</span>
                <Status ok={t.ready} text={t.ready ? 'Ready' : (t.missing ?? 'Not ready')} />
              </div>
              <div class="mt-0.5 text-[12.5px] text-mute">{t.about}</div>
              <Show when={t.name === 'github'}>
                <div class="mt-1 border-t border-line">
                  <KeyRow
                    inset
                    name="github.token"
                    label="GitHub token"
                    hint="Fine-grained, for your repos: Contents, Pull requests and Issues read and write, Actions and Checks read, no Workflows."
                  />
                </div>
              </Show>
            </div>
          )}
        </For>
      </Card>
    </Section>
  )
}

function EnvVars() {
  const [names, { refetch }] = createResource(() => call<string[]>({ op: 'list_env' }))
  const [editing, setEditing] = createSignal<string | null>(null)
  const [name, setName] = createSignal('')
  const [value, setValue] = createSignal('')
  const [error, setError] = createSignal('')
  const save = async (n: string) => {
    setError('')
    try {
      await call({ op: 'set_env', name: n.trim(), value: value() })
      setValue('')
      setName('')
      setEditing(null)
      await refetch()
    } catch (e) {
      setError((e as Error).message)
    }
  }
  return (
    <Section
      title="Shared env vars"
      hint="Set once and shared: a project gets the ones its setup names, or that you add to it. Kept encrypted, never shown again."
    >
      <Card>
        <For each={names() ?? []} fallback={<p class="px-3.5 py-3 text-[13px] text-mute">None yet.</p>}>
          {(n) => (
            <div class="px-3.5 py-2.5">
              <div class="flex items-center gap-2">
                <span class="min-w-0 flex-1 truncate font-mono text-[12.5px]">{n}</span>
                <InlineConfirm
                  label="Remove"
                  confirmLabel="Remove"
                  danger
                  onConfirm={() => call({ op: 'delete_env', name: n }).then(() => refetch())}
                />
                <Btn small onClick={() => (setEditing(editing() === n ? null : n), setValue(''))}>
                  Replace
                </Btn>
              </div>
              <Show when={editing() === n}>
                <div class="mt-2 flex gap-2">
                  <input
                    class={`${inputClass} flex-1`}
                    type="password"
                    autocomplete="off"
                    autofocus
                    placeholder="Its new value"
                    value={value()}
                    onInput={(e) => setValue(e.currentTarget.value)}
                    onKeyDown={(e) => e.key === 'Enter' && value() && void save(n)}
                  />
                  <Btn kind="primary" disabled={!value()} onClick={() => void save(n)}>
                    Save
                  </Btn>
                </div>
              </Show>
            </div>
          )}
        </For>
      </Card>
      <div class="mt-2 flex gap-2">
        <input
          class={`${inputClass} w-44 font-mono text-[12.5px]`}
          placeholder="NAME"
          spellcheck={false}
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <input
          class={`${inputClass} min-w-0 flex-1`}
          type="password"
          autocomplete="off"
          placeholder="value"
          value={editing() ? '' : value()}
          onInput={(e) => (setEditing(null), setValue(e.currentTarget.value))}
          onKeyDown={(e) => e.key === 'Enter' && name().trim() && value() && void save(name())}
        />
        <Btn disabled={!name().trim() || !value()} onClick={() => void save(name())}>
          Add
        </Btn>
      </div>
      <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
    </Section>
  )
}

function MachineSetup() {
  const [saved, setSaved] = createSignal(false)
  const [error, setError] = createSignal('')
  const value = () => String(state.settings.setup_repo ?? '')
  const save = async (r: string) => {
    setError('')
    try {
      await updateSettings({ setup_repo: r })
      setSaved(true)
    } catch (e) {
      setError((e as Error).message)
    }
  }
  return (
    <Section title="Setup repo" hint="Corra reads its corra/ folder: shared/ for every machine and setups/<name>/ for each project setup.">
      <RepoPicker value={value()} placeholder="Pick your setup repo" onPick={(r) => void save(r)} />
      <Show when={saved()}>
        <p class="mt-2 text-[12.5px] text-mute">Saved. Machines pick it up on next wake or Re-run setup.</p>
      </Show>
      <Show when={error()}>
        <p class="mt-2 text-[12.5px] text-bad">{error()}</p>
      </Show>
    </Section>
  )
}
