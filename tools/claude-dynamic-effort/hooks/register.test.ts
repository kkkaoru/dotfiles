// This TypeScript file is executed with Bun through vitest.
/// <reference types="node" />
import { getHeapStatistics, setFlagsFromString } from 'node:v8'
import { runInNewContext } from 'node:vm'
import { expect, test } from 'vitest'

import { register } from './register'

interface Hooked {
  readonly event: unknown
  readonly matcher: unknown
  readonly hook: unknown
}

interface Context {
  readonly tokens?: number
  readonly window: number
}

interface Fake {
  readonly hooks: Hooked[]
  readonly sent: unknown[]
  readonly efforts: string[]
  readonly statuses: unknown[]
  readonly inputs: unknown[]
  usageCalls: number
  toolCalls: number
  context: Context
  tokens: number | undefined
  threshold: number | undefined
  rejectRuns: boolean
  rejectUsage: boolean
  estimate: number | undefined
  turns: number
  transcript: unknown[]
  transcriptGate: Promise<void>
  messageReads: number
  compactions: number
  sessionId: string
  readonly store: Map<string, unknown>
  rejectStore: boolean
}

interface Step {
  readonly turnId: string
  readonly index: number
  readonly effort?: string | number
  readonly agentId?: string
  readonly model?: string
}

const PERSON = { kind: 'composer' } satisfies { kind: string }
const OWN = { kind: 'plugin', name: 'dynamic-effort' } satisfies { kind: string; name: string }
const SUMMARY = [{ role: 'user', text: 'summary', toolUses: [] }] satisfies unknown[]

function setup(options?: Readonly<Record<string, string>>): Fake {
  const fake: Fake = { hooks: [], sent: [], efforts: [], statuses: [], inputs: [], usageCalls: 0, toolCalls: 0, context: { window: 100 }, tokens: undefined, threshold: undefined, rejectRuns: false, rejectUsage: false, estimate: undefined, turns: 0, transcript: [], transcriptGate: Promise.resolve(), messageReads: 0, compactions: 0, sessionId: 'session-1', store: new Map(), rejectStore: false }
  const on = (event: unknown, matcher: unknown, hook?: unknown): { readonly catch: () => void } => {
    fake.hooks.push(hook === undefined ? { event, matcher: undefined, hook: matcher } : { event, matcher, hook })
    return { catch: () => undefined }
  }
  register(on, options ?? {})
  return fake
}

function engine(fake: Fake): unknown {
  return {
    command: {
      register: async () => ({ command: 'dynamic-effort' }),
      run: async ({ args }: { readonly args: string }) => {
        fake.efforts.push(args)
        return fake.rejectRuns ? Promise.reject(new Error('dropped')) : { text: '' }
      },
    },
    session: {
      usage: async (options?: { readonly breakdown?: string }) => ({
        calls: (fake.usageCalls += 1),
        failure: fake.rejectUsage ? await Promise.reject(new Error('unavailable')) : undefined,
        context:
          options?.breakdown === undefined
            ? fake.context
            : { ...fake.context, breakdown: { autoCompactThreshold: fake.threshold, totalTokens: fake.estimate } },
      }),
      id: async () => fake.sessionId,
      turns: async () => fake.turns,
      messages: async () => {
        fake.messageReads += 1
        await fake.transcriptGate
        return fake.transcript
      },
      compact: async () => {
        fake.compactions += 1
        return { skip: 'test' }
      },
    },
    store: {
      get: async (key: string) => (fake.rejectStore ? Promise.reject(new Error('unreadable')) : fake.store.get(key)),
      set: async (key: string, value: unknown) => {
        fake.store.set(key, value)
      },
    },
    ui: { status: (text: unknown) => fake.statuses.push(text) },
  }
}

function hookFor(fake: Fake, event: string, key?: string): Function {
  const found = fake.hooks.find(
    hooked => hooked.event === event && (key === undefined || Object.values(Object(hooked.matcher)).flat().includes(key)),
  )?.hook
  if (typeof found !== 'function') {
    throw new Error(`no hook for ${event}`)
  }
  return found
}

