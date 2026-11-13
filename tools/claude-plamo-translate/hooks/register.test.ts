// This TypeScript file is executed with Bun.
import { expect, test, vi } from 'vitest'

interface Hooked {
  readonly event: string
  readonly matcher: unknown
  readonly hook: Function
  catcher: Function | null
}

interface Fetched {
  readonly url: string
  readonly body: string | undefined
}

interface Fake {
  readonly hooks: Hooked[]
  readonly fetched: Fetched[]
  readonly spawned: string[][]
  readonly toasts: string[]
  readonly logs: string[]
  readonly env: Record<string, string>
  readonly files: Set<string>
  dialogs: Record<string, string> | undefined
  healthy: boolean
  healOnSleep: boolean
  gets: number
  now: number
  translation: string
}

interface FetchInit {
  readonly body?: string
}

const OPTIONS = { replyLanguage: 'ja', promptLanguage: 'en', port: 18555 }

async function setup(options: Record<string, string | number>): Promise<Fake> {
  vi.resetModules()
  const { register } = await import('./register')
  const fake: Fake = {
    hooks: [],
    fetched: [],
    spawned: [],
    toasts: [],
    logs: [],
    env: { HOME: '/Users/me' },
    files: new Set(['/Users/me/.local/bin/mlx_lm.server']),
    dialogs: undefined,
    healthy: true,
    healOnSleep: false,
    gets: 0,
    now: 0,
    translation: 'TRANSLATED',
  }
  const on = (event: string, matcherOrHook: unknown, maybeHook?: Function): unknown => {
    const hooked: Hooked = {
      event,
      matcher: typeof matcherOrHook === 'function' ? undefined : matcherOrHook,
      hook: typeof matcherOrHook === 'function' ? matcherOrHook : (maybeHook ?? (() => undefined)),
      catcher: null,
    }
    fake.hooks.push(hooked)
    return { catch: (catcher: Function) => (hooked.catcher = catcher) }
  }
  Reflect.apply(register, undefined, [on, options])
  return fake
}

function respond(fake: Fake, url: string, init: FetchInit | undefined): unknown {
  fake.fetched.push({ url, body: init?.body })
  if (url.endsWith('/v1/models')) {
    return fake.healthy ? { ok: true, text: '{}' } : Promise.reject(new Error('ECONNREFUSED'))
  }
  return { ok: true, text: JSON.stringify({ choices: [{ text: fake.translation }] }) }
}

async function* serverOutput(fake: Fake, argv: string[]): AsyncGenerator<{ text: string }> {
  fake.spawned.push(argv)
  yield { text: 'Starting httpd\n' }
}

const engine = (fake: Fake): Record<string, unknown> => ({
  http: { fetch: async (url: string, init?: FetchInit) => respond(fake, url, init) },
  clock: {
    now: async () => fake.now,
    sleep: async (ms: number) => {
      fake.now += ms
      fake.healthy ||= fake.healOnSleep
    },
  },
  env: { get: async (name: string) => fake.env[name] },
  fs: { exists: async (path: string) => fake.files.has(path) },
  process: { spawn: ({ argv }: { argv: string[] }) => serverOutput(fake, argv) },
  ui: {
    toast: (text: string) => fake.toasts.push(text),
    log: (text: string) => fake.logs.push(text),
  },
  state: {
    get: async ({ key }: { key: string }) => {
      fake.gets += 1
      return { value: key === 'dialogs' ? fake.dialogs : undefined, version: 0 }
    },
    set: async ({ key }: { key: string }, value: Record<string, never>) => {
      if (key === 'dialogs') {
        fake.dialogs = value
      }
      return { isSet: true, version: 1 }
    },
  },
})

// A ui.render hook is picked by the component its matcher names.
const matches = (hooked: Hooked, event: string, input: unknown): boolean =>
  hooked.event === event &&
  (event !== 'ui.render' || JSON.stringify(hooked.matcher) === JSON.stringify({ component: Reflect.get(Object(input), 'component') }))

const hookFor = (fake: Fake, event: string, input?: unknown): Hooked =>
  fake.hooks.find(hooked => matches(hooked, event, input)) ?? { event, matcher: undefined, hook: () => undefined, catcher: null }

const call = (fake: Fake, event: string, input: unknown, next: Function = (value: unknown) => value): Promise<unknown> =>
  Reflect.apply(hookFor(fake, event, input).hook, undefined, [engine(fake), input, next])

