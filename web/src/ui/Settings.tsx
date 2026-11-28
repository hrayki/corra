import { For, Show, createMemo, createSignal } from 'solid-js'
import ArrowLeft from 'lucide-solid/icons/arrow-left'
import { state } from '../lib/store'
import { AppTab } from './settings/AppTab'
import { SecretsTab } from './settings/SecretsTab'
import { MachinesTab } from './settings/MachinesTab'

const LEGACY_TAB: Record<string, string> = {
  general: 'app',
  devices: 'app',
  keys: 'secrets',
  providers: 'secrets',
  tools: 'secrets',
  env: 'secrets',
  setup: 'secrets',
  machines: 'machines',
  data: 'machines',
}

function normalizeTab(saved: string | null): string {
  if (!saved) return 'app'
  return LEGACY_TAB[saved] ?? (saved === 'app' || saved === 'secrets' || saved === 'machines' ? saved : 'app')
}

export function Settings() {
  const hubUp = () => state.conn === 'open'
  const pages = createMemo(() => {
    const all: { key: string; label: string; show: boolean }[] = [
      { key: 'app', label: 'App', show: true },
      { key: 'secrets', label: 'Secrets & Setup', show: hubUp() },
      { key: 'machines', label: 'Machines & Data', show: true },
    ]
    return all.filter((p) => p.show)
  })
  const [page, setPageRaw] = createSignal(normalizeTab(localStorage.getItem('corra_settings_page')))
  const setPage = (k: string) => {
    setPageRaw(k)
    localStorage.setItem('corra_settings_page', k)
  }
  const on = () => (pages().some((p) => p.key === page()) ? page() : 'app')
  return (
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto flex max-w-4xl gap-10 px-4 pt-4 pb-16 sm:px-6 md:pt-10">
        <nav class="sticky top-10 hidden w-44 shrink-0 self-start md:block" aria-label="Settings">
          <h1 class="mb-4 px-2.5 text-[20px] font-semibold tracking-tight">Settings</h1>
          <For each={pages()}>
            {(p) => (
              <button
                class="flex h-8 w-full items-center rounded-lg px-2.5 text-left text-[13px]"
                classList={{
                  'bg-ink/[0.06] font-medium text-ink': on() === p.key,
                  'text-mute hover:bg-ink/[0.04] hover:text-ink': on() !== p.key,
                }}
                aria-current={on() === p.key ? 'page' : undefined}
                onClick={() => setPage(p.key)}
              >
                {p.label}
              </button>
            )}
          </For>
        </nav>
        <div class="min-w-0 flex-1 md:max-w-2xl md:pt-[46px]">
          <div class="mb-4 flex items-center gap-2 md:hidden">
            <a href="#/" aria-label="Back" class="-ml-1 flex size-9 items-center justify-center rounded-lg text-mute hover:bg-ink/5">
              <ArrowLeft size={18} />
            </a>
            <h1 class="text-[20px] font-semibold tracking-tight">Settings</h1>
          </div>
          <div class="-mx-4 mb-6 flex gap-1 overflow-x-auto px-4 md:hidden">
            <For each={pages()}>
              {(p) => (
                <button
                  class="h-9 shrink-0 rounded-lg px-3 text-[13px] whitespace-nowrap"
                  classList={{ 'bg-ink/[0.07] font-medium text-ink': on() === p.key, 'text-mute': on() !== p.key }}
                  onClick={() => setPage(p.key)}
                >
                  {p.label}
                </button>
              )}
            </For>
          </div>
          <div class="space-y-10">
            <Show when={on() === 'app'}>
              <AppTab />
            </Show>
            <Show when={on() === 'secrets'}>
              <SecretsTab />
            </Show>
            <Show when={on() === 'machines'}>
              <MachinesTab />
            </Show>
          </div>
        </div>
      </div>
    </div>
  )
}
