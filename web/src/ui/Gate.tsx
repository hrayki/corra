import { Match, Show, Switch, createSignal, onMount } from 'solid-js'
import { pairDevice, signInWithKey } from '../lib/auth'
import { Btn, inputClass } from './bits'

function codeFromHash() {
  return location.hash.match(/^#\/pair\/([A-Za-z0-9-]+)/)?.[1] ?? ''
}

export function Gate(props: { onDone: () => void }) {
  const [mode, setMode] = createSignal<'key' | 'pair'>(codeFromHash() ? 'pair' : 'key')
  const [key, setKey] = createSignal('')
  const [code, setCode] = createSignal(codeFromHash())
  const [busy, setBusy] = createSignal(false)
  const [error, setError] = createSignal('')
  const run = (f: () => Promise<unknown>) => async () => {
    setBusy(true)
    setError('')
    try {
      await f()
      if (location.hash.startsWith('#/pair/')) location.hash = '#/'
      props.onDone()
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }
  const pair = run(() => pairDevice(code(), navigator.userAgent.slice(0, 60)))
  const signIn = run(() => signInWithKey(key()))
  onMount(() => {
    if (codeFromHash()) void pair()
  })
  return (
    <div class="flex h-full items-center justify-center p-6">
      <div class="w-full max-w-sm">
        <div class="mb-6 text-[15px] font-semibold tracking-tight">corra</div>
        <Switch>
          <Match when={mode() === 'key'}>
            <h1 class="text-[20px] font-semibold tracking-tight">Sign in</h1>
            <p class="mt-1.5 text-[13.5px] text-mute">With the hub's key: CORRA_ACCESS_KEY, or the file access-key in its data folder.</p>
            <form
              class="mt-5"
              onSubmit={(e) => {
                e.preventDefault()
                if (key().trim()) void signIn()
              }}
            >
              <input type="text" name="username" autocomplete="username" value="corra" class="hidden" readOnly />
              <input
                type="password"
                name="password"
                autocomplete="current-password"
                class={`${inputClass} h-10 w-full font-mono`}
                placeholder="The hub's key"
                autofocus
                value={key()}
                onInput={(e) => setKey(e.currentTarget.value)}
              />
              <div class="mt-3 flex flex-wrap items-center gap-3">
                <Btn kind="primary" type="submit" disabled={busy() || !key().trim()}>
                  Sign in
                </Btn>
                <button
                  type="button"
                  class="text-[13px] text-mute underline decoration-line underline-offset-4 hover:text-ink"
                  onClick={() => setMode('pair')}
                >
                  Use a pairing code
                </button>
              </div>
            </form>
          </Match>
          <Match when={mode() === 'pair'}>
            <h1 class="text-[20px] font-semibold tracking-tight">Pair this device</h1>
            <p class="mt-1.5 text-[13.5px] text-mute">
              On a device that's signed in, open Settings and make a pairing code. Scan its QR code, or type the code here.
            </p>
            <input
              class={`${inputClass} mt-5 h-10 w-full font-mono tracking-[0.3em] uppercase`}
              placeholder="ABCD2345"
              autocapitalize="characters"
              spellcheck={false}
              value={code()}
              onInput={(e) => setCode(e.currentTarget.value)}
              onKeyDown={(e) => e.key === 'Enter' && code().trim().length >= 8 && void pair()}
            />
            <div class="mt-3 flex flex-wrap items-center gap-3">
              <Btn kind="primary" disabled={busy() || code().trim().length < 8} onClick={pair}>
                Pair
              </Btn>
              <button
                class="text-[13px] text-mute underline decoration-line underline-offset-4 hover:text-ink"
                onClick={() => setMode('key')}
              >
                Use the hub's key
              </button>
            </div>
          </Match>
        </Switch>
        <Show when={error()}>
          <p class="mt-3 text-[13px] text-bad">{error()}</p>
        </Show>
      </div>
    </div>
  )
}