// Runs the turn.step hook over a response that arrived with `answer` (next is the stream beneath).
async function stepWith(fake: Fake, host: unknown, answer: string, agentId?: string): Promise<unknown> {
  const stream = Reflect.apply(hookFor(fake, 'turn.step').hook, undefined, [
    host,
    { turnId: 't1', index: 0, model: 'm', messageCount: 1, agentId },
    async function* () {
      yield { kind: 'text', text: answer }
      return { answer }
    },
  ])
  await stream.next()
  return (await stream.next()).value
}

const step = (fake: Fake, answer: string, agentId?: string): Promise<unknown> => stepWith(fake, engine(fake), answer, agentId)

const prompt = (text: string, extra: Record<string, unknown> = {}): unknown => ({
  text,
  wait: false,
  origin: { kind: 'composer' },
  ...extra,
})

test('a Japanese prompt reaches the model in English', async () => {
  const fake = await setup(OPTIONS)
  fake.translation = 'Tell me the shortcut to add a tab'
  expect(await call(fake, 'prompt.submit', prompt('タブを追加するショートカットを教えて'))).toStrictEqual({
    text: 'Tell me the shortcut to add a tab',
    wait: false,
    origin: { kind: 'composer' },
  })
  expect(JSON.parse(fake.fetched[1]?.body ?? '{}')).toStrictEqual({
    prompt:
      '<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>タブを追加するショートカットを教えて<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>',
    max_tokens: 136,
    temperature: 0,
    stop: ['<|plamo:tag|>', '<|plamo:bos|>'],
  })
})

test('an English prompt is sent as typed without asking the server', async () => {
  const fake = await setup(OPTIONS)
  expect(await call(fake, 'prompt.submit', prompt('add a tab'))).toStrictEqual({
    text: 'add a tab',
    wait: false,
    origin: { kind: 'composer' },
  })
  expect(fake.fetched).toStrictEqual([])
})

test('slash commands, meta prompts and notifications are not translated', async () => {
  const fake = await setup(OPTIONS)
  await call(fake, 'prompt.submit', prompt('日本語', { command: { name: 'goal' } }))
  await call(fake, 'prompt.submit', prompt('日本語', { isMeta: true }))
  await call(fake, 'prompt.submit', prompt('日本語', { origin: { kind: 'task-notification' } }))
  expect(fake.fetched).toStrictEqual([])
})

test('prompt translation can be turned off', async () => {
  const fake = await setup({ ...OPTIONS, promptLanguage: 'off' })
  await call(fake, 'prompt.submit', prompt('日本語'))
  expect(fake.fetched).toStrictEqual([])
})

test('a failed prompt translation sends the prompt as typed', async () => {
  const fake = await setup(OPTIONS)
  const catcher = hookFor(fake, 'prompt.submit').catcher ?? (() => undefined)
  expect(Reflect.apply(catcher, undefined, [engine(fake), prompt('日本語'), (value: unknown) => value])).toStrictEqual({
    text: '日本語',
    wait: false,
    origin: { kind: 'composer' },
  })
})

test('a stopped server is started once and the prompt is translated when it is ready', async () => {
  const fake = await setup(OPTIONS)
  fake.healthy = false
  fake.healOnSleep = true
  expect(await call(fake, 'prompt.submit', prompt('日本語'))).toStrictEqual({ text: 'TRANSLATED', wait: false, origin: { kind: 'composer' } })
  expect(fake.spawned).toStrictEqual([
    [
      '/Users/me/.local/bin/mlx_lm.server',
      '--model',
      '/Users/me/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit',
      '--port',
      '18555',
      '--trust-remote-code',
    ],
  ])
})

test('without mlx-lm the llama.cpp server is started with the GGUF model', async () => {
  const fake = await setup(OPTIONS)
  fake.healthy = false
  fake.healOnSleep = true
  fake.files.clear()
  await call(fake, 'prompt.submit', prompt('日本語'))
  expect(fake.spawned).toStrictEqual([
    [
      '/Users/me/.local/share/plamo-translate/llama.cpp/llama-server',
      '-m',
      '/Users/me/.local/share/plamo-translate/plamo-3-translate-0.8b-Q4_K_M.gguf',
      '-c',
      '16384',
      '--port',
      '18555',
    ],
  ])
})