const invoke = (fake: Fake, event: string, args: readonly unknown[], key?: string): unknown =>
  Reflect.apply(hookFor(fake, event, key), undefined, [engine(fake), ...args])

const isGenerator = (value: unknown): value is AsyncGenerator<unknown, unknown> =>
  typeof value === 'object' && value !== null && Symbol.asyncIterator in value

async function drain(stream: unknown): Promise<unknown> {
  if (!isGenerator(stream)) {
    throw new Error('not a stream')
  }
  const chunk = await stream.next()
  return chunk.done === true ? chunk.value : drain(stream)
}

// A turn.step stream must be a generator even when, standing for the model, it streams nothing.
// oxlint-disable-next-line require-yield
async function* model(fake: Fake, input: { readonly effort?: unknown }): AsyncGenerator<never, unknown> {
  fake.sent.push(input.effort)
  fake.inputs.push(input)
  return {
    usage:
      fake.tokens === undefined
        ? null
        : { input_tokens: fake.tokens, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, output_tokens: 0 },
  }
}

const start = (fake: Fake): unknown => invoke(fake, 'session.start', [{}, async () => ({})])

const step = (fake: Fake, input: Step): Promise<unknown> =>
  drain(invoke(fake, 'turn.step', [{ model: 'opus', effort: 'high', ...input }, (event: Step) => model(fake, event)]))

const complete = (fake: Fake, agentId?: string): unknown =>
  invoke(fake, 'turn.complete', [{ agentId }, async () => ({ text: '' })])

const action = (fake: Fake, args: string): unknown => invoke(fake, 'command.run', [{ args }], 'dynamic-effort')

const effortCommand = (fake: Fake, args: string, origin: unknown): unknown =>
  invoke(fake, 'command.run', [{ args, origin }, async () => ({ text: 'Set' })], 'effort')

const compact = (fake: Fake, input: unknown, result: unknown): unknown =>
  invoke(fake, 'session.compact', [input, async () => result])

const compactInstructions = (fake: Fake, input: unknown): Promise<unknown> =>
  Promise.resolve(
    invoke(fake, 'session.compact', [input, async (forwarded: { readonly instructions?: string }) => ({ instructions: forwarded.instructions })]),
  )

// Each call reads a different file unless a test repeats one on purpose.
const tool = (fake: Fake, agentId: string | undefined, isError: boolean): unknown =>
  invoke(fake, 'tool.call', [
    { tool: 'Read', file_path: `/file-${(fake.toolCalls += 1)}`, tool_use_id: `id-${fake.toolCalls}`, agentId },
    async () => ({ isError, text: 'contents' }),
  ])

const repeatTool = (fake: Fake, command: string, text: string): unknown =>
  invoke(fake, 'tool.call', [
    { tool: 'Bash', command, description: `run ${(fake.toolCalls += 1)}`, tool_use_id: `id-${fake.toolCalls}`, requestMeta: { at: `${fake.toolCalls}` } },
    async () => ({ isError: false, text }),
  ])

const failedRound = (command: string): unknown => ({
  role: 'assistant',
  text: '',
  toolUses: [{ tool_use_id: command, tool: 'Bash', input: { command }, isError: true, text: 'boom' }],
})

// A dropped command's rejection reaches the mod's handler a few microtasks later.
const SETTLE_TICKS = 5 satisfies number

const settle = (ticks: number = SETTLE_TICKS): Promise<void> =>
  ticks === 0 ? Promise.resolve() : Promise.resolve().then(() => settle(ticks - 1))

// Performance guards: sizes large enough that a per-step scan or an unbounded buffer shows up,
// bounds loose enough for a busy CI machine.
const SMALL_SESSION = 4_000 satisfies number
const LARGE_SESSION = 40_000 satisfies number
// Linear total time scales 10x between the sessions; a per-step cost growing with the session
// would scale about 100x.
const MAX_SCALING = 25 satisfies number
const TOOL_CALLS = 200_000 satisfies number
// 200k retained booleans alone would take about 1.6 MB.
const MAX_HEAP_GROWTH = 1_000_000 satisfies number

