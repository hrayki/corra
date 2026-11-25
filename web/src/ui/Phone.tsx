import { Match, Show, Switch, createEffect, createMemo, createResource, createSignal, on, onCleanup } from 'solid-js'
import ChevronLeft from 'lucide-solid/icons/chevron-left'
import Circle from 'lucide-solid/icons/circle'
import Power from 'lucide-solid/icons/power'
import Smartphone from 'lucide-solid/icons/smartphone'
import Square from 'lucide-solid/icons/square'
import { b64bytes, callOn, listenPhone, state } from '../lib/store'
import { Btn, IconBtn } from './bits'
import type { PhoneInput } from '../proto/PhoneInput'

const KEYS: Record<string, string> = {
  Enter: 'enter',
  Backspace: 'backspace',
  Tab: 'tab',
  Escape: 'back',
  Delete: 'delete',
  ArrowUp: 'up',
  ArrowDown: 'down',
  ArrowLeft: 'left',
  ArrowRight: 'right',
}

const canPlay = async () =>
  typeof VideoDecoder !== 'undefined' &&
  (await VideoDecoder.isConfigSupported({ codec: 'avc1.42e01f' })
    .then((r) => !!r.supported)
    .catch(() => false))

function codecOf(config: Uint8Array) {
  for (let i = 0; i + 4 < config.length; i++) {
    if (config[i] === 0 && config[i + 1] === 0 && config[i + 2] === 1 && (config[i + 3] & 0x1f) === 7) {
      return 'avc1.' + [config[i + 4], config[i + 5], config[i + 6]].map((b) => b.toString(16).padStart(2, '0')).join('')
    }
  }
  return 'avc1.42e01f'
}

