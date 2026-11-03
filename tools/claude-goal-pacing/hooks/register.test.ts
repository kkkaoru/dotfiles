// This TypeScript file is executed with Bun.
import { expect, test } from 'vitest'

import { register } from './register'

interface Hooked {
  readonly event: string
  readonly hook: Function
}

interface Fake {
  readonly hooks: Hooked[]
  readonly toasts: unknown[]
  checks: number
}

function setup(): Fake {
  const fake: Fake = { hooks: [], toasts: [], checks: 0 }
  Reflect.apply(register, undefined, [(event: string, hook: Function) => fake.hooks.push({ event, hook }), {}])
  return fake
}

const hookFor = (fake: Fake, event: string): Function => fake.hooks.find(hooked => hooked.event === event)?.hook ?? (() => undefined)

const engine = (fake: Fake): unknown => ({ ui: { toast: (text: unknown) => fake.toasts.push(text) } })

const stop = (fake: Fake, active: boolean): unknown =>
  Reflect.apply(hookFor(fake, 'classic.Stop'), undefined, [
    engine(fake),
    { stop_hook_active: active },
    async () => {
      fake.checks += 1
      return { block: 'Goal not yet met' }
    },
  ])

const toolCall = (fake: Fake, agentId?: string): unknown =>
  Reflect.apply(hookFor(fake, 'tool.call'), undefined, [engine(fake), { tool: 'Bash', agentId }, () => 'ran'])

test('an idle goal continuation ends the turn without checking again', async () => {
  const fake = setup()
  expect(await stop(fake, false)).toStrictEqual({ block: 'Goal not yet met' })
  expect(await stop(fake, true)).toStrictEqual({})
  expect(fake.checks).toBe(1)
  expect(fake.toasts).toHaveLength(1)
})

test('a continuation that used a tool is checked again', async () => {
  const fake = setup()
  await stop(fake, false)
  expect(toolCall(fake)).toBe('ran')
  expect(await stop(fake, true)).toStrictEqual({ block: 'Goal not yet met' })
  expect(await stop(fake, true)).toStrictEqual({})
  expect(fake.checks).toBe(2)
})

test('subagent tool calls do not count as work of the main turn', async () => {
  const fake = setup()
  toolCall(fake, 'agent-1')
  expect(await stop(fake, true)).toStrictEqual({})
  expect(fake.checks).toBe(0)
})