setFlagsFromString('--expose-gc')
const collect = (): unknown => runInNewContext('gc')()

async function steps(fake: Fake, count: number, turn: number = 0): Promise<void> {
  if (turn === count) {
    return
  }
  await tool(fake, undefined, turn % 3 === 0)
  await step(fake, { turnId: 't', index: turn })
  return steps(fake, count, turn + 1)
}

async function timed(count: number): Promise<number> {
  const fake = setup()
  fake.tokens = 50
  await start(fake)
  const started = performance.now()
  await steps(fake, count)
  return performance.now() - started
}

async function tools(fake: Fake, count: number): Promise<void> {
  if (count === 0) {
    return
  }
  await tool(fake, undefined, count % 2 === 0)
  return tools(fake, count - 1)
}

test('main steps ramp with context, recover on failures and leave subagents and budgets alone', async () => {
  const fake = setup()
  await start(fake)
  fake.tokens = 90
  await step(fake, { turnId: 't1', index: 0 })
  fake.tokens = 10
  await step(fake, { turnId: 't1', index: 1 })
  await tool(fake, undefined, true)
  await tool(fake, 'agent-1', true)
  await step(fake, { turnId: 't1', index: 2 })
  await step(fake, { turnId: 't1', index: 3, agentId: 'agent-1' })
  await step(fake, { turnId: 't1', index: 4, effort: 4096 })
  expect(fake.sent).toStrictEqual(['medium', 'xhigh', 'high', 'high', 4096])
  expect(fake.statuses).toStrictEqual(['effort: medium (dynamic)', 'effort: xhigh (dynamic)', 'effort: high (dynamic)'])
})

test('the auto-compact threshold is used when the breakdown reports one', async () => {
  const fake = setup()
  fake.threshold = 50
  fake.tokens = 45
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await step(fake, { turnId: 't1', index: 1 })
  expect(fake.sent).toStrictEqual(['medium', 'xhigh'])
})

test('the compaction level is queued once, confirmed, and inherited subagent steps get the original', async () => {
  const fake = setup()
  await start(fake)
  await complete(fake)
  expect(fake.efforts).toStrictEqual([])
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake, 'agent-1')
  await complete(fake)
  await complete(fake)
  await effortCommand(fake, 'max', OWN)
  await step(fake, { turnId: 't2', index: 0, agentId: 'agent-1', effort: 'max' })
  await step(fake, { turnId: 't2', index: 1, agentId: 'agent-1', effort: 'low' })
  expect(fake.efforts).toStrictEqual(['max'])
  expect(fake.sent).toStrictEqual(['medium', 'high', 'low'])
})

test('a dropped command is queued again at the next turn end', async () => {
  const fake = setup()
  fake.rejectRuns = true
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await settle()
  fake.rejectRuns = false
  await complete(fake)
  expect(fake.efforts).toStrictEqual(['max', 'max'])
})

test("the person's /effort becomes the original and other runs change nothing", async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await effortCommand(fake, 'max', OWN)
  await effortCommand(fake, 'low', PERSON)
  await effortCommand(fake, 'auto', PERSON)
  await complete(fake)
  await effortCommand(fake, 'max', OWN)
  await step(fake, { turnId: 't2', index: 0, agentId: 'agent-1', effort: 'max' })
  expect(fake.efforts).toStrictEqual(['max', 'max'])
  expect(fake.sent).toStrictEqual(['medium', 'low'])
})

