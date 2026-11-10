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

interface Fake {
  readonly hooks: Hooked[]
  readonly sent: unknown[]
  readonly efforts: string[]
  readonly aborted: string[]
  readonly prompts: string[]
  readonly toasts: unknown[]
  readonly logs: unknown[]
  readonly inputs: unknown[]
  hostCalls: number
  tokens: number | undefined
  estimate: number
  threshold: number | undefined
  rejectRuns: boolean
  rejectAbort: boolean
  rejectPrompt: boolean
  runGate: Promise<void>
  sessionId: string
  readonly store: Map<string, unknown>
  rejectStore: boolean
  failingKeys: ReadonlySet<string>
  registered: number
  settings: unknown
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
const COMPACTED = { messages: [{ role: 'user', text: 'summary', toolUses: [] }] } satisfies { messages: unknown[] }

function setup(): Fake {
  const fake: Fake = { hooks: [], sent: [], efforts: [], aborted: [], prompts: [], toasts: [], logs: [], inputs: [], hostCalls: 0, tokens: undefined, estimate: 0, threshold: undefined, rejectRuns: false, rejectAbort: false, rejectPrompt: false, runGate: Promise.resolve(), sessionId: 'session-1', store: new Map(), rejectStore: false, failingKeys: new Set(), registered: 0, settings: { effortLevel: 'high', modelSettings: { opus: { effortLevel: 'high' } } } }
  const on = (event: unknown, matcher: unknown, hook?: unknown): { readonly catch: () => void } => {
    fake.hooks.push(hook === undefined ? { event, matcher: undefined, hook: matcher } : { event, matcher, hook })
    return { catch: () => undefined }
  }
  register(on, {})
  return fake
}

function engine(fake: Fake): unknown {
  return {
    command: {
      register: async () => ({ command: 'dynamic-effort', count: (fake.registered += 1) }),
      run: async ({ command, args }: { readonly command: string; readonly args: string }) => {
        fake.efforts.push(`${command} ${args}`.trim())
        await fake.runGate
        return fake.rejectRuns ? Promise.reject(new Error('dropped')) : { text: '' }
      },
    },
    session: {
      id: async () => {
        fake.hostCalls += 1
        return fake.sessionId
      },
      usage: async () => {
        fake.hostCalls += 1
        return {
          context: {
            tokens: fake.tokens,
            window: 1000,
            breakdown: { autoCompactThreshold: fake.threshold, totalTokens: fake.estimate },
          },
        }
      },
    },
    turn: {
      abort: async ({ turnId }: { readonly turnId: string }) => {
        fake.aborted.push(turnId)
        return fake.rejectAbort ? Promise.reject(new Error('not running')) : undefined
      },
    },
    prompt: {
      submit: async ({ text }: { readonly text: string }) => {
        fake.prompts.push(text)
        return fake.rejectPrompt ? Promise.reject(new Error('refused')) : {}
      },
    },
    store: {
      get: async (key: string) => storeResult(fake, () => fake.store.get(key)),
      set: async (key: string, value: unknown) => storeResult(fake, () => fake.store.set(key, value)),
      delete: async (key: string) =>
        fake.failingKeys.has(key) ? Promise.reject(new Error(`locked ${key}`)) : storeResult(fake, () => fake.store.delete(key)),
      keys: async () => storeResult(fake, () => [...fake.store.keys()]),
    },
    ui: { toast: (text: unknown) => fake.toasts.push(text), log: (text: unknown) => fake.logs.push(text) },
    settings: { read: async () => (fake.settings instanceof Error ? Promise.reject(fake.settings) : fake.settings) },
    clock: {
      after: (_ms: number, run: () => void) => {
        queueMicrotask(run)
        return { cancel: () => undefined }
      },
    },
  }
}

const storeResult = <Value>(fake: Fake, read: () => Value): Promise<Value> =>
  fake.rejectStore ? Promise.reject(new Error('unavailable')) : Promise.resolve(read())

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
  return { usage: null }
}

const start = (fake: Fake): unknown => invoke(fake, 'session.start', [{}, async () => ({})])

