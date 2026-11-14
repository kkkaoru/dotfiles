// This TypeScript file is executed by the Claude Code mod engine (and with Bun under vitest).
import type {
  EngineInterface,
  PluginOptions,
  PromptComposeResult,
  PromptSubmitInput,
  Register,
  RenderInputOf,
  RenderSurface,
} from 'claude-code'

import type { Dialogs } from '../types'
import { newestFirst } from './queue'
import { translateQuestions } from './questions'
import { isInLanguage, plamoPrompt, translateMarkdown } from './translate'
import type { Translator } from './translate'

interface Settings {
  readonly replyLanguage: string
  readonly promptLanguage: string
  readonly port: number
  readonly toastMs: number
}

interface Worker {
  running: boolean
  // Blocks on screen waiting for a translation, the newest asked last.
  readonly wanted: Set<string>
  wake: (() => void) | null
}

interface Progress {
  // Requests to the server since nothing was left running, and how many of them have finished.
  total: number
  done: number
  // Counts the runs of requests, so a timer from an earlier run stays quiet.
  run: number
  // When the toast shown last disappears; no other is shown before then.
  visibleUntil: number
}

interface Server {
  starting: Promise<boolean> | null
  warned: boolean
  // Set when a start never became ready: later calls give up at once instead of waiting again.
  failed: boolean
}

type AssistantRender = RenderInputOf<'AssistantMessage', RenderSurface>

interface CompletionChoice {
  readonly text?: unknown
}

interface CompletionBody {
  readonly choices?: readonly CompletionChoice[]
}

const OFF: string = 'off'
const HOST: string = 'http://127.0.0.1'
// Written by scripts/setup-plamo-translate.sh: mlx-lm on a Mac, llama.cpp on Linux and Windows.
const MLX_SERVER: string = '.local/bin/mlx_lm.server'
const MLX_MODEL: string = '.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit'
const LLAMA_SERVER: string = '.local/share/plamo-translate/llama.cpp/llama-server'
const GGUF_MODEL: string = '.local/share/plamo-translate/plamo-3-translate-0.8b-Q4_K_M.gguf'
// Input and output together, as the GGUF model card runs llama-server.
const LLAMA_CONTEXT: string = '16384'
const WINDOWS: string = 'Windows_NT'
// PLaMo's own markers end a translation; llama-server does not stop on them by itself.
const STOP: readonly string[] = ['<|plamo:tag|>', '<|plamo:bos|>']
const READY_TIMEOUT_MS: number = 60_000
const READY_POLL_MS: number = 500
// Every text block of the session's responses, the ones between tool calls included.
const MAX_REPLIES: number = 200
// Every question, header, label and description of the session's newest dialogs.
const MAX_DIALOG_TEXTS: number = 200
const TOKENS_PER_CHAR: number = 4
const MIN_TOKENS: number = 64
const MAX_TOKENS: number = 4096
const USER_ORIGINS: ReadonlySet<string> = new Set(['composer', 'bridge'])
const DIALOGS = { plugin: 'plamo-translate', key: 'dialogs' } satisfies { plugin: 'plamo-translate'; key: 'dialogs' }
const SETUP_HINT: string =
  'plamo-translate: the PLaMo server is not reachable; run scripts/setup-plamo-translate.sh. Text is passed through untranslated.'
const UNTRANSLATED_ID: string = 'plamo-translate:untranslated'
// Requests the server works on at once (llama-server's default slots); the rest wait newest first.
const PARALLEL_REQUESTS: number = 4
// How long an idle worker lets a burst of draws (a resumed or scrolled screen) arrive before it
// picks the newest.
const SETTLE_MS: number = 100
// A run of translations still going after this long shows a toast lasting the toastSeconds
// option, checked as often: a new one only once the last is gone, since overlapping toasts flicker.
const STATUS_MS: number = 2000
const MS_PER_SECOND: number = 1000

const server: Server = { starting: null, warned: false, failed: false }
const progress: Progress = { total: 0, done: 0, run: 0, visibleUntil: 0 }
// Prompts sent translated this session, so a recalled one comes back exactly as typed.
const TYPED_LIMIT: number = 200
// Source text -> its translation, newest last; turn.step starts a live reply's early.
const replies: Map<string, Promise<string>> = new Map()
// Prompt history keeps what the model read: translation -> the prompt as typed, newest last.
const typed: Map<string, string> = new Map()
// Source text -> its finished translation, which a draw takes without waiting.
const ready: Map<string, string> = new Map()
// A draw never waits on the server: a render's engine is cut off whenever a newer render of the
// same block supersedes it, as happens many times while a resumed or scrolled screen settles. The
// worker translates with session.start's engine and redraws once a translation lands.
const worker: Worker = { running: false, wanted: new Set(), wake: null }
// Requests to the server, newest first: a new prompt, then the blocks drawn last.
const enqueue = newestFirst(PARALLEL_REQUESTS)