test('the session effort follows the mode: max while dynamic, the original while off', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await effortCommand(fake, 'max', OWN)
  expect(await action(fake, 'off')).toStrictEqual({
    text: 'dynamic=off original=high session=max pending=false last=none min=medium end=xhigh levels=medium,high,xhigh compactLimits=100 context=0 recoverySteps=0',
  })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't2', index: 0, effort: 'max' })
  fake.rejectRuns = true
  await complete(fake)
  await settle()
  fake.rejectRuns = false
  await complete(fake)
  await effortCommand(fake, 'high', OWN)
  await complete(fake)
  await action(fake, 'on')
  await complete(fake)
  expect(fake.efforts).toStrictEqual(['max', 'high', 'high', 'max'])
  expect(fake.sent).toStrictEqual(['medium', 'high'])
  expect(fake.statuses).toStrictEqual(['effort: medium (dynamic)', 'effort: static', undefined])
})
test('status, the default action and an unknown action answer without changing the mode', async () => {
  const fake = setup()
  await start(fake)
  expect(await action(fake, 'bogus')).toStrictEqual({ text: 'Usage: /dynamic-effort on|off|status' })
  expect(await action(fake, '')).toStrictEqual({
    text: 'dynamic=on original=unknown session=unknown pending=false last=none min=medium end=xhigh levels=medium,high,xhigh compactLimits=unknown context=0 recoverySteps=0',
  })
  expect(fake.statuses).toStrictEqual([])
})
test('precompute waits for the compaction level and a compaction lets effort fall with the context', async () => {
  const fake = setup()
  await start(fake)
  fake.tokens = 90
  await step(fake, { turnId: 't1', index: 0 })
  await step(fake, { turnId: 't1', index: 1 })
  expect(await compact(fake, { trigger: 'precompute' }, { messages: SUMMARY })).toStrictEqual({
    skip: 'dynamic-effort: compaction effort is not applied yet',
  })
  await complete(fake)
  await effortCommand(fake, 'max', OWN)
  expect(await compact(fake, { trigger: 'precompute' }, { messages: SUMMARY })).toStrictEqual({
    messages: [{ role: 'user', text: 'summary', toolUses: [] }],
  })
  await compact(fake, { trigger: 'auto', agentId: 'agent-1' }, { messages: SUMMARY })
  await compact(fake, { trigger: 'manual' }, { skip: 'blocked' })
  await compact(fake, { trigger: 'auto' }, { messages: SUMMARY })
  await step(fake, { turnId: 't2', index: 0 })
  fake.tokens = 10
  await step(fake, { turnId: 't2', index: 1 })
  await step(fake, { turnId: 't2', index: 2 })
  await compact(fake, { trigger: 'auto' }, { messages: SUMMARY, tokensAfter: 95 })
  await step(fake, { turnId: 't3', index: 0 })
  expect(fake.sent).toStrictEqual(['medium', 'xhigh', 'medium', 'xhigh', 'medium', 'xhigh'])
})

test('the compaction row is hidden and the spinner shows the sent effort only while dynamic', async () => {
  const fake = setup()
  await start(fake)
  const render = (event: unknown, key: string): unknown => invoke(fake, 'ui.render', [event, (rendered: unknown) => rendered], key)
  expect(render({ props: { command: 'effort', args: ' max ' } }, 'CommandOutput')).toStrictEqual({ type: 'Box', children: [] })
  expect(render({ props: { command: 'effort', args: 'low' } }, 'CommandOutput')).toStrictEqual({ props: { command: 'effort', args: 'low' } })
  expect(render({ props: { command: 'cost', args: 'max' } }, 'CommandOutput')).toStrictEqual({ props: { command: 'cost', args: 'max' } })
  expect(render({ props: { suffix: '…' } }, 'Spinner')).toStrictEqual({ props: { suffix: '…' } })
  await step(fake, { turnId: 't1', index: 0 })
  expect(render({ props: { suffix: '…' } }, 'Spinner')).toStrictEqual({ props: { suffix: '… · dynamic medium' } })
  await action(fake, 'off')
  expect(render({ props: { suffix: '…' } }, 'Spinner')).toStrictEqual({ props: { suffix: '…' } })
})

test('two clean tool rounds lower the recovery floor and a repeated step is not counted twice', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 2 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 3 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 3 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'high', 'medium', 'medium'])
})

test('steps make no host call after the one threshold lookup per model', async () => {
  const fake = setup()
  await start(fake)
  await steps(fake, 1_000)
  expect(fake.usageCalls).toBe(1)
  await step(fake, { turnId: 'other', index: 0, effort: 'high', model: 'sonnet' })
  await step(fake, { turnId: 'other', index: 1, effort: 'high', model: 'sonnet' })
  expect(fake.usageCalls).toBe(2)
})