test('on Windows the llama.cpp server is found under USERPROFILE with its .exe', async () => {
  const fake = await setup(OPTIONS)
  fake.healthy = false
  fake.healOnSleep = true
  fake.files.clear()
  delete fake.env.HOME
  Object.assign(fake.env, { USERPROFILE: 'C:\\Users\\me', OS: 'Windows_NT' })
  await call(fake, 'prompt.submit', prompt('日本語'))
  expect(fake.spawned).toStrictEqual([
    [
      'C:\\Users\\me/.local/share/plamo-translate/llama.cpp/llama-server.exe',
      '-m',
      'C:\\Users\\me/.local/share/plamo-translate/plamo-3-translate-0.8b-Q4_K_M.gguf',
      '-c',
      '16384',
      '--port',
      '18555',
    ],
  ])
})

test('a server that never answers leaves the prompt as typed and warns once', async () => {
  const fake = await setup(OPTIONS)
  fake.healthy = false
  expect(await call(fake, 'prompt.submit', prompt('日本語'))).toStrictEqual({
    text: '日本語',
    wait: false,
    origin: { kind: 'composer' },
  })
  await call(fake, 'prompt.submit', prompt('日本語'))
  expect(fake.toasts).toStrictEqual([
    'plamo-translate: the PLaMo server is not reachable; run scripts/setup-plamo-translate.sh. Text is passed through untranslated.',
  ])
})

test('a server that cannot start is logged', async () => {
  const fake = await setup(OPTIONS)
  fake.healthy = false
  const broken = { ...engine(fake), process: {
      // oxlint-disable-next-line require-yield -- a child that cannot start fails its first pull
      spawn: async function* () {
        throw new Error('ENOENT')
      },
    } }
  await Reflect.apply(hookFor(fake, 'prompt.submit').hook, undefined, [broken, prompt('日本語'), (value: unknown) => value])
  expect(fake.logs[0]?.startsWith('plamo-translate: starting the PLaMo server failed')).toBe(true)
})

test('session start warms the server', async () => {
  const fake = await setup(OPTIONS)
  expect(await call(fake, 'session.start', { source: 'startup' })).toStrictEqual({ source: 'startup' })
  await vi.waitFor(() => expect(fake.fetched.map(({ url }) => url)).toStrictEqual(['http://127.0.0.1:18555/v1/models']))
})

test('session start leaves the server alone when both directions are off', async () => {
  const fake = await setup({ ...OPTIONS, replyLanguage: 'off', promptLanguage: 'off' })
  await call(fake, 'session.start', { source: 'startup' })
  expect(fake.fetched).toStrictEqual([])
})

const completions = (fake: Fake): number => fake.fetched.filter(({ url }) => url.endsWith('/v1/completions')).length

const block = (text: string, extra: Record<string, unknown> = {}): unknown => ({
  component: 'AssistantMessage',
  props: { text, isFirstOfReply: true, ...extra },
})

test('a response is translated as it arrives and its block is drawn translated without asking again', async () => {
  const fake = await setup(OPTIONS)
  fake.translation = 'タブを追加します。'
  expect(await step(fake, 'Adds a tab.\n')).toStrictEqual({ answer: 'Adds a tab.\n' })
  expect(await call(fake, 'ui.render', block('Adds a tab.'))).toStrictEqual(block('タブを追加します。'))
  expect(completions(fake)).toBe(1)
})

test('a block drawn before its response was seen is translated before it is drawn', async () => {
  const fake = await setup(OPTIONS)
  fake.translation = '読んでいます。'
  expect(await call(fake, 'ui.render', block('Reading files.'))).toStrictEqual(block('読んでいます。'))
})

test('text already in Japanese, a subagent response and a response without text are not translated', async () => {
  const fake = await setup(OPTIONS)
  await step(fake, 'タブを追加します。')
  await step(fake, 'Adds a tab.', 'a1')
  await step(fake, '')
  expect(fake.fetched).toStrictEqual([])
})

test('a block whose translation failed is drawn as written', async () => {
  const fake = await setup(OPTIONS)
  fake.translation = ''
  expect(await call(fake, 'ui.render', block('Adds a tab.'))).toStrictEqual(block('Adds a tab.'))
})