const sessionStart = (fake: Fake, source: string): unknown =>
  invoke(fake, 'classic.SessionStart', [{ source }, async () => ({})])

const turnStart = (fake: Fake, turnId: string, agentId?: string): unknown =>
  invoke(fake, 'turn.start', [{ turnId, agentId, text: '' }, async () => ({})])

const step = (fake: Fake, input: Step): Promise<unknown> =>
  drain(invoke(fake, 'turn.step', [{ model: 'opus', effort: 'high', ...input }, (event: Step) => model(fake, event)]))

const complete = (fake: Fake, agentId?: string): unknown =>
  invoke(fake, 'turn.complete', [{ agentId }, async () => ({ text: '' })])

const hint = (fake: Fake, tail?: string): unknown =>
  invoke(fake, 'ui.render', [{ props: { hint: '? for shortcuts', tail } }, (rendered: unknown) => rendered], 'PromptHint')

const action = (fake: Fake, args: string): unknown => invoke(fake, 'command.run', [{ args }], 'dynamic-effort')

const effortCommand = (fake: Fake, args: string, origin: unknown): unknown =>
  invoke(fake, 'command.run', [{ args, origin }, async () => ({ text: 'Set' })], 'effort')

const compact = (fake: Fake, input: unknown): unknown => invoke(fake, 'session.compact', [input, async () => COMPACTED])

const compactInstructions = (fake: Fake, input: unknown): Promise<unknown> =>
  Promise.resolve(
    invoke(fake, 'session.compact', [input, async (forwarded: { readonly instructions?: string }) => ({ instructions: forwarded.instructions })]),
  )

// A dropped command's rejection reaches the mod's handler a few microtasks later.
const SETTLE_TICKS = 8 satisfies number

const settle = (ticks: number = SETTLE_TICKS): Promise<void> =>
  ticks === 0 ? Promise.resolve() : Promise.resolve().then(() => settle(ticks - 1))

// Performance guards: sizes large enough that a per-step scan or an unbounded buffer shows up,
// bounds loose enough for a busy CI machine.
const SMALL_SESSION = 4_000 satisfies number
const LARGE_SESSION = 40_000 satisfies number
// Linear total time scales 10x between the sessions; a per-step cost growing with the session
// would scale about 100x.
const MAX_SCALING = 25 satisfies number
const HEAP_STEPS = 100_000 satisfies number
const MAX_HEAP_GROWTH = 2_000_000 satisfies number

setFlagsFromString('--expose-gc')
const collect = (): unknown => runInNewContext('gc')()

async function steps(fake: Fake, count: number, turn: number = 0): Promise<void> {
  if (turn === count) {
    return
  }
  await step(fake, { turnId: 't', index: turn })
  return steps(fake, count, turn + 1)
}

async function timed(count: number): Promise<number> {
  const fake = setup()
  await start(fake)
  const started = performance.now()
  await steps(fake, count)
  return performance.now() - started
}

test('main steps always go out at the session level, the same event, and nothing is queued below the threshold', async () => {
  const fake = setup()
  fake.threshold = 100
  fake.tokens = 89
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  await step(fake, { turnId: 't1', index: 1, effort: 'medium' })
  await complete(fake)
  await settle()
  expect(fake.sent).toStrictEqual(['medium', 'medium'])
  expect(fake.efforts).toStrictEqual([])
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=medium session=medium pending=false last=medium' })
})

test('at 90% of the threshold a turn end queues raise, compact and restore once', async () => {
  const fake = setup()
  fake.threshold = 100
  fake.tokens = 90
  const queued = { resolve: (): void => undefined }
  fake.runGate = new Promise<void>(resolve => {
    queued.resolve = resolve
  })
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual(['effort max', 'compact', 'effort high'])
  expect(fake.aborted).toStrictEqual([])
  expect(fake.prompts).toStrictEqual([])
  await effortCommand(fake, 'max', OWN)
  expect(await compact(fake, { trigger: 'manual' })).toStrictEqual(COMPACTED)
  await effortCommand(fake, 'high', OWN)
  queued.resolve()
  await settle()
  fake.tokens = 10
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual(['effort max', 'compact', 'effort high'])
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=high session=high pending=false last=high' })
})