test('unchanged events are passed on without a copy', async () => {
  const fake = setup()
  await start(fake)
  const budget = { turnId: 't1', index: 0, model: 'opus', effort: 4096 }
  await drain(invoke(fake, 'turn.step', [budget, (event: unknown) => model(fake, Object(event))]))
  expect(fake.inputs[0]).toBe(budget)
  const spinner = { props: { suffix: '…' } }
  expect(invoke(fake, 'ui.render', [spinner, (rendered: unknown) => rendered], 'Spinner')).toBe(spinner)
  const row = { props: { command: 'cost', args: '' } }
  expect(invoke(fake, 'ui.render', [row, (rendered: unknown) => rendered], 'CommandOutput')).toBe(row)
})

test('the per-step cost stays constant as a session grows', async () => {
  await timed(SMALL_SESSION)
  const small = await timed(SMALL_SESSION)
  const large = await timed(LARGE_SESSION)
  expect(large / small < MAX_SCALING).toBe(true)
})

test('tool rounds keep the heap flat whether dynamic mode is on or off', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't', index: 0 })
  collect()
  const before = getHeapStatistics().used_heap_size
  await tools(fake, TOOL_CALLS)
  await action(fake, 'off')
  await tools(fake, TOOL_CALLS)
  collect()
  expect(getHeapStatistics().used_heap_size - before < MAX_HEAP_GROWTH).toBe(true)
})

test('with the threshold cached a step reaches the model without waiting for a microtask', async () => {
  const fake = setup()
  await start(fake)
  await settle()
  const first = invoke(fake, 'turn.step', [{ turnId: 't1', index: 0, model: 'opus', effort: 'high' }, (event: Step) => model(fake, event)])
  if (!isGenerator(first)) {
    throw new Error('not a stream')
  }
  void first.next()
  expect(fake.sent).toStrictEqual(['medium'])
})

test('a failed prefetch leaves the first step to fetch the threshold itself', async () => {
  const fake = setup()
  fake.rejectUsage = true
  await start(fake)
  await settle()
  fake.rejectUsage = false
  fake.tokens = 90
  await step(fake, { turnId: 't1', index: 0 })
  await step(fake, { turnId: 't1', index: 1 })
  expect(fake.usageCalls).toBe(2)
  expect(fake.sent).toStrictEqual(['medium', 'xhigh'])
})

test('on a large context a recovered floor drops right away', async () => {
  const fake = setup()
  fake.context = { window: 1_000_000 }
  fake.tokens = 100_000
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 2 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 3 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'high', 'medium'])
})

test('a failure raises effort, a repeat with the same outcome holds it, clean rounds lower it', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await repeatTool(fake, 'gh run view 1', 'in_progress')
  await step(fake, { turnId: 't1', index: 2 })
  await repeatTool(fake, 'gh run view 1', 'in_progress')
  await step(fake, { turnId: 't1', index: 3 })
  await repeatTool(fake, 'gh run view 1', 'completed')
  await step(fake, { turnId: 't1', index: 4 })
  await repeatTool(fake, 'bun lint', 'ok')
  await step(fake, { turnId: 't1', index: 5 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'high', 'high', 'high', 'medium'])
})
test('a repeat is remembered only within the window of recent calls', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await repeatTool(fake, 'gh run view 1', 'in_progress')
  await step(fake, { turnId: 't1', index: 2 })
  await tool(fake, undefined, false)
  await tool(fake, undefined, false)
  await tool(fake, undefined, false)
  await tool(fake, undefined, false)
  await tool(fake, undefined, false)
  await tool(fake, undefined, false)
  await tool(fake, undefined, false)
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 3 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 4 })
  await repeatTool(fake, 'gh run view 1', 'in_progress')
  await step(fake, { turnId: 't1', index: 5 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 6 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'high', 'medium', 'high', 'high', 'medium'])
})
test('a resumed session ramps from the estimated context before its first response', async () => {
  const fake = setup()
  fake.turns = 2
  fake.estimate = 95
  await start(fake)
  await settle()
  await step(fake, { turnId: 't1', index: 0 })
  expect(fake.sent).toStrictEqual(['xhigh'])
  expect(fake.compactions).toBe(0)
})

