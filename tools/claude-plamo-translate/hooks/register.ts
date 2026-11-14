// This TypeScript file is executed by the Claude Code mod engine (and with Bun under vitest).
import type { EngineInterface, PluginOptions, PromptComposeResult, PromptSubmitInput, Register } from 'claude-code'

import type { Dialogs } from '../types'
import { translateQuestions } from './questions'
import { isInLanguage, plamoPrompt, translateMarkdown } from './translate'
import type { Translator } from './translate'

interface Settings {
  readonly replyLanguage: string
  readonly promptLanguage: string
  readonly port: number
}

interface Server {
  starting: Promise<boolean> | null
  warned: boolean
  // Set when a start never became ready: later calls give up at once instead of waiting again.
  failed: boolean
}

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

const server: Server = { starting: null, warned: false, failed: false }
// Source text -> its translation, newest last. The terminal never redraws a row it has printed,
// so a block is drawn only once its translation is ready; turn.step starts the work early.
const replies: Map<string, Promise<string>> = new Map()

const settingsOf = (options: PluginOptions): Settings => ({
  replyLanguage: String(options.replyLanguage),
  promptLanguage: String(options.promptLanguage),
  port: Number(options.port),
})

const completionText = (body: CompletionBody): string | null => {
  const text = body.choices?.[0]?.text
  return typeof text === 'string' ? text : null
}

const maxTokens = (text: string): number => Math.min(MAX_TOKENS, MIN_TOKENS + text.length * TOKENS_PER_CHAR)

async function isHealthy(engine: EngineInterface, port: number): Promise<boolean> {
  try {
    return (await engine.http.fetch(`${HOST}:${port}/v1/models`)).ok
  } catch {
    // A refused connection is the "not running" answer, not a failure to report.
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

function translatorFor(engine: EngineInterface, port: number, lang: string): Translator {
  return async text => ((await ensureServer(engine, port)) ? complete(engine, port, plamoPrompt(text, lang), text) : warnOnce(engine))
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

// Map#set returns the map, which is always truthy.
function rememberTranslation(source: string, translation: Promise<string>): Promise<string> {
  if (replies.size >= MAX_REPLIES) {
    replies.delete(replies.keys().next().value ?? '')
  }
  return replies.set(source, translation) && translation
}

// Keyed by the trimmed text: a response's answer and its drawn block differ in edge whitespace.
const translationOf = (engine: EngineInterface, settings: Settings, source: string): Promise<string> =>
  replies.get(source.trim()) ??
  rememberTranslation(source.trim(), translateMarkdown(source.trim(), translatorFor(engine, settings.port, settings.replyLanguage)))

const wantsReply = (text: string, isSummary: true | undefined, settings: Settings): boolean =>
  isSummary === undefined && needsTranslation(text, settings.replyLanguage)

// Keeps the newest texts only: the map lives as long as the session.
const withTexts = (dialogs: Dialogs, translated: Dialogs): Dialogs =>
  Object.fromEntries([...Object.entries(dialogs), ...Object.entries(translated)].slice(-MAX_DIALOG_TEXTS))

const textTranslator =
  (engine: EngineInterface, settings: Settings) =>
  async (text: string): Promise<string> =>
    needsTranslation(text, settings.replyLanguage)
      ? translateMarkdown(text, translatorFor(engine, settings.port, settings.replyLanguage))
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
      ? next({
          ...event,
          text: await translateMarkdown(event.text, translatorFor(engine, settings.port, settings.promptLanguage)),
        })
      : next(event),
  ).catch((_engine, event, next) => next(event))

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
      ? next({ ...event, props: { ...event.props, text: await translationOf(engine, settings, event.props.text) } })
      : next(event),
  )
}