test('the turn end falls back to the estimate when the status line has no count', async () => {
  const fake = setup()
  fake.threshold = 100
  fake.estimate = 95
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual(['effort max', 'compact', 'effort high'])
})

test('no compaction is queued when auto-compaction or dynamic mode is off', async () => {
  const fake = setup()
  fake.tokens = 95
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual([])
  fake.threshold = 100
  await action(fake, 'off')
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual([])
})

test('a subagent turn end queues nothing and keeps the main turn id', async () => {
  const fake = setup()
  fake.threshold = 100
  fake.tokens = 95
  await start(fake)
  await turnStart(fake, 'main-1')
  await turnStart(fake, 'agent-turn', 'agent-1')
  await step(fake, { turnId: 'main-1', index: 0 })
  await complete(fake, 'agent-1')
  await settle()
  expect(fake.efforts).toStrictEqual([])
  await compact(fake, { trigger: 'auto' })
  expect(fake.aborted).toStrictEqual(['main-1'])
})

test('an auto-compaction inside a turn stops the turn, runs the sequence and resumes the work', async () => {
  const fake = setup()
  await start(fake)
  await turnStart(fake, 'main-1')
  await step(fake, { turnId: 'main-1', index: 0, effort: 'medium' })
  fake.settings = { effortLevel: 'medium' }
  expect(await compact(fake, { trigger: 'auto' })).toStrictEqual({ skip: 'dynamic-effort: compacting again at max effort' })
  await settle()
  expect(fake.aborted).toStrictEqual(['main-1'])
  expect(fake.efforts).toStrictEqual(['effort max', 'compact', 'effort medium'])
  expect(fake.prompts).toStrictEqual(['The conversation was compacted while you were working. Continue the task from where you stopped.'])
})

test('an auto-compaction with the session idle runs the sequence and resumes nothing', async () => {
  const fake = setup()
  await start(fake)
  await turnStart(fake, 'main-1')
  await step(fake, { turnId: 'main-1', index: 0 })
  await complete(fake)
  expect(await compact(fake, { trigger: 'auto' })).toStrictEqual({ skip: 'dynamic-effort: compacting again at max effort' })
  await settle()
  expect(fake.aborted).toStrictEqual([])
  expect(fake.efforts).toStrictEqual(['effort max', 'compact', 'effort high'])
  expect(fake.prompts).toStrictEqual([])
})

test('a turn that cannot be stopped rejects, so the engine compacts at the session effort', async () => {
  const fake = setup()
  fake.rejectAbort = true
  await start(fake)
  await turnStart(fake, 'main-1')
  await step(fake, { turnId: 'main-1', index: 0 })
  await expect(compact(fake, { trigger: 'auto' })).rejects.toThrow('not running')
  await settle()
  expect(fake.efforts).toStrictEqual([])
})

test('a refused resume is logged after the sequence is queued', async () => {
  const fake = setup()
  fake.rejectPrompt = true
  await start(fake)
  await turnStart(fake, 'main-1')
  await step(fake, { turnId: 'main-1', index: 0 })
  await compact(fake, { trigger: 'auto' })
  await settle()
  expect(fake.efforts).toStrictEqual(['effort max', 'compact', 'effort high'])
  expect(fake.logs).toStrictEqual(['dynamic-effort: resuming the turn failed: refused'])
})

test("the person's /compact is skipped and run again with its focus between the raise and the restore", async () => {
  const fake = setup()
  const queued = { resolve: (): void => undefined }
  fake.runGate = new Promise<void>(resolve => {
    queued.resolve = resolve
  })
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  expect(await compact(fake, { trigger: 'manual', instructions: 'Keep the plan' })).toStrictEqual({
    skip: 'dynamic-effort: compacting again at max effort',
  })
  await settle()
  expect(fake.efforts).toStrictEqual(['effort max', 'compact Keep the plan', 'effort high'])
  expect(await compact(fake, { trigger: 'manual', instructions: 'Keep the plan' })).toStrictEqual(COMPACTED)
  queued.resolve()
  await settle()
  expect(await compact(fake, { trigger: 'manual' })).toStrictEqual({ skip: 'dynamic-effort: compacting again at max effort' })
})