const settingsOf = (options: PluginOptions): Settings => ({
  replyLanguage: String(options.replyLanguage),
  promptLanguage: String(options.promptLanguage),
  port: Number(options.port),
  toastMs: Number(options.toastSeconds) * MS_PER_SECOND,
})

const completionText = (body: CompletionBody): string | null => {
  const text = body.choices?.[0]?.text
  return typeof text === 'string' ? text : null
}

const maxTokens = (text: string): number => Math.min(MAX_TOKENS, MIN_TOKENS + text.length * TOKENS_PER_CHAR)

// What a call of a render's engine throws once a newer render of the same block replaced it.
const isSuperseded = (error: unknown): boolean => error instanceof Error && error.message.endsWith(': superseded')

async function isHealthy(engine: EngineInterface, port: number): Promise<boolean> {
  try {
    return (await engine.http.fetch(`${HOST}:${port}/v1/models`)).ok
  } catch (error) {
    // A refused connection is the "not running" answer; a superseded render must not start a server.
    if (isSuperseded(error)) {
      throw error
    }
    return false
  }
}

async function waitReady(engine: EngineInterface, port: number, deadline: number): Promise<boolean> {
  if (await isHealthy(engine, port)) {
    return true
  }
  if ((await engine.clock.now()) >= deadline) {
    return false
  }
  await engine.clock.sleep(READY_POLL_MS)
  return waitReady(engine, port, deadline)
}

const mlxArgv = (home: string, port: number): string[] => [
  `${home}/${MLX_SERVER}`,
  '--model',
  `${home}/${MLX_MODEL}`,
  '--port',
  String(port),
  '--trust-remote-code',
]

const llamaArgv = (home: string, port: number, os: string | undefined): string[] => [
  `${home}/${LLAMA_SERVER}${os === WINDOWS ? '.exe' : ''}`,
  '-m',
  `${home}/${GGUF_MODEL}`,
  '-c',
  LLAMA_CONTEXT,
  '--port',
  String(port),
]

// Windows has no HOME unless Git Bash started the session.
async function serverArgv(engine: EngineInterface, port: number): Promise<string[]> {
  const home = `${(await engine.env.get('HOME')) ?? (await engine.env.get('USERPROFILE'))}`
  return (await engine.fs.exists(`${home}/${MLX_SERVER}`))
    ? mlxArgv(home, port)
    : llamaArgv(home, port, await engine.env.get('OS'))
}

// The child lives as long as this loop: it ends with the session or a reload of the mod.
async function runServer(engine: EngineInterface, port: number): Promise<void> {
  const stream = engine.process.spawn({ argv: await serverArgv(engine, port) })
  // oxlint-disable-next-line no-await-in-loop -- the child's output arrives in order, one piece at a time
  for await (const chunk of stream) {
    engine.ui.log(`plamo-translate server: ${chunk.text.trim()}`, { to: 'debug' })
  }
  server.starting = null
}

async function startServer(engine: EngineInterface, port: number): Promise<boolean> {
  void runServer(engine, port).catch((error: unknown) => {
    server.starting = null
    engine.ui.log(`plamo-translate: starting the PLaMo server failed: ${String(error)}`, { to: 'debug' })
  })
  server.failed = !(await waitReady(engine, port, (await engine.clock.now()) + READY_TIMEOUT_MS))
  return !server.failed
}

async function ensureServer(engine: EngineInterface, port: number): Promise<boolean> {
  if (await isHealthy(engine, port)) {
    return true
  }
  if (server.failed) {
    return false
  }
  server.starting ??= startServer(engine, port)
  return server.starting
}

function warnOnce(engine: EngineInterface): null {
  if (!server.warned) {
    server.warned = true
    engine.ui.toast(SETUP_HINT)
  }
  return null
}