test('a resumed session rebuilds its floor and failed calls, so a fix after resume is verified', async () => {
  const fake = setup()
  fake.turns = 3
  fake.transcript = [
    { role: 'user', text: 'fix it', toolUses: [] },
    failedRound('bun test'),
    { role: 'assistant', text: 'done', toolUses: [] },
    failedRound('bun lint'),
  ]
  await start(fake)
  await settle()
  await step(fake, { turnId: 't1', index: 0 })
  await repeatTool(fake, 'bun lint', 'boom')
  await step(fake, { turnId: 't1', index: 1 })
  expect(fake.sent).toStrictEqual(['xhigh', 'high'])
  expect(fake.compactions).toBe(0)
})

test('a new session reads no transcript and a replay never overwrites a step that already ran', async () => {
  const fresh = setup()
  await start(fresh)
  await settle()
  expect(fresh.messageReads).toBe(0)
  const resumed = setup()
  resumed.turns = 1
  resumed.transcript = [failedRound('bun test'), failedRound('bun lint')]
  const gate = { open: (): void => undefined }
  resumed.transcriptGate = new Promise(resolve => {
    gate.open = resolve
  })
  await start(resumed)
  await settle()
  await step(resumed, { turnId: 't1', index: 0 })
  gate.open()
  await settle()
  await step(resumed, { turnId: 't1', index: 1 })
  expect(resumed.sent).toStrictEqual(['medium', 'medium'])
  expect(resumed.compactions).toBe(0)
})

test('after a run of failures, two clean rounds per level bring effort back down', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 2 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 3 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 4 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 5 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 6 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'xhigh', 'xhigh', 'xhigh', 'xhigh', 'high'])
})

test('the compaction guideline follows the person\'s focus and leaves subagent compactions alone', async () => {
  const fake = setup()
  await start(fake)
  expect(await compactInstructions(fake, { trigger: 'auto' })).toStrictEqual({
    instructions: 'Also list approaches that were tried and abandoned, with the error or reason, so they are not repeated.',
  })
  expect(await compactInstructions(fake, { trigger: 'manual', instructions: 'Keep the API notes' })).toStrictEqual({
    instructions:
      'Keep the API notes\n\nAlso list approaches that were tried and abandoned, with the error or reason, so they are not repeated.',
  })
  expect(await compactInstructions(fake, { trigger: 'auto', agentId: 'agent-1', instructions: 'own' })).toStrictEqual({
    instructions: 'own',
  })
})

const sessionStart = (fake: Fake, source: string): unknown =>
  invoke(fake, 'classic.SessionStart', [{ source }, async () => ({})], source)

test("a person's /effort that ran before the mod's command is kept and released to when off", async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await effortCommand(fake, 'low', PERSON)
  await effortCommand(fake, 'max', OWN)
  await step(fake, { turnId: 't2', index: 0, agentId: 'agent-1', effort: 'max' })
  await action(fake, 'off')
  await complete(fake)
  expect(fake.efforts).toStrictEqual(['max', 'low'])
  expect(fake.sent).toStrictEqual(['medium', 'low'])
})

test('an in-process /clear starts the context and failure evidence over but keeps the original', async () => {
  const fake = setup()
  await start(fake)
  fake.tokens = 95
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  fake.tokens = undefined
  await sessionStart(fake, 'clear')
  await settle()
  await step(fake, { turnId: 't2', index: 0, effort: 'max' })
  await step(fake, { turnId: 't2', index: 1, agentId: 'agent-1', effort: 'max' })
  expect(fake.sent).toStrictEqual(['medium', 'xhigh', 'medium', 'high'])
})

