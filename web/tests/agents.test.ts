import { test } from 'node:test'
import assert from 'node:assert/strict'
import { agentInfo } from '../src/lib/agents.ts'
import type { ItemView } from '../src/lib/timeline.ts'

const call = (input: object, status: string, output?: string, at = 1000, ended?: number): ItemView =>
  ({
    id: 'toolu_1',
    body: {
      type: 'tool_call',
      name: 'Agent',
      input,
      status,
      output: output === undefined ? null : { text: output, is_error: false, images: [] },
    },
    done: status !== 'running',
    at,
    ended,
  }) as unknown as ItemView
const said = (id: string, text: string, at: number): ItemView =>
  ({ id, parent: 'toolu_1', body: { type: 'agent_message', text }, done: true, at }) as unknown as ItemView

test('a background agent runs until Claude Code reports how it ended, and that ending is what the row says', () => {
  const launched = call({ description: 'Watch logs', run_in_background: true }, 'completed', 'Async agent launched successfully.')
  const step = said('m1', 'Reading the logs', 2000)
  assert.equal(agentInfo(launched, [step], ['toolu_1'], 60_000).state, 'running')
  const stopped = agentInfo(launched, [step, said('task:a', 'Background agent stopped: Watch logs', 9000)], [], 60_000)
  assert.equal(stopped.state, 'stopped')
  assert.equal(stopped.ended, 9000)
  const done = agentInfo(launched, [step, said('task:a', 'Background agent completed: No errors in the last hour.', 9000)], [], 60_000)
  assert.deepEqual([done.state, done.result], ['done', 'No errors in the last hour.'])
})

test('a background agent whose session went away no longer shows as working', () => {
  const launched = call({ description: 'Watch logs', run_in_background: true }, 'completed', 'Async agent launched successfully.')
  assert.equal(agentInfo(launched, [], [], 60_000).state, 'stopped')
  assert.equal(agentInfo(launched, [], [], 3_000).state, 'running', 'a just-launched agent flickered to stopped')
})

test("a finished agent's result leaves out Claude Code's note on how to message it", () => {
  const a = agentInfo(
    call(
      { description: 'Explore', subagent_type: 'Explore' },
      'completed',
      "Found it.\n\nagentId: a42 (use SendMessage with to: 'a42' to continue this agent)\n<usage>total_tokens: 4</usage>",
      1000,
      5000,
    ),
    [],
    undefined,
    9000,
  )
  assert.deepEqual([a.state, a.result, a.kind, a.ended], ['done', 'Found it.', 'Explore', 5000])
})
