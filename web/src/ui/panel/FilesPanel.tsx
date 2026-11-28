import { For, Show, createEffect, createResource, createSignal, on } from 'solid-js'
import FileIcon from 'lucide-solid/icons/file'
import FolderIcon from 'lucide-solid/icons/folder'
import { call, state } from '../../lib/store'
import { panel } from '../../lib/panel'
import { inside } from '../../lib/filelinks'
import { providerOf } from '../../lib/catalog'
import { CodeBlock } from '../tools'
import { Markdown } from '../Markdown'
import { WakeNotice, machineOf, projectOf } from '../Panel'

type Entry = { name: string; dir: boolean; size: number }

export function FilesPanel(props: { thread: string }) {
  const [open, setOpen] = createSignal<{ path: string; line?: number } | null>(null)
  const connected = () => machineOf(props.thread)?.status === 'connected'
  const makes = () => !!providerOf(projectOf(props.thread)?.provider)?.creates_machines
  createEffect(
    on(panel, (p) => {
      if (p?.kind === 'files' && p.path) setOpen({ path: inside(state.threads[props.thread]?.cwd ?? '', p.path), line: p.line })
    }),
  )
  return (
    <Show when={!makes() || connected()} fallback={<WakeNotice thread={props.thread} />}>
      <div class="flex h-full max-sm:flex-col">
        <div class="w-56 shrink-0 overflow-auto border-line py-1.5 max-sm:max-h-48 max-sm:w-full max-sm:border-b sm:border-r">
          <Dir thread={props.thread} path="" depth={0} onOpen={(path) => setOpen({ path })} selected={open()?.path ?? null} />
        </div>
        <div class="min-w-0 flex-1 overflow-auto">
          <Show when={open()} fallback={<p class="p-5 text-[13px] text-mute">Pick a file to read it.</p>} keyed>
            {(o) => <FileView thread={props.thread} path={o.path} line={o.line} />}
          </Show>
        </div>
      </div>
    </Show>
  )
}

function Dir(props: { thread: string; path: string; depth: number; onOpen: (p: string) => void; selected: string | null }) {
  const [list] = createResource(
    () => props.path,
    async (p) => (await call<{ entries: Entry[] }>({ op: 'list_dir', thread: props.thread, path: p || '.' })).entries,
  )
  return (
    <Show when={!list.error} fallback={<p class="px-3 py-1 text-[12px] text-bad">{String(list.error?.message ?? '')}</p>}>
      <For each={(list() ?? []).filter((e) => e.name !== '.git')}>
        {(e) => (
          <Node
            thread={props.thread}
            e={e}
            path={props.path ? `${props.path}/${e.name}` : e.name}
            depth={props.depth}
            onOpen={props.onOpen}
            selected={props.selected}
          />
        )}
      </For>
    </Show>
  )
}

function Node(props: { thread: string; e: Entry; path: string; depth: number; onOpen: (p: string) => void; selected: string | null }) {
  const [expanded, setExpanded] = createSignal(false)
  createEffect(() => {
    if (props.e.dir && props.selected?.startsWith(props.path + '/')) setExpanded(true)
  })
  return (
    <>
      <button
        ref={(b) =>
          createEffect(() => {
            if (props.selected === props.path) queueMicrotask(() => b.scrollIntoView({ block: 'nearest' }))
          })
        }
        class="flex w-full items-center gap-1.5 py-[3px] pr-2 text-left text-[12.5px] hover:bg-ink/5"
        classList={{ 'bg-ink/[0.06] font-medium': props.selected === props.path }}
        style={{ 'padding-left': `${10 + props.depth * 12}px` }}
        onClick={() => (props.e.dir ? setExpanded(!expanded()) : props.onOpen(props.path))}
      >
        {props.e.dir ? <FolderIcon size={13} class="shrink-0 text-mute" /> : <FileIcon size={13} class="shrink-0 text-faint" />}
        <span class="truncate" classList={{ 'text-mute': props.e.name.startsWith('.') }}>
          {props.e.name}
        </span>
      </button>
      <Show when={props.e.dir && expanded()}>
        <Dir thread={props.thread} path={props.path} depth={props.depth + 1} onOpen={props.onOpen} selected={props.selected} />
      </Show>
    </>
  )
}

const IMAGE = /\.(png|jpe?g|gif|webp|svg)$/i

function FileView(props: { thread: string; path: string; line?: number }) {
  const [file] = createResource(
    () => props.path,
    (p) => call<{ text?: string; base64?: string; size: number; truncated: boolean }>({ op: 'read_file', thread: props.thread, path: p }),
  )
  const mime = () => {
    const ext = props.path.split('.').pop()?.toLowerCase()
    return ext === 'svg' ? 'image/svg+xml' : `image/${ext === 'jpg' ? 'jpeg' : ext}`
  }
  return (
    <div>
      <div class="sticky top-0 z-10 border-b border-line bg-bg px-4 py-2 font-mono text-[12px] text-mute">{props.path}</div>
      <Show when={file()} fallback={<p class="p-4 text-[13px] text-mute">{file.error ? String(file.error.message) : 'Loading'}</p>}>
        {(f) => (
          <Show
            when={IMAGE.test(props.path) && (f().base64 || f().text)}
            fallback={
              <Show
                when={typeof f().text === 'string'}
                fallback={<p class="p-4 text-[13px] text-mute">A binary file, {f().size.toLocaleString()} bytes.</p>}
              >
                <Show
                  when={/\.mdx?$/i.test(props.path)}
                  fallback={
                    <div class="[&>div]:max-h-none [&>div]:rounded-none [&>div]:border-0 [&>div]:bg-transparent">
                      <CodeBlock code={f().text!} path={props.path} line={props.line} />
                    </div>
                  }
                >
                  <div class="p-5">
                    <Markdown text={f().text!} base={props.path.split('/').slice(0, -1).join('/')} />
                  </div>
                </Show>
              </Show>
            }
          >
            <img
              class="m-4 max-w-[90%] rounded-lg border border-line"
              src={f().base64 ? `data:${mime()};base64,${f().base64}` : `data:image/svg+xml;utf8,${encodeURIComponent(f().text ?? '')}`}
            />
          </Show>
        )}
      </Show>
    </div>
  )
}