test('a replayed call and a live one compare equal whatever order their keys were built in', async () => {
  const fake = setup()
  fake.turns = 1
  fake.transcript = [
    { role: 'assistant', text: '', toolUses: [{ tool_use_id: 'a', tool: 'Bash', input: { command: 'echo same' }, text: 'same' }] },
  ]
  await start(fake)
  await settle()
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await repeatTool(fake, 'echo same', 'same')
  await step(fake, { turnId: 't1', index: 2 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 3 })
  await tool(fake, undefined, false)
  await step(fake, { turnId: 't1', index: 4 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'high', 'high', 'medium'])
})

test('a compaction clears the repeat window but keeps the recovery floor', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await repeatTool(fake, 'cat a.ts', 'contents')
  await step(fake, { turnId: 't1', index: 2 })
  await compact(fake, { trigger: 'auto' }, { messages: SUMMARY })
  await repeatTool(fake, 'cat a.ts', 'contents')
  await step(fake, { turnId: 't1', index: 3 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'high', 'medium'])
})

const malformedTool = (fake: Fake, text: string): unknown =>
  invoke(fake, 'tool.call', [
    { tool: 'Edit', file_path: `/file-${(fake.toolCalls += 1)}`, tool_use_id: `id-${fake.toolCalls}` },
    async () => ({ isError: true, text }),
  ])

test('a malformed call holds effort instead of raising it, while a real failure raises it', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await malformedTool(fake, '<tool_use_error>InputValidationError: old_string is required</tool_use_error>')
  await step(fake, { turnId: 't1', index: 1 })
  await malformedTool(fake, '<tool_use_error>Error: No such tool available: Grepx</tool_use_error>')
  await step(fake, { turnId: 't1', index: 2 })
  await malformedTool(fake, 'String to replace not found in file.')
  await step(fake, { turnId: 't1', index: 3 })
  expect(fake.sent).toStrictEqual(['medium', 'medium', 'medium', 'high'])
})

test('the chosen minimum sets the floor and the ladder up to xhigh, and an unknown choice falls back', async () => {
  const low = setup({ minEffort: 'low' })
  await start(low)
  await step(low, { turnId: 't1', index: 0 })
  await tool(low, undefined, true)
  await step(low, { turnId: 't1', index: 1 })
  expect(low.sent).toStrictEqual(['low', 'medium'])
  const high = setup({ minEffort: 'high' })
  low.tokens = undefined
  await start(high)
  high.tokens = 95
  await step(high, { turnId: 't1', index: 0 })
  await step(high, { turnId: 't1', index: 1 })
  expect(high.sent).toStrictEqual(['high', 'xhigh'])
  const unknown = setup({ minEffort: 'max' })
  await start(unknown)
  expect(await action(unknown, 'status')).toStrictEqual({
    text: 'dynamic=on original=unknown session=unknown pending=false last=none min=medium end=xhigh levels=medium,high,xhigh compactLimits=unknown context=0 recoverySteps=0',
  })
})

const commandTool = (fake: Fake, command: string, isError: boolean): Promise<unknown> =>
  Promise.resolve(
    invoke(fake, 'tool.call', [
      { tool: 'Bash', command, tool_use_id: `id-${(fake.toolCalls += 1)}` },
      async () => ({ isError, text: isError ? 'FAIL' : `ok ${fake.toolCalls}` }),
    ]),
  )

test('a failed call that later succeeds with the same input lowers effort at once', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await commandTool(fake, 'bun test', true)
  await step(fake, { turnId: 't1', index: 1 })
  await commandTool(fake, 'bun test', true)
  await step(fake, { turnId: 't1', index: 2 })
  await commandTool(fake, 'bun test', false)
  await step(fake, { turnId: 't1', index: 3 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'xhigh', 'high'])
})