test('a refused re-run lets the next compaction through at the session effort, then re-runs again', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  fake.rejectRuns = true
  await compact(fake, { trigger: 'manual' })
  await settle()
  expect(fake.toasts).toStrictEqual(['dynamic-effort: could not compact at max effort; run /compact again'])
  expect(fake.logs).toStrictEqual([
    'dynamic-effort: /effort max failed: dropped',
    'dynamic-effort: /compact failed: dropped',
    'dynamic-effort: /effort high failed: dropped',
  ])
  fake.rejectRuns = false
  expect(await compact(fake, { trigger: 'manual' })).toStrictEqual(COMPACTED)
  expect(await compact(fake, { trigger: 'manual' })).toStrictEqual({ skip: 'dynamic-effort: compacting again at max effort' })
})

test('a compaction stays at the session effort when the restore would change the saved default', async () => {
  const fake = setup()
  fake.settings = { effortLevel: 'medium' }
  fake.threshold = 100
  fake.tokens = 95
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual([])
  expect(await compact(fake, { trigger: 'manual' })).toStrictEqual(COMPACTED)
  expect(await compact(fake, { trigger: 'auto' })).toStrictEqual(COMPACTED)
})

test("the restore also compares the session model's saved level, and a model without one follows the default", async () => {
  const fake = setup()
  fake.settings = { effortLevel: 'high', modelSettings: { opus: { effortLevel: 'low' } } }
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  expect(await compact(fake, { trigger: 'manual' })).toStrictEqual(COMPACTED)
  fake.settings = { effortLevel: 'high', modelSettings: 'corrupt' }
  expect(await compact(fake, { trigger: 'manual' })).toStrictEqual({ skip: 'dynamic-effort: compacting again at max effort' })
})

test('an unreadable settings file rejects the compaction hook', async () => {
  const fake = setup()
  fake.settings = new Error('unreadable')
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await expect(compact(fake, { trigger: 'manual' })).rejects.toThrow('unreadable')
  expect(fake.efforts).toStrictEqual([])
})

test('a compaction passes untouched while off, before the level is known, or for a subagent', async () => {
  const fake = setup()
  await start(fake)
  expect(await compact(fake, { trigger: 'auto' })).toStrictEqual(COMPACTED)
  await step(fake, { turnId: 't1', index: 0 })
  expect(await compact(fake, { trigger: 'auto', agentId: 'agent-1' })).toStrictEqual(COMPACTED)
  await action(fake, 'off')
  expect(await compact(fake, { trigger: 'auto' })).toStrictEqual(COMPACTED)
  await action(fake, 'on')
  expect(fake.efforts).toStrictEqual([])
})

test("the person's /effort max compacts at max with no sequence", async () => {
  const fake = setup()
  await start(fake)
  await effortCommand(fake, 'max', PERSON)
  await step(fake, { turnId: 't1', index: 0, effort: 'max' })
  expect(await compact(fake, { trigger: 'auto' })).toStrictEqual(COMPACTED)
  expect(fake.sent).toStrictEqual(['max'])
  expect(fake.efforts).toStrictEqual([])
})

test('precompute waits for the compaction level, unless the session will never be raised', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  expect(await compact(fake, { trigger: 'precompute' })).toStrictEqual({ skip: 'dynamic-effort: compaction effort is not applied yet' })
  await effortCommand(fake, 'max', OWN)
  expect(await compact(fake, { trigger: 'precompute' })).toStrictEqual(COMPACTED)
  await effortCommand(fake, 'high', OWN)
  fake.settings = { effortLevel: 'low' }
  expect(await compact(fake, { trigger: 'precompute' })).toStrictEqual(COMPACTED)
})