test('a failing reply translation is logged', async () => {
  const fake = await setup(OPTIONS)
  const broken = {
    ...engine(fake),
    http: { fetch: async (url: string) => (url.endsWith('/v1/models') ? { ok: true } : Promise.reject(new Error('server down'))) },
  }
  await stepWith(fake, broken, 'Adds a tab.')
  await vi.waitFor(() => expect(fake.logs).toStrictEqual(['plamo-translate: translating the reply failed: Error: server down']))
})

test('only the newest two hundred translations are kept', async () => {
  const fake = await setup(OPTIONS)
  await Promise.all(Array.from({ length: 201 }, (_value, index) => call(fake, 'ui.render', block(`Line ${index}.`))))
  await call(fake, 'ui.render', block('Line 200.'))
  expect(completions(fake)).toBe(201)
  await call(fake, 'ui.render', block('Line 0.'))
  expect(completions(fake)).toBe(202)
})

test('summaries and Japanese blocks are drawn as written', async () => {
  const fake = await setup(OPTIONS)
  expect(await call(fake, 'ui.render', block('Adds a tab.', { isSummary: true }))).toStrictEqual(block('Adds a tab.', { isSummary: true }))
  expect(await call(fake, 'ui.render', block('タブを追加します。'))).toStrictEqual(block('タブを追加します。'))
  expect(fake.fetched).toStrictEqual([])
})

test('replies are drawn as written when they are off', async () => {
  const fake = await setup({ ...OPTIONS, replyLanguage: 'off' })
  expect(await call(fake, 'ui.render', block('Adds a tab.'))).toStrictEqual(block('Adds a tab.'))
})

test('after a start that never became ready the server is not waited for again', async () => {
  const fake = await setup(OPTIONS)
  fake.healthy = false
  await call(fake, 'ui.render', block('Adds a tab.'))
  const spawns = fake.spawned.length
  expect(await call(fake, 'ui.render', block('Other text.'))).toStrictEqual(block('Other text.'))
  expect(fake.spawned.length).toBe(spawns)
})

test('a server error answer leaves the text untranslated', async () => {
  const fake = await setup(OPTIONS)
  const failing = {
    ...engine(fake),
    http: { fetch: async (url: string) => (url.endsWith('/v1/models') ? { ok: true } : { ok: false, text: '' }) },
  }
  expect(await Reflect.apply(hookFor(fake, 'prompt.submit').hook, undefined, [failing, prompt('日本語'), (value: unknown) => value])).toStrictEqual({
    text: '日本語',
    wait: false,
    origin: { kind: 'composer' },
  })
})

test('a completion without text leaves the text untranslated', async () => {
  const fake = await setup(OPTIONS)
  const empty = {
    ...engine(fake),
    http: { fetch: async (url: string) => (url.endsWith('/v1/models') ? { ok: true } : { ok: true, text: '{"choices":[]}' }) },
  }
  expect(await Reflect.apply(hookFor(fake, 'prompt.submit').hook, undefined, [empty, prompt('日本語'), (value: unknown) => value])).toStrictEqual({
    text: '日本語',
    wait: false,
    origin: { kind: 'composer' },
  })
})

const QUESTIONS = [
  {
    question: 'Which font should Herdr use?',
    header: 'Font',
    options: [
      { label: 'Keep UDEV Gothic', description: 'Same as Ghostty.' },
      { label: 'Switch', description: 'Install a light font.' },
    ],
    multiSelect: false,
  },
]

test('a question dialog is translated when the tool is called and drawn translated', async () => {
  const fake = await setup(OPTIONS)
  fake.translation = '訳'
  expect(await call(fake, 'tool.call', { tool: 'AskUserQuestion', questions: QUESTIONS })).toStrictEqual({
    tool: 'AskUserQuestion',
    questions: QUESTIONS,
  })
  await vi.waitFor(() => expect(Object.keys(fake.dialogs ?? {}).length).toBe(6))
  expect(
    await call(fake, 'ui.render', { component: 'AskUserQuestion', props: { tool: 'AskUserQuestion', questions: QUESTIONS } }),
  ).toStrictEqual({
    component: 'AskUserQuestion',
    props: {
      tool: 'AskUserQuestion',
      questions: [
        {
          question: '訳',
          header: '訳',
          options: [
            { label: '訳', description: '訳' },
            { label: '訳', description: '訳' },
          ],
          multiSelect: false,
        },
      ],
    },
  })
})

