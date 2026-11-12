// This TypeScript file is executed by the Claude Code mod engine (and with Bun under vitest).
// Markdown-aware text handling for PLaMo 3 Translate; no engine calls.

// One request to the model: plain text in, translated text out (null when it failed).
export type Translator = (text: string) => Promise<string | null>

interface FenceScan {
  open: string | null
  readonly inside: boolean[]
}

const FENCE: RegExp = /^(```|~~~)/
const TABLE_RULE: RegExp = /^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?$/
// Inline code, autolinks, bare URLs and link targets survive translation as {{n}} slots.
const PROTECTED: RegExp = /`[^`\n]+`|<https?:\/\/[^>\s]+>|\]\([^)\s]+\)|https?:\/\/[^\s)>\]]+/g
const SLOT: RegExp = /\{\{(\d+)\}\}/g
// Leading list/heading/quote markers stay as written; only the text after them is sent.
const MARKER: RegExp = /^(\s*(?:[-*+]\s+|\d+[.)]\s+|#{1,6}\s+|>\s*)*)(.*)$/
const HAS_LETTER: RegExp = /\p{L}/u
const RESERVED: string = '<|plamo:'

const KANA: RegExp = /[぀-ヿ]/g
const HAN: RegExp = /[㐀-鿿]/g
const NON_LATIN: RegExp = /[^\s\p{P}\p{S}\p{N} -ɏ]/gu
const LETTER: RegExp = /\p{L}/gu
const JAPANESE_SHARE: number = 0.3
const JAPANESE: ReadonlySet<string> = new Set(['ja', 'ja-easy'])
const POLITE_FORM: string = '<|plamo:key|>form<|plamo:val|>polite'

const count = (text: string, pattern: RegExp): number => text.match(pattern)?.length ?? 0

const readsAsJapanese = (text: string, letters: number): boolean =>
  count(text, KANA) > 0 && (count(text, KANA) + count(text, HAN)) / letters >= JAPANESE_SHARE

/** Whether `text` already reads as `lang` (judged for ja and en only), so translating it would only cost time. */
export function isInLanguage(text: string, lang: string): boolean {
  const letters = count(text, LETTER)
  if (letters === 0) {
    return true
  }
  if (JAPANESE.has(lang)) {
    return readsAsJapanese(text, letters)
  }
  // Any non-Latin letter (a Japanese line in an English paste) is worth translating.
  return lang === 'en' && count(text, NON_LATIN) === 0
}

/** The raw PLaMo prompt (the chat template's PFML3 form; the tokenizer adds BOS). */
export const plamoPrompt = (text: string, lang: string): string =>
  `<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>${text}<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>${lang}${lang === 'ja' ? POLITE_FORM : ''}<|plamo:msg|>`

const untranslatable = (text: string): boolean => !HAS_LETTER.test(text.replace(SLOT, '')) || text.includes(RESERVED)

// A dropped or duplicated slot would lose code, so the source is kept instead.
const slotsIntact = (out: string, kept: readonly string[]): boolean =>
  new Set([...out.matchAll(SLOT)].map(match => match[1])).size === kept.length &&
  [...out.matchAll(SLOT)].length === kept.length

const restore = (out: string | null, kept: readonly string[], source: string): string =>
  out === null || out.trim() === '' || !slotsIntact(out, kept)
    ? source
    : out.replace(SLOT, (_match, index: string) => kept[Number(index)] ?? '').trim()

function mask(text: string): { readonly masked: string; readonly kept: string[] } {
  const kept: string[] = []
  return { masked: text.replace(PROTECTED, match => `{{${kept.push(match) - 1}}}`), kept }
}

/** Translates one piece of prose, keeping code spans and URLs byte for byte. */
export async function translateProse(text: string, translate: Translator): Promise<string> {
  const { masked, kept } = mask(text)
  return untranslatable(masked) ? text : restore(await translate(masked), kept, text)
}

const translateCell = async (cell: string, translate: Translator): Promise<string> =>
  cell.trim() === '' ? cell : ` ${await translateProse(cell.trim(), translate)} `

const translateTableRow = async (line: string, translate: Translator): Promise<string> =>
  TABLE_RULE.test(line.trim())
    ? line
    : (await Promise.all(line.split('|').map(cell => translateCell(cell, translate)))).join('|')

async function translateLine(line: string, translate: Translator): Promise<string> {
  if (line.trimStart().startsWith('|')) {
    return translateTableRow(line, translate)
  }
  const [, marker = '', body = ''] = MARKER.exec(line) ?? []
  return body.trim() === '' ? line : `${marker}${await translateProse(body, translate)}`
}

// A closing fence must match the opener: ``` inside ~~~ stays code.
const closeFence = (open: string, opener: string): string | null => (open === opener ? null : open)

const nextFence = (open: string | null, opener: string): string | null =>
  open === null ? opener : closeFence(open, opener)

function scanLine(scan: FenceScan, line: string): FenceScan {
  const opener = FENCE.exec(line.trimStart())?.[1]
  scan.inside.push(scan.open !== null || opener !== undefined)
  if (opener !== undefined) {
    scan.open = nextFence(scan.open, opener)
  }
  return scan
}

/** For each line, whether it belongs to a fenced code block (fences included). */
export function fencedLines(lines: readonly string[]): boolean[] {
  const scan: FenceScan = { open: null, inside: [] }
  lines.forEach(line => scanLine(scan, line))
  return scan.inside
}

/** Translates markdown line by line; fenced code blocks pass through untouched. */
export async function translateMarkdown(text: string, translate: Translator): Promise<string> {
  const lines = text.split('\n')
  const fenced = fencedLines(lines)
  return (
    await Promise.all(lines.map((line, index) => (fenced[index] ? line : translateLine(line, translate))))
  ).join('\n')
}
