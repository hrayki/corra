import { For, Show, createEffect, createSignal, onCleanup } from 'solid-js'
import { Portal } from 'solid-js/web'
import X from 'lucide-solid/icons/x'
import { call } from '../lib/store'
import { closeRename, dismissToast, pushToast, renaming, toasts } from '../lib/toast'
import { Btn, inputClass } from './bits'

export function ToastHost() {
  return (
    <Show when={toasts().length > 0}>
      <Portal>
        <div
          aria-live="polite"
          class="pointer-events-none fixed right-4 bottom-4 z-50 flex max-w-sm flex-col gap-2 max-sm:right-3 max-sm:left-3"
        >
          <For each={toasts()}>
            {(t) => (
              <div
                class="pointer-events-auto flex items-start gap-2.5 rounded-xl border bg-raised px-3.5 py-2.5 text-[13px] shadow-[var(--shadow)]"
                classList={{
                  'border-bad/40 text-bad': t.tone === 'error',
                  'border-line text-ink': t.tone === 'info',
                }}
              >
                <span class="min-w-0 flex-1 leading-snug">{t.message}</span>
                <button
                  type="button"
                  aria-label="Dismiss"
                  class="-mt-0.5 -mr-1 flex size-6 shrink-0 items-center justify-center rounded-md text-faint hover:bg-ink/5 hover:text-ink"
                  onClick={() => dismissToast(t.id)}
                >
                  <X size={13} />
                </button>
              </div>
            )}
          </For>
        </div>
      </Portal>
    </Show>
  )
}

export function RenameDialog() {
  const [title, setTitle] = createSignal('')
  const [saving, setSaving] = createSignal(false)
  let inputRef: HTMLInputElement | undefined

  createEffect(() => {
    const t = renaming()
    if (!t) return
    setTitle(t.title)
    setSaving(false)
    queueMicrotask(() => {
      inputRef?.focus()
      inputRef?.select()
    })
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') closeRename()
    }
    window.addEventListener('keydown', onKey)
    onCleanup(() => window.removeEventListener('keydown', onKey))
  })

  const save = async () => {
    const t = renaming()
    const next = title().trim()
    if (!t || !next || saving()) return
    setSaving(true)
    try {
      await call({ op: 'update_thread', thread: t.id, title: next })
      closeRename()
    } catch (e) {
      pushToast((e as Error).message)
      setSaving(false)
    }
  }

  return (
    <Show when={renaming()}>
      <Portal>
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-ink/25 p-4 backdrop-blur-[1px]" onClick={closeRename}>
          <form
            role="dialog"
            aria-label="Rename thread"
            class="w-full max-w-sm rounded-2xl border border-line bg-raised p-4 shadow-[var(--shadow)]"
            onClick={(e) => e.stopPropagation()}
            onSubmit={(e) => {
              e.preventDefault()
              void save()
            }}
          >
            <div class="text-[14px] font-semibold">Rename thread</div>
            <input
              ref={inputRef}
              class={`${inputClass} mt-3 w-full`}
              placeholder="Thread title"
              value={title()}
              onInput={(e) => setTitle(e.currentTarget.value)}
            />
            <div class="mt-4 flex justify-end gap-2">
              <Btn kind="ghost" disabled={saving()} onClick={closeRename}>
                Cancel
              </Btn>
              <Btn kind="primary" type="submit" disabled={!title().trim() || saving()}>
                {saving() ? 'Saving…' : 'Save'}
              </Btn>
            </div>
          </form>
        </div>
      </Portal>
    </Show>
  )
}