// The model writes multiSelect before options and may leave it out; the dialog draws the input as
// the tool's schema parsed it, with its keys in schema order and the default filled in.
test('a dialog is drawn translated though its drawn questions differ from the call in key order', async () => {
  const fake = await setup(OPTIONS)
  fake.translation = '訳'
  const asCalled = [{ question: 'Which one?', header: 'Pick', options: [{ label: 'First', description: 'The first one.' }] }]
  await call(fake, 'tool.call', { tool: 'AskUserQuestion', questions: asCalled })
  await vi.waitFor(() => expect(Object.keys(fake.dialogs ?? {}).length).toBe(4))
  const asDrawn = [{ question: 'Which one?', header: 'Pick', options: [{ label: 'First', description: 'The first one.' }], multiSelect: false }]
  expect(
    await call(fake, 'ui.render', { component: 'AskUserQuestion', props: { tool: 'AskUserQuestion', questions: asDrawn } }),
  ).toStrictEqual({
    component: 'AskUserQuestion',
    props: {
      tool: 'AskUserQuestion',
      questions: [{ question: '訳', header: '訳', options: [{ label: '訳', description: '訳' }], multiSelect: false }],
    },
  })
})

test('a drawn text with no stored translation stays as asked', async () => {
  const fake = await setup(OPTIONS)
  fake.dialogs = { Font: '書体' }
  expect(
    await call(fake, 'ui.render', { component: 'AskUserQuestion', props: { tool: 'AskUserQuestion', questions: QUESTIONS } }),
  ).toStrictEqual({
    component: 'AskUserQuestion',
    props: { tool: 'AskUserQuestion', questions: [{ ...QUESTIONS[0], header: '書体' }] },
  })
})

test('a dialog already in the reply language is not stored', async () => {
  const fake = await setup(OPTIONS)
  const japanese = [{ question: 'どれにしますか？', header: 'えらぶ', options: [{ label: 'はい', description: 'そうします。' }], multiSelect: false }]
  await call(fake, 'tool.call', { tool: 'AskUserQuestion', questions: japanese })
  await vi.waitFor(() => expect(fake.gets).toBe(1))
  expect(fake.dialogs).toBe(undefined)
  expect(fake.fetched).toStrictEqual([])
})

test('a dialog is drawn as asked before its translation is ready', async () => {
  const fake = await setup(OPTIONS)
  expect(
    await call(fake, 'ui.render', { component: 'AskUserQuestion', props: { tool: 'AskUserQuestion', questions: QUESTIONS } }),
  ).toStrictEqual({ component: 'AskUserQuestion', props: { tool: 'AskUserQuestion', questions: QUESTIONS } })
})

test('dialogs are not translated when replies are off', async () => {
  const fake = await setup({ ...OPTIONS, replyLanguage: 'off' })
  await call(fake, 'tool.call', { tool: 'AskUserQuestion', questions: QUESTIONS })
  expect(fake.gets).toBe(0)
})

test('a failing dialog translation is logged', async () => {
  const fake = await setup(OPTIONS)
  const broken = {
    ...engine(fake),
    state: { get: () => Promise.reject(new Error('state down')), set: async () => ({}) },
  }
  await Reflect.apply(hookFor(fake, 'tool.call').hook, undefined, [broken, { tool: 'AskUserQuestion', questions: QUESTIONS }, (value: unknown) => value])
  await vi.waitFor(() => expect(fake.logs).toStrictEqual(['plamo-translate: translating the question failed: Error: state down']))
})

test('only the newest 200 dialog texts are kept', async () => {
  const fake = await setup(OPTIONS)
  fake.dialogs = Object.fromEntries(Array.from({ length: 200 }, (_value, index) => [`old ${index}`, '古']))
  await call(fake, 'tool.call', { tool: 'AskUserQuestion', questions: QUESTIONS })
  await vi.waitFor(() => expect(fake.gets).toBe(1))
  expect(Object.keys(fake.dialogs ?? {}).length).toBe(200)
  expect(fake.dialogs?.['old 0']).toBe(undefined)
})

test('a failing dialog hook still opens the dialog', async () => {
  const fake = await setup(OPTIONS)
  const catcher = hookFor(fake, 'tool.call').catcher ?? (() => undefined)
  expect(Reflect.apply(catcher, undefined, [engine(fake), { tool: 'AskUserQuestion', questions: QUESTIONS }, (value: unknown) => value])).toStrictEqual({
    tool: 'AskUserQuestion',
    questions: QUESTIONS,
  })
})
