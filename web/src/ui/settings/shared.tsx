import { Show, createResource, createRoot, createSignal, type JSX } from 'solid-js'
import { call, state } from '../../lib/store'
import { refreshCatalog } from '../../lib/catalog'
import { Btn, inputClass } from '../bits'

export function Section(props: { title: string; hint?: string; children: JSX.Element }) {
  return (
    <section>
      <h2 class="text-[14px] font-semibold">{props.title}</h2>
      <Show when={props.hint}>
        <p class="mt-0.5 text-[13px] text-mute">{props.hint}</p>
      </Show>
      <div class="mt-3">{props.children}</div>
    </section>
  )
}

export function Card(props: { children: JSX.Element }) {
  return <div class="divide-y divide-line overflow-hidden rounded-xl border border-line">{props.children}</div>
}

export function Status(props: { ok: boolean; text: string }) {
  return (
    <span class="inline-flex items-center gap-1.5 text-[12px]" classList={{ 'text-ok': props.ok, 'text-mute': !props.ok }}>
      <span class="size-1.5 rounded-full" classList={{ 'bg-ok': props.ok, 'bg-faint': !props.ok }} />
      {props.text}
    </span>
  )
}

export function InlineConfirm(props: {
  label: string
  confirmLabel?: string
  danger?: boolean
  disabled?: boolean
  onConfirm: () => Promise<unknown> | void
}) {
  const [asking, setAsking] = createSignal(false)
  const [busy, setBusy] = createSignal(false)
  const go = async () => {
    if (busy()) return
    setBusy(true)
    try {
      await props.onConfirm()
      setAsking(false)
    } finally {
      setBusy(false)
    }
  }
  return (
    <Show
      when={asking()}
      fallback={
        <Btn small kind="ghost" disabled={props.disabled} onClick={() => setAsking(true)}>
          {props.label}
        </Btn>
      }
    >
      <span class="inline-flex items-center gap-1">
        <Btn small kind={props.danger ? 'danger' : 'primary'} disabled={busy()} onClick={() => void go()}>
          {busy() ? '…' : (props.confirmLabel ?? props.label)}
        </Btn>
        <Btn small kind="ghost" disabled={busy()} onClick={() => setAsking(false)}>
          Cancel
        </Btn>
      </span>
    </Show>
  )
}

const [secretNames, secretsRes] = createRoot(() =>
  createResource(
    () => state.conn === 'open',
    (up) => (up ? call<string[]>({ op: 'list_secrets' }) : Promise.resolve([] as string[])),
  ),
)

export { secretNames }
export const hasSecret = (n: string) => !!secretNames()?.includes(n)
export const refreshKeys = () => {
  void secretsRes.refetch()
  refreshCatalog()
}

export function KeyRow(props: { name: string; label: string; hint?: string; inset?: boolean }) {
  const [open, setOpen] = createSignal(false)
  const [value, setValue] = createSignal('')
  const [error, setError] = createSignal('')
  const save = async () => {
    setError('')
    try {
      await call({ op: 'set_secret', name: props.name, value: value() })
      setValue('')
      setOpen(false)
      refreshKeys()
    } catch (e) {
      setError((e as Error).message)
    }
  }
  const remove = async () => {
    setError('')
    try {
      await call({ op: 'delete_secret', name: props.name })
      refreshKeys()
    } catch (e) {
      setError((e as Error).message)
    }
  }
  return (
    <div classList={{ 'px-3.5 py-3': !props.inset, 'py-2': props.inset }}>
      <div class="flex items-start gap-2">
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-baseline gap-x-2">
            <span class="text-[13px] font-medium">{props.label}</span>
            <span class="text-[12px]" classList={{ 'text-ok': hasSecret(props.name), 'text-faint': !hasSecret(props.name) }}>
              {hasSecret(props.name) ? 'Saved' : 'Not set'}
            </span>
          </div>
          <Show when={props.hint}>
            <div class="mt-0.5 text-[12.5px] text-mute">{props.hint}</div>
          </Show>
        </div>
        <Show when={hasSecret(props.name)}>
          <InlineConfirm label="Remove" confirmLabel="Remove" danger onConfirm={remove} />
        </Show>
        <Btn small onClick={() => (setOpen(!open()), setValue(''))}>
          {hasSecret(props.name) ? 'Replace' : 'Add'}
        </Btn>
      </div>
      <Show when={open()}>
        <div class="mt-2 flex gap-2">
          <input
            class={`${inputClass} flex-1`}
            type="password"
            autocomplete="off"
            autofocus
            placeholder="Paste the value"
            value={value()}
            onInput={(e) => setValue(e.currentTarget.value)}
            onKeyDown={(e) => e.key === 'Enter' && value() && void save()}
          />
          <Btn kind="primary" disabled={!value()} onClick={() => void save()}>
            Save
          </Btn>
        </div>
      </Show>
      <Show when={error()}>
        <p class="mt-1.5 text-[12.5px] text-bad">{error()}</p>
      </Show>
    </div>
  )
}
