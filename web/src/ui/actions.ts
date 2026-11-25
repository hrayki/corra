import { call, callOn, state, type ThreadX } from '../lib/store'
import { markSeen } from '../lib/seen'
import type { MenuItem } from './bits'
import { paired, providerLabel, providerOf } from '../lib/catalog'

export function threadActions(t: ThreadX, onError: (message: string) => void): MenuItem[] {
  const run = (p: Promise<unknown>) => void p.catch((e) => onError((e as Error).message))
  const set = (body: Record<string, unknown>) => run(call({ op: 'update_thread', thread: t.id, ...body } as never))
  const items: MenuItem[] = [
    { label: t.pinned_at ? 'Unpin' : 'Pin', onSelect: () => set({ pinned: !t.pinned_at }) },
    {
      label: t.settled_at ? 'Reopen' : 'Settle',
      hint: t.settled_at ? '' : 'done for now',
      onSelect: () => set({ settled: !t.settled_at }),
    },
    {
      label: 'Rename',
      onSelect: () => {
        const title = prompt('Thread title', t.title)
        if (title?.trim()) set({ title: title.trim() })
      },
    },
    { label: 'Mark as unread', disabled: t.status !== 'idle', onSelect: () => markSeen(t.id, t.last_seq - 1) },
    {
      label: 'Fork',
      onSelect: () => run(call<{ id: string }>({ op: 'fork_thread', thread: t.id }).then((n) => (location.hash = `#/t/${n.id}`))),
    },
  ]
  if (state.conn.hub === 'open') {
    const project = state.projects[t.project]
    const machine = t.machine ? state.machines[t.machine] : undefined
    const targets: MenuItem[] = []
    const idle = t.status === 'idle' || t.status === 'error'
    const move = (to?: string) => run(callOn('hub', { op: 'move_thread', thread: t.id, machine: to }))
    if (project && providerOf(project.provider)?.creates_machines && machine && paired(machine.provider)) {
      targets.push({
        label: `A new ${providerLabel(project.provider)} machine`,
        disabled: !idle,
        onSelect: () => move(undefined),
      })
    }
    const ordered = Object.values(state.machines).sort((a, b) => Number(b.id === state.thisMachine) - Number(a.id === state.thisMachine))
    for (const m of ordered) {
      if (m.status !== 'connected' || m.id === t.machine || !paired(m.provider)) continue
      targets.push({ label: m.id === state.thisMachine ? 'This computer' : m.name, disabled: !idle, onSelect: () => move(m.id) })
    }
    if (targets.length) items.push('sep', { heading: idle ? 'Move to' : 'Move to (when the agent is idle)' }, ...targets)
  }
  items.push('sep', {
    label: 'Move to trash',
    danger: true,
    onSelect: () =>
      run(
        call({ op: 'update_thread', thread: t.id, hidden: true }).then(() => {
          if (location.hash === `#/t/${t.id}`) location.hash = '#/'
        }),
      ),
  })
  return items
}