export function PhonePanel() {
  let canvas!: HTMLCanvasElement
  let box!: HTMLDivElement
  const machineCount = () => Object.keys(state.machines).length
  const [info, { refetch: refetchInfo }] = createResource(machineCount, () =>
    callOn<{ host: unknown; machine?: string }>('hub', { op: 'phone_host' }).catch(() => ({ host: null, machine: undefined })),
  )
  const phone = createMemo(() => {
    const id = info()?.machine
    const m = id ? state.machines[id] : undefined
    return m && !['destroyed', 'destroying'].includes(m.status) ? m : undefined
  })
  const status = () => phone()?.status
  const [error, setError] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const [shown, setShown] = createSignal(false)
  const [size, setSize] = createSignal<{ w: number; h: number } | null>(null)
  const [still, setStill] = createSignal('')
  const [stopping, setStopping] = createSignal(false)
  const [live, setLive] = createSignal(true)
  let failures = 0
  let stop: (() => void) | undefined
  let decoder: VideoDecoder | undefined
  let poll: number | undefined
  let opened = false

  const run = async (fn: () => Promise<unknown>) => {
    setError('')
    setBusy(true)
    try {
      await fn()
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }
  const send = (input: PhoneInput) => void callOn('hub', { op: 'phone_input', input }).catch(() => {})

  const close = () => {
    stop?.()
    stop = undefined
    clearInterval(poll)
    if (decoder && decoder.state !== 'closed') decoder.close()
    decoder = undefined
    if (opened) void callOn('hub', { op: 'phone_close' }).catch(() => {})
    opened = false
    setShown(false)
  }

  const open = async () => {
    close()
    setError('')
    if (!(await canPlay()) || failures > 2) setLive(false)
    if (!live()) {
      const shot = async () => {
        try {
          const r = await callOn<{ image: { data: string }; width: number; height: number }>('hub', { op: 'phone', action: { do: 'shot' } })
          setStill(`data:image/jpeg;base64,${r.image.data}`)
          setSize({ w: r.width, h: r.height })
          setShown(true)
        } catch (e) {
          setError((e as Error).message)
        }
      }
      await shot()
      poll = window.setInterval(() => void shot(), 1500)
      return
    }
    let config: Uint8Array | undefined
    let n = 0
    let waiting = true
    const ctx = canvas.getContext('2d')!
    const fresh = () => {
      waiting = true
      if (decoder && decoder.state !== 'closed') decoder.close()
      decoder = new VideoDecoder({
        output: (f) => {
          if (canvas.width !== f.displayWidth || canvas.height !== f.displayHeight) {
            canvas.width = f.displayWidth
            canvas.height = f.displayHeight
          }
          ctx.drawImage(f, 0, 0)
          f.close()
          setShown(true)
        },
        error: () => {
          failures++
          if (failures > 2) void open()
          else void callOn('hub', { op: 'phone_open' }).catch(() => {})
        },
      })
    }
    fresh()
    stop = listenPhone((m) => {
      if (m.t === 'phone_video') {
        setSize({ w: m.width, h: m.height })
        return
      }
      const data = b64bytes(m.data)
      if (m.config) {
        config = data
        fresh()
        decoder!.configure({ codec: codecOf(data), optimizeForLatency: true })
        return
      }
      if (!config || !decoder || decoder.state !== 'configured' || (waiting && !m.key)) return
      waiting = false
      let chunk = data
      if (m.key) {
        chunk = new Uint8Array(config.length + data.length)
        chunk.set(config)
        chunk.set(data, config.length)
      }
      decoder.decode(new EncodedVideoChunk({ type: m.key ? 'key' : 'delta', timestamp: n++ * 33333, data: chunk }))
    })
    opened = true
    try {
      await callOn('hub', { op: 'phone_open' })
    } catch (e) {
      setError((e as Error).message)
    }
  }

  createEffect(
    on(
      () => [status(), phone()?.id] as const,
      ([s]) => {
        if (s === 'connected') void open()
        else close()
      },
    ),
  )
  onCleanup(close)

  const point = (e: MouseEvent) => {
    const el = live() ? canvas : (box.querySelector('img') as HTMLElement)
    const r = el.getBoundingClientRect()
    const s = size()
    if (!s || !r.width) return null
    return {
      x: Math.round(((e.clientX - r.left) / r.width) * s.w),
      y: Math.round(((e.clientY - r.top) / r.height) * s.h),
      width: s.w,
      height: s.h,
    }
  }
  let down: { x: number; y: number } | null = null
  const onDown = (e: PointerEvent) => {
    const p = point(e)
    if (!p) return
    box.focus()
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    down = p
    if (live()) send({ kind: 'touch', action: 'down', ...p })
  }
  const onMove = (e: PointerEvent) => {
    const p = point(e)
    if (down && p && live()) send({ kind: 'touch', action: 'move', ...p })
  }
  const onUp = (e: PointerEvent) => {
    const p = point(e)
    if (!down || !p) return
    const from = down
    down = null
    if (live()) return send({ kind: 'touch', action: 'up', ...p })
    const moved = Math.hypot(p.x - from.x, p.y - from.y) > 12
    void callOn('hub', {
      op: 'phone',
      action: moved ? { do: 'swipe', x1: from.x, y1: from.y, x2: p.x, y2: p.y } : { do: 'tap', x: p.x, y: p.y },
    }).catch(() => {})
  }
  const onWheel = (e: WheelEvent) => {
    const p = point(e)
    if (!p || !live()) return
    e.preventDefault()
    send({ kind: 'scroll', ...p, dx: -Math.sign(e.deltaX), dy: -Math.sign(e.deltaY) })
  }
  const onKey = (e: KeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return
    const named = KEYS[e.key]
    if (!named && e.key.length !== 1) return
    e.preventDefault()
    if (live()) return send(named ? { kind: 'key', key: named } : { kind: 'text', text: e.key })
    void callOn('hub', { op: 'phone', action: named ? { do: 'key', key: named } : { do: 'text', text: e.key } }).catch(() => {})
  }
  const press = (key: string) =>
    live() ? send({ kind: 'key', key }) : void callOn('hub', { op: 'phone', action: { do: 'key', key } }).catch(() => {})

  const Notice = (p: { title: string; text?: string; children?: any }) => (
    <div class="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
      <Smartphone size={28} class="text-faint" stroke-width={1.5} />
      <div class="text-[14px] font-medium text-balance">{p.title}</div>
      <Show when={p.text}>
        <p class="max-w-[300px] text-[13px] text-mute text-balance">{p.text}</p>
      </Show>
      {p.children}
      <Show when={error()}>
        <p class="max-w-[320px] text-[12.5px] text-bad">{error()}</p>
      </Show>
    </div>
  )

  return (
    <Switch>
      <Match when={state.conn.hub === 'none'}>
        <Notice title="The test phone runs through a hub" text="Connect a hub in Settings to start an Android phone from here." />
      </Match>
      <Match when={info() && !info()!.host}>
        <Notice
          title="The phone is off"
          text="Pick where it runs in Settings, under Tools. It needs a machine with KVM, like a DigitalOcean droplet or an Agent37 VM."
        />
      </Match>
      <Match when={!phone() || status() === 'failed' || status() === 'asleep'}>
        <Notice
          title={status() === 'failed' ? "The phone didn't start" : 'No phone is running'}
          text="An Android phone with Google Play, for you and your agents. It runs in the cloud while you use it, and its apps and data go when you stop it."
        >
          <Btn kind="primary" disabled={busy()} onClick={() => run(() => callOn('hub', { op: 'phone_up' }).then(() => refetchInfo()))}>
            {status() === 'failed' ? 'Try again' : 'Start the phone'}
          </Btn>
        </Notice>
      </Match>
      <Match when={status() !== 'connected'}>
        <Notice
          title={status() === 'offline' ? 'The phone lost its connection' : 'Starting the phone'}
          text="About two minutes. The first start also makes the phone's image, which takes about fifteen."
        >
          <Btn kind="ghost" small disabled={busy()} onClick={() => run(() => callOn('hub', { op: 'phone_down' }))}>
            Cancel
          </Btn>
        </Notice>
      </Match>
      <Match when={true}>
        <div class="flex h-full flex-col">
          <div
            ref={box}
            tabindex={0}
            class="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-panel p-4 outline-none"
            onKeyDown={onKey}
          >
            <Show
              when={live()}
              fallback={
                <img
                  src={still()}
                  alt=""
                  draggable={false}
                  class="max-h-full max-w-full touch-none rounded-2xl select-none"
                  classList={{ invisible: !shown() }}
                  onPointerDown={onDown}
                  onPointerUp={onUp}
                />
              }
            >
              <canvas
                ref={canvas}
                class="max-h-full max-w-full touch-none rounded-2xl"
                classList={{ invisible: !shown() }}
                onPointerDown={onDown}
                onPointerMove={onMove}
                onPointerUp={onUp}
                onPointerCancel={onUp}
                onWheel={onWheel}
                onContextMenu={(e) => e.preventDefault()}
              />
            </Show>
            <Show when={!shown()}>
              <div class="absolute inset-0 flex items-center justify-center p-6 text-center text-[13px] text-mute">
                {error() || 'Waking the phone'}
              </div>
            </Show>
          </div>
          <div class="flex shrink-0 items-center justify-center gap-1 border-t border-line px-2 py-1.5">
            <IconBtn label="Back" onClick={() => press('back')}>
              <ChevronLeft size={17} />
            </IconBtn>
            <IconBtn label="Home" onClick={() => press('home')}>
              <Circle size={13} />
            </IconBtn>
            <IconBtn label="Recent apps" onClick={() => press('recents')}>
              <Square size={12} />
            </IconBtn>
            <div class="mx-1 h-4 w-px bg-line" />
            <Show
              when={stopping()}
              fallback={
                <IconBtn
                  label="Stop the phone"
                  onClick={() => {
                    setStopping(true)
                    setTimeout(() => setStopping(false), 4000)
                  }}
                >
                  <Power size={14} />
                </IconBtn>
              }
            >
              <Btn kind="danger" small disabled={busy()} onClick={() => run(() => callOn('hub', { op: 'phone_down' }))}>
                Destroy the phone
              </Btn>
            </Show>
          </div>
        </div>
      </Match>
    </Switch>
  )
}