async function complete(engine: EngineInterface, port: number, prompt: string, text: string): Promise<string | null> {
  const response = await engine.http.fetch(`${HOST}:${port}/v1/completions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt, max_tokens: maxTokens(text), temperature: 0, stop: STOP }),
  })
  return response.ok ? completionText(JSON.parse(response.text)) : null
}

const statusLine = ({ total, done }: Progress): string => `PLaMo: translating… ${done}/${total} lines`

const isRunning = (run: number): boolean => run === progress.run && progress.done < progress.total

function showProgress(engine: EngineInterface, now: number, toastMs: number): void {
  if (now >= progress.visibleUntil) {
    engine.ui.toast(statusLine(progress), { timeoutMs: toastMs })
    progress.visibleUntil = now + toastMs
  }
}

// Every STATUS_MS while the run goes on, toasts unless one is still up; the first check comes
// after STATUS_MS, so a quick translation shows nothing.
async function toastWhileSlow(engine: EngineInterface, run: number, toastMs: number): Promise<void> {
  await engine.clock.sleep(STATUS_MS)
  if (!isRunning(run)) {
    return
  }
  showProgress(engine, await engine.clock.now(), toastMs)
  return toastWhileSlow(engine, run, toastMs)
}

function startRun(engine: EngineInterface, toastMs: number): void {
  progress.run += 1
  void toastWhileSlow(engine, progress.run, toastMs).catch((error: unknown) =>
    engine.ui.log(`plamo-translate: showing the progress failed: ${String(error)}`, { to: 'debug' }),
  )
}

// Counts one reply request toward the status; once every counted one has finished the run ends.
async function counted(engine: EngineInterface, toastMs: number, request: () => Promise<string | null>): Promise<string | null> {
  if (progress.total === 0) {
    startRun(engine, toastMs)
  }
  progress.total += 1
  try {
    return await request()
  } finally {
    progress.done += 1
    if (progress.done >= progress.total) {
      progress.total = 0
      progress.done = 0
    }
  }
}

function translatorFor(engine: EngineInterface, port: number, lang: string): Translator {
  return async text =>
    (await ensureServer(engine, port)) ? enqueue(() => complete(engine, port, plamoPrompt(text, lang), text)) : warnOnce(engine)
}

// What is shown to the person (replies, dialogs): only this direction toasts its progress; a
// prompt's translation is left out of the count.
function replyTranslatorFor(engine: EngineInterface, settings: Settings): Translator {
  const translate = translatorFor(engine, settings.port, settings.replyLanguage)
  return text => counted(engine, settings.toastMs, () => translate(text))
}

// Tells the model what survives translation, so it marks text the person must read exactly.
const untranslatedRule = (lang: string): string =>
  `Your text and AskUserQuestion dialogs are shown to the user machine-translated into \`${lang}\`. Only inline code (\`...\`) and fenced code blocks (\`\`\`) are left exactly as written. Put anything the user must read or copy verbatim in one of them: commands, file paths, identifiers, option and flag names, literal values, error messages and quoted UI text. Do not wrap ordinary prose.`

const withRule = ({ sections }: PromptComposeResult, lang: string): PromptComposeResult => ({
  sections: [...sections, { id: UNTRANSLATED_ID, text: untranslatedRule(lang), scope: 'session' }],
})

const isUserPrompt = (event: PromptSubmitInput): boolean =>
  event.command === undefined && event.isMeta === undefined && USER_ORIGINS.has(event.origin.kind)

const needsTranslation = (text: string, lang: string): boolean => lang !== OFF && !isInLanguage(text, lang)

function rememberTyped(translation: string, text: string): string {
  typed.delete(translation)
  typed.set(translation, text)
  const oldest = typed.size > TYPED_LIMIT ? typed.keys().next().value : undefined
  if (oldest !== undefined) {
    typed.delete(oldest)
  }
  return translation
}

const translatedPrompt = async (engine: EngineInterface, settings: Settings, text: string): Promise<string> =>
  rememberTyped(await translateMarkdown(text, translatorFor(engine, settings.port, settings.promptLanguage)), text)

// A slash command or the person's own draft put back is left as it is.
const wantsRecall = (text: string, settings: Settings): boolean =>
  text.trim() !== '' && !text.startsWith('/') && needsTranslation(text, settings.replyLanguage)

const recalledText = async (engine: EngineInterface, settings: Settings, text: string): Promise<string> =>
  typed.get(text) ?? translateMarkdown(text, translatorFor(engine, settings.port, settings.replyLanguage))

// Fills the box only if it still holds the entry: the person may have moved on meanwhile.
async function showRecalled(engine: EngineInterface, shown: string, text: string): Promise<void> {
  if (shown !== text && (await engine.prompt.read()).text === text) {
    await engine.prompt.fill({ text: shown })
  }
}

// Kept only while its source is remembered: one evicted meanwhile stays out.
function keepReady(source: string, text: string): string {
  if (replies.has(source)) {
    ready.set(source, text)
  }
  return text
}

// A finished translation is kept for draws; a failed one is forgotten so the next draw asks again,
// and its callers still get the error.
const settle = (source: string, translation: Promise<string>): Promise<string> =>
  translation.then(
    text => keepReady(source, text),
    (error: unknown) => {
      replies.delete(source)
      throw error
    },
  )

function rememberTranslation(source: string, translation: Promise<string>): Promise<string> {
  if (replies.size >= MAX_REPLIES) {
    const oldest = replies.keys().next().value ?? ''
    replies.delete(oldest)
    ready.delete(oldest)
  }
  const remembered = settle(source, translation)
  return replies.set(source, remembered) && remembered
}

// Keyed by the trimmed text: a response's answer and its drawn block differ in edge whitespace.
const translationOf = (engine: EngineInterface, settings: Settings, source: string): Promise<string> =>
  replies.get(source.trim()) ??
  rememberTranslation(source.trim(), translateMarkdown(source.trim(), replyTranslatorFor(engine, settings)))

// A render's engine is cut off when a newer render of the same block supersedes it, which happens
// many times while a screen settles; the failed translation is forgotten, so the render asks once
// more with its own engine (a second failure reaches the engine).
const drawnTranslation = (engine: EngineInterface, settings: Settings, text: string): Promise<string> =>
  translationOf(engine, settings, text).catch(() => translationOf(engine, settings, text))

const wantsReply = (text: string, isSummary: true | undefined, settings: Settings): boolean =>
  isSummary === undefined && needsTranslation(text, settings.replyLanguage)

// Off screen, or not laid out yet in the fullscreen terminal (which draws it again once it shows).
const outOfView = ({ props, viewport }: AssistantRender): boolean =>
  props.onScreen === null || (props.onScreen === undefined && viewport?.isFullscreen === true)

// Asks the worker for a block; asked again, the block moves to the front.
function want(text: string): void {
  worker.wanted.delete(text)
  worker.wanted.add(text)
  worker.wake?.()
  worker.wake = null
}

const newestWanted = (): string | undefined => [...worker.wanted].at(-1)

async function serveNext(engine: EngineInterface, settings: Settings): Promise<void> {
  const text = newestWanted()
  if (text === undefined) {
    await new Promise<void>(resolve => {
      worker.wake = resolve
    })
    return engine.clock.sleep(SETTLE_MS)
  }
  worker.wanted.delete(text)
  return translationOf(engine, settings, text).then(
    () => engine.ui.invalidate('ui.render'),
    (error: unknown) => workerFailed(engine, text, error),
  )
}

// A translation a superseded render had started fails with that render: the worker asks again
// with its own engine. Any other failure is logged.
function workerFailed(engine: EngineInterface, text: string, error: unknown): void {
  if (isSuperseded(error)) {
    want(text)
    return
  }
  engine.ui.log(`plamo-translate: translating a block failed: ${String(error)}`, { to: 'debug' })
}

// Runs for the session with session.start's engine, one block at a time, the newest wanted first.
async function serve(engine: EngineInterface, settings: Settings): Promise<void> {
  worker.running = true
  for (;;) {
    // oxlint-disable-next-line no-await-in-loop -- one block at a time, each the newest still wanted
    await serveNext(engine, settings)
  }
}

// A live reply's translation is already running: its draw waits for it (failing, the worker retries).
const awaitStarted = (pending: Promise<string>, source: string): Promise<string> =>
  pending.catch(() => {
    want(source.trim())
    return source
  })

// Without a worker (the mod was reloaded mid-session), the draw translates for itself.
async function askFor(engine: EngineInterface, settings: Settings, source: string): Promise<string> {
  if (!worker.running) {
    return drawnTranslation(engine, settings, source)
  }
  want(source.trim())
  return source
}

// What a block is drawn with: its finished translation; a live reply's once it lands; else, on
// screen, the block as written while the worker translates it, newest first.
async function drawnText(engine: EngineInterface, settings: Settings, event: AssistantRender): Promise<string> {
  const source = event.props.text
  const text = source.trim()
  const pending = replies.get(text)
  if (ready.has(text)) {
    return ready.get(text) ?? source
  }
  if (outOfView(event)) {
    worker.wanted.delete(text)
    return source
  }
  return pending === undefined ? askFor(engine, settings, source) : awaitStarted(pending, source)
}

// Keeps the newest texts only: the map lives as long as the session.
const withTexts = (dialogs: Dialogs, translated: Dialogs): Dialogs =>
  Object.fromEntries([...Object.entries(dialogs), ...Object.entries(translated)].slice(-MAX_DIALOG_TEXTS))

const textTranslator =
  (engine: EngineInterface, settings: Settings) =>
  async (text: string): Promise<string> =>
    needsTranslation(text, settings.replyLanguage)
      ? translateMarkdown(text, replyTranslatorFor(engine, settings))
      : text

// Stored by source text, not by the whole questions: the dialog draws the input as the tool's
// schema parsed it (keys reordered, defaults added), which never equals the call's JSON.
async function rememberDialog(engine: EngineInterface, settings: Settings, questions: readonly unknown[]): Promise<void> {
  const translate = textTranslator(engine, settings)
  const pairs: [string, string][] = []
  await translateQuestions(questions, async text => {
    const translation = await translate(text)
    pairs.push([text, translation])
    return translation
  })
  const translated = Object.fromEntries(pairs.filter(([text, translation]) => text !== translation))
  const { value } = await engine.state.get(DIALOGS)
  if (Object.keys(translated).length > 0) {
    await engine.state.set(DIALOGS, withTexts(value ?? {}, translated))
  }
}

export const register: Register = (on, options) => {
  const settings = settingsOf(options)

  on('session.start', async (engine, event, next) => {
    const started = await next(event)
    if (settings.replyLanguage !== OFF && !worker.running) {
      void serve(engine, settings).catch((error: unknown) =>
        engine.ui.log(`plamo-translate: the translation worker stopped: ${String(error)}`),
      )
    }
    if (settings.replyLanguage !== OFF || settings.promptLanguage !== OFF) {
      void ensureServer(engine, settings.port).catch((error: unknown) =>
        engine.ui.log(`plamo-translate: warming the server failed: ${String(error)}`, { to: 'debug' }),
      )
    }
    return started
  })

  on('prompt.compose', async (_engine, event, next) =>
    settings.replyLanguage === OFF ? next(event) : withRule(await next(event), settings.replyLanguage),
  )

  // The model reads the translation; the person's own text is not kept for it. A failed
  // translation must not eat the prompt, so the catch sends it as typed.
  on('prompt.submit', async (engine, event, next) =>
    isUserPrompt(event) && needsTranslation(event.text, settings.promptLanguage)
      ? next({ ...event, text: await translatedPrompt(engine, settings, event.text) })
      : next(event),
  ).catch((_engine, event, next) => next(event))

  // The history keeps the English the model read; an entry recalled with the arrows is shown as
  // it was typed, or translated back when this session did not send it.
  on('prompt.change', { origin: { kind: 'history' } }, async (engine, event, next) => {
    const changed = await next(event)
    if (wantsRecall(event.text, settings)) {
      await showRecalled(engine, await recalledText(engine, settings, event.text), event.text)
    }
    return changed
  })

  // Each response of the main loop, the text between tool calls included, is translated for the
  // screen once it has arrived whole; the model and the transcript keep the original.
  on('turn.step', async function* (engine, event, next) {
    const response = yield* next(event)
    if (event.agentId === undefined && needsTranslation(response.answer, settings.replyLanguage)) {
      void translationOf(engine, settings, response.answer).catch((error: unknown) =>
        engine.ui.log(`plamo-translate: translating the reply failed: ${String(error)}`),
      )
    }
    return response
  })

  // The dialog opens at once; its translation is drawn in place when it is ready. A failure here
  // must not keep the question from the person, so the catch lets the call go on.
  on('tool.call', { tool: 'AskUserQuestion' }, (engine, event, next) => {
    if (settings.replyLanguage !== OFF) {
      void rememberDialog(engine, settings, event.questions).catch((error: unknown) =>
        engine.ui.log(`plamo-translate: translating the question failed: ${String(error)}`),
      )
    }
    return next(event)
  }).catch((_engine, event, next) => next(event))

  // The answer comes back with the labels as drawn, so the model reads the translated choice.
  on('ui.render', { component: 'AskUserQuestion' }, async (engine, event, next) => {
    const { value } = await engine.state.get(DIALOGS)
    return value === undefined
      ? next(event)
      : next({
          ...event,
          props: { ...event.props, questions: await translateQuestions(event.props.questions, async text => value[text] ?? text) },
        })
  })

  // Draws each text block translated; the transcript keeps the original (ctrl+o).
  on('ui.render', { component: 'AssistantMessage' }, async (engine, event, next) =>
    wantsReply(event.props.text, event.props.isSummary, settings)
      ? next({ ...event, props: { ...event.props, text: await drawnText(engine, settings, event) } })
      : next(event),
  )
}