test('a step that inherits the raised level is sent at the person level', async () => {
  const fake = setup()
  const queued = { resolve: (): void => undefined }
  fake.runGate = new Promise<void>(resolve => {
    queued.resolve = resolve
  })
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  fake.settings = { effortLevel: 'medium' }
  await compact(fake, { trigger: 'manual' })
  await settle()
  await step(fake, { turnId: 'a1', index: 0, effort: 'max', agentId: 'agent-1' })
  await step(fake, { turnId: 't2', index: 0, effort: 'max' })
  expect(fake.sent).toStrictEqual(['medium', 'medium', 'medium'])
  expect(hint(fake)).toStrictEqual({ props: { hint: '? for shortcuts', tail: '🧠 medium' } })
  queued.resolve()
  await settle()
})

test('outside the sequence a subagent keeps its own effort', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  await step(fake, { turnId: 'a1', index: 0, effort: 'max', agentId: 'agent-1' })
  await step(fake, { turnId: 'a1', index: 1, effort: 'low', agentId: 'agent-1' })
  expect(fake.sent).toStrictEqual(['medium', 'max', 'low'])
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=medium session=medium pending=false last=medium' })
})

test('a session stranded at max goes back to the level at the next turn end, and a dropped restore is queued again', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0 })
  await effortCommand(fake, 'max', OWN)
  fake.rejectRuns = true
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual(['effort high'])
  expect(fake.logs).toStrictEqual(['dynamic-effort: /effort high failed: dropped'])
  fake.rejectRuns = false
  await complete(fake)
  await settle()
  expect(fake.efforts).toStrictEqual(['effort high', 'effort high'])
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=high session=high pending=false last=high' })
})

test('a session at max with no known level is sent at max and left there', async () => {
  const fake = setup()
  fake.threshold = 100
  fake.tokens = 95
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'max' })
  await complete(fake)
  await settle()
  expect(fake.sent).toStrictEqual(['max'])
  expect(fake.efforts).toStrictEqual([])
})

test("the person's /effort becomes the level, and auto hands it back to the next step", async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  await effortCommand(fake, 'high', PERSON)
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=high session=high pending=false last=medium' })
  await effortCommand(fake, 'auto', PERSON)
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=unknown session=high pending=false last=medium' })
  await step(fake, { turnId: 't2', index: 0, effort: 'low' })
  expect(fake.sent).toStrictEqual(['medium', 'low'])
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=low session=low pending=false last=low' })
})

test('a level chosen another way is taken from the step', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  await step(fake, { turnId: 't2', index: 0, effort: 'low' })
  fake.settings = { effortLevel: 'low' }
  await compact(fake, { trigger: 'manual' })
  await settle()
  expect(fake.efforts).toStrictEqual(['effort max', 'compact', 'effort low'])
})

test('an unknown /effort argument and an own echo leave the original', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  await effortCommand(fake, 'nonsense', PERSON)
  await effortCommand(fake, 'nonsense', OWN)
  await effortCommand(fake, 'auto', OWN)
  await effortCommand(fake, 'max', OWN)
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=medium session=max pending=false last=medium' })
})

test('the default and an unknown action answer without changing the mode', async () => {
  const fake = setup()
  await start(fake)
  expect(await action(fake, '')).toStrictEqual({ text: 'dynamic=on original=unknown session=unknown pending=false last=none' })
  expect(await action(fake, 'sideways')).toStrictEqual({ text: 'Usage: /dynamic-effort on|off|status' })
  expect(fake.store.size).toBe(0)
})

test('the compaction guideline follows the focus and leaves subagent compactions alone', async () => {
  const fake = setup()
  await start(fake)
  expect(await compactInstructions(fake, { trigger: 'auto' })).toStrictEqual({
    instructions: 'Also list approaches that were tried and abandoned, with the error or reason, so they are not repeated.',
  })
  expect(await compactInstructions(fake, { trigger: 'manual', instructions: 'Keep the API notes' })).toStrictEqual({
    instructions: 'Keep the API notes\n\nAlso list approaches that were tried and abandoned, with the error or reason, so they are not repeated.',
  })
  expect(await compactInstructions(fake, { trigger: 'auto', agentId: 'agent-1', instructions: 'own' })).toStrictEqual({ instructions: 'own' })
})