test('a run of failures at the cap gets one change-approach note, cleared by a success', async () => {
  const fake = setup()
  await start(fake)
  const notes = async (index: number): Promise<unknown> => {
    const result = await commandTool(fake, `attempt ${index}`, true)
    await step(fake, { turnId: 't1', index })
    return Object(result).context
  }
  await step(fake, { turnId: 't1', index: 0 })
  expect([await notes(1), await notes(2), await notes(3), await notes(4), await notes(5), await notes(6)]).toStrictEqual([
    undefined,
    undefined,
    undefined,
    undefined,
    [
      'dynamic-effort: this approach has now failed for several rounds at the highest work effort. More reasoning on the same path is unlikely to help: state what is blocking, then try a different approach or ask the person.',
    ],
    undefined,
  ])
  await commandTool(fake, 'cleanup', false)
  await step(fake, { turnId: 't1', index: 7 })
  expect(await notes(8)).toStrictEqual(undefined)
})

test('a repeated long command is still a repeat after its input is condensed', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await tool(fake, undefined, true)
  await step(fake, { turnId: 't1', index: 1 })
  await repeatTool(fake, `echo ${'x'.repeat(5000)}`, 'out')
  await step(fake, { turnId: 't1', index: 2 })
  await repeatTool(fake, `echo ${'x'.repeat(5000)}`, 'out')
  await step(fake, { turnId: 't1', index: 3 })
  expect(fake.sent).toStrictEqual(['medium', 'high', 'high', 'high'])
})

test('an error result without text still counts as a failure', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await invoke(fake, 'tool.call', [{ tool: 'Read', file_path: '/missing', tool_use_id: 'id-1' }, async () => ({ isError: true })])
  await step(fake, { turnId: 't1', index: 1 })
  expect(fake.sent).toStrictEqual(['medium', 'high'])
})

test('a prefetch that finishes after the first step leaves the step its own threshold lookup', async () => {
  const fake = setup()
  void start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await settle()
  expect(fake.usageCalls).toBe(2)
})

test('a resumed session keeps its switch, a new session starts on', async () => {
  const fake = setup()
  await start(fake)
  await action(fake, 'off')
  await settle()
  expect(fake.store.get('offSessions')).toStrictEqual(['session-1'])
  await sessionStart(fake, 'resume')
  await step(fake, { turnId: 't1', index: 0 })
  fake.sessionId = 'session-2'
  await sessionStart(fake, 'clear')
  await step(fake, { turnId: 't2', index: 0 })
  expect(fake.sent).toStrictEqual(['high', 'medium'])
})

test('switching back on forgets the session, and an unreadable store leaves a session on', async () => {
  const fake = setup()
  fake.store.set('offSessions', ['old', 'session-1', 7])
  await start(fake)
  await action(fake, 'on')
  await settle()
  expect(fake.store.get('offSessions')).toStrictEqual(['old'])
  fake.store.set('offSessions', 'corrupt')
  await sessionStart(fake, 'resume')
  await step(fake, { turnId: 't1', index: 0 })
  expect(fake.sent).toStrictEqual(['medium'])
})

test('the off list keeps only the most recent sessions', async () => {
  const fake = setup()
  fake.store.set('offSessions', Array.from({ length: 32 }, (_, index) => `old-${index}`))
  await start(fake)
  await action(fake, 'off')
  await settle()
  expect(fake.store.get('offSessions')).toHaveLength(32)
  expect(Array.from(Object(fake.store.get('offSessions'))).at(-1)).toBe('session-1')
})

test('a store that fails leaves the session on and its switch unsaved', async () => {
  const fake = setup()
  fake.rejectStore = true
  await start(fake)
  await action(fake, 'off')
  await step(fake, { turnId: 't1', index: 0 })
  await action(fake, 'on')
  fake.rejectStore = false
  await sessionStart(fake, 'clear')
  fake.rejectStore = true
  await action(fake, 'off')
  await settle()
  expect(fake.sent).toStrictEqual(['high'])
  expect(fake.store.has('offSessions')).toBe(false)
})

test('a switch read for a reset that a newer one replaced is dropped', async () => {
  const fake = setup()
  fake.store.set('offSessions', ['session-1'])
  void start(fake)
  fake.store.set('offSessions', [])
  await sessionStart(fake, 'resume')
  await step(fake, { turnId: 't1', index: 0 })
  expect(fake.sent).toStrictEqual(['medium'])
})
