// This TypeScript file is executed by the Claude Code mod engine (and with Bun under vitest).
// Translates the AskUserQuestion dialog's text; no engine calls.

// Translates one string into the reply language (the source when it needs none or failed).
export type TextTranslator = (text: string) => Promise<string>

type Fields = Record<string, unknown>

// The tool's schema caps a header at 12 characters: a longer translation keeps the source.
const HEADER_LIMIT: number = 12
const QUESTION_KEYS: readonly string[] = ['question', 'header']
const OPTION_KEYS: readonly string[] = ['label', 'description']

const isFields = (value: unknown): value is Fields => typeof value === 'object' && value !== null && !Array.isArray(value)

// Only the string fields present are translated, so no absent key is added.
const translateKeys = async (fields: Fields, keys: readonly string[], translate: TextTranslator): Promise<Fields> =>
  Object.fromEntries(
    await Promise.all(
      keys.filter(key => typeof fields[key] === 'string').map(async key => [key, await translate(String(fields[key]))]),
    ),
  )

const fitHeader = (translated: Fields, source: Fields): Fields =>
  typeof translated.header === 'string' && [...translated.header].length > HEADER_LIMIT
    ? { ...translated, header: source.header }
    : translated

const translateOption = async (option: unknown, translate: TextTranslator): Promise<unknown> =>
  isFields(option) ? { ...option, ...(await translateKeys(option, OPTION_KEYS, translate)) } : option

const translateOptions = async (question: Fields, translate: TextTranslator): Promise<Fields> =>
  Array.isArray(question.options)
    ? { options: await Promise.all(question.options.map(option => translateOption(option, translate))) }
    : {}

async function translateQuestion(question: unknown, translate: TextTranslator): Promise<unknown> {
  if (!isFields(question)) {
    return question
  }
  const [text, options] = await Promise.all([
    translateKeys(question, QUESTION_KEYS, translate),
    translateOptions(question, translate),
  ])
  return { ...question, ...fitHeader(text, question), ...options }
}

/** The dialog's questions with their text, header and option labels and descriptions translated. */
export const translateQuestions = (questions: readonly unknown[], translate: TextTranslator): Promise<unknown[]> =>
  Promise.all(questions.map(question => translateQuestion(question, translate)))