test('an in-process /clear keeps the level', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  await sessionStart(fake, 'clear')
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=medium session=unknown pending=false last=none' })
})

test('a budget passes through, is not recorded and clears the hint', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  expect(hint(fake)).toStrictEqual({ props: { hint: '? for shortcuts', tail: '🧠 medium' } })
  await step(fake, { turnId: 't1', index: 1, effort: 4096 })
  expect(fake.sent).toStrictEqual(['medium', 4096])
  expect(hint(fake)).toStrictEqual({ props: { hint: '? for shortcuts', tail: undefined } })
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=medium session=medium pending=false last=none' })
})

test('the hint shows the sent level, keeps another tail and reads static while off', async () => {
  const fake = setup()
  await start(fake)
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  expect(hint(fake, 'other')).toStrictEqual({ props: { hint: '? for shortcuts', tail: 'other · 🧠 medium' } })
  await action(fake, 'off')
  expect(hint(fake)).toStrictEqual({ props: { hint: '? for shortcuts', tail: '🧠 static' } })
})

test('the shift+tab key hint is dropped and the effort joined into the line', async () => {
  const fake = setup()
  await start(fake)
  const render = (text: string, tail?: string): unknown =>
    invoke(fake, 'ui.render', [{ props: { hint: text, tail, isWorking: false } }, (rendered: unknown) => rendered], 'PromptHint')
  expect(render('(shift+tab to cycle)')).toStrictEqual({ props: { hint: '', tail: undefined, isWorking: false } })
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  expect(render('(shift+tab to cycle) · ← for agents')).toStrictEqual({
    props: { hint: '← for agents · 🧠 medium', tail: undefined, isWorking: false },
  })
  expect(render('auto mode on (shift+tab to cycle) · esc to interrupt', 'other')).toStrictEqual({
    props: { hint: 'auto mode on · esc to interrupt · other · 🧠 medium', tail: undefined, isWorking: false },
  })
  expect(render('(shift+tab to cycle)')).toStrictEqual({ props: { hint: '🧠 medium', tail: undefined, isWorking: false } })
  expect(render('? for shortcuts')).toStrictEqual({ props: { hint: '? for shortcuts', tail: '🧠 medium', isWorking: false } })
})

test('unchanged events are passed on without a copy', async () => {
  const fake = setup()
  await start(fake)
  const budget = { turnId: 't1', index: 0, model: 'opus', effort: 4096 }
  await drain(invoke(fake, 'turn.step', [budget, (event: unknown) => model(fake, Object(event))]))
  expect(fake.inputs[0]).toBe(budget)
  const rendered = { props: { hint: '? for shortcuts' } }
  expect(invoke(fake, 'ui.render', [rendered, (passed: unknown) => passed], 'PromptHint')).toBe(rendered)
  const named = { turnId: 't1', index: 1, model: 'opus', effort: 'medium' }
  await drain(invoke(fake, 'turn.step', [named, (event: unknown) => model(fake, Object(event))]))
  expect(fake.inputs[1]).toBe(named)
})

test('steps make no host call', async () => {
  const fake = setup()
  await start(fake)
  const before = fake.hostCalls
  await step(fake, { turnId: 't1', index: 0, effort: 'medium' })
  await step(fake, { turnId: 't1', index: 1, effort: 'medium' })
  expect(fake.hostCalls - before).toBe(0)
})

test('a step reaches the model without waiting for a microtask', async () => {
  const fake = setup()
  await start(fake)
  const stream = invoke(fake, 'turn.step', [{ turnId: 't1', index: 0, model: 'opus', effort: 'medium' }, (event: Step) => model(fake, event)])
  if (!isGenerator(stream)) {
    throw new Error('not a stream')
  }
  void stream.next()
  expect(fake.sent).toStrictEqual(['medium'])
})

test('the per-step cost stays constant as a session grows', async () => {
  await timed(SMALL_SESSION)
  const small = await timed(SMALL_SESSION)
  const large = await timed(LARGE_SESSION)
  expect(large / small).toBeLessThan(MAX_SCALING)
})

test('steps keep the heap flat', async () => {
  const fake = setup()
  fake.sent.push = (): number => 0
  fake.inputs.push = (): number => 0
  await start(fake)
  await steps(fake, SMALL_SESSION)
  collect()
  const before = getHeapStatistics().used_heap_size
  await steps(fake, HEAP_STEPS)
  collect()
  expect(getHeapStatistics().used_heap_size - before).toBeLessThan(MAX_HEAP_GROWTH)
}, 30_000)

test('a resumed session keeps its switch, a new session starts on', async () => {
  const fake = setup()
  await start(fake)
  await action(fake, 'off')
  await settle()
  expect([...fake.store]).toStrictEqual([['off:session-1', true]])
  await sessionStart(fake, 'resume')
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=off original=unknown session=unknown pending=false last=none' })
  fake.sessionId = 'session-2'
  await sessionStart(fake, 'clear')
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=unknown session=unknown pending=false last=none' })
})

test('switching back on forgets only its own session, and an unexpected value leaves a session on', async () => {
  const fake = setup()
  fake.store.set('off:old', true)
  fake.store.set('off:session-1', true)
  await start(fake)
  await action(fake, 'on')
  await settle()
  expect([...fake.store.keys()]).toStrictEqual(['off:old'])
  fake.store.set('off:session-1', 'corrupt')
  await sessionStart(fake, 'resume')
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=unknown session=unknown pending=false last=none' })
})

test('the off keys keep only the most recent sessions and leave other keys alone', async () => {
  const fake = setup()
  fake.store.set('other', 1)
  fake.store.set('off:session-1', true)
  Array.from({ length: 32 }, (_, index) => fake.store.set(`off:old-${index}`, true))
  await start(fake)
  await action(fake, 'off')
  await settle()
  const keys = [...fake.store.keys()]
  expect(keys).toHaveLength(33)
  expect(keys.slice(0, 2)).toStrictEqual(['other', 'off:old-1'])
  expect(keys.at(-1)).toBe('off:session-1')
})

test('one failed delete of an old switch leaves the other deletes running and is logged', async () => {
  const fake = setup()
  fake.store.set('off:old-a', true)
  fake.store.set('off:old-b', true)
  fake.store.set('off:old-c', true)
  Array.from({ length: 31 }, (_, index) => fake.store.set(`off:keep-${index}`, true))
  fake.failingKeys = new Set(['off:old-a'])
  await start(fake)
  await action(fake, 'off')
  await settle(10)
  expect([...fake.store.keys()].slice(0, 2)).toStrictEqual(['off:old-a', 'off:keep-0'])
  expect(fake.logs).toStrictEqual(['dynamic-effort: saving the switch failed: pruning old switches failed: locked off:old-a'])
})

test('a store that fails rejects the start, leaves the session on and logs the unsaved switch', async () => {
  const fake = setup()
  fake.rejectStore = true
  await expect(start(fake)).rejects.toThrow('starting the session failed: unavailable')
  expect(fake.registered).toBe(1)
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=unknown session=unknown pending=false last=none' })
  await action(fake, 'off')
  await step(fake, { turnId: 't1', index: 0 })
  await action(fake, 'on')
  fake.rejectStore = false
  await sessionStart(fake, 'clear')
  fake.rejectStore = true
  await action(fake, 'off')
  await settle()
  expect(fake.store.size).toBe(0)
  expect(fake.logs).toStrictEqual(['dynamic-effort: saving the switch failed: unavailable'])
})

test('a switch read for a reset that a newer one replaced is dropped', async () => {
  const fake = setup()
  fake.store.set('off:session-1', true)
  void start(fake)
  fake.store.delete('off:session-1')
  await sessionStart(fake, 'resume')
  await settle()
  expect(await action(fake, 'status')).toStrictEqual({ text: 'dynamic=on original=unknown session=unknown pending=false last=none' })
})
