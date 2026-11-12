// This TypeScript file is executed with Bun.
import { expect, test } from 'vitest'

import { fencedLines, isInLanguage, plamoPrompt, translateMarkdown, translateProse } from './translate'

const upper = async (text: string): Promise<string | null> => text.toUpperCase()

test('Japanese text with kana reads as Japanese', () => {
  expect(isInLanguage('タブを追加するショートカットは ⌘T です。', 'ja')).toBe(true)
})

test('English text with one quoted Japanese word does not read as Japanese', () => {
  expect(isInLanguage('Press ctrl-b then c to open a new tab, then rename it to 「作業」 later.', 'ja')).toBe(false)
})

test('Chinese text without kana does not read as Japanese', () => {
  expect(isInLanguage('这是中文', 'ja')).toBe(false)
})

test('plain English reads as English', () => {
  expect(isInLanguage('Tell me the shortcut to add a tab', 'en')).toBe(true)
})

test('a Japanese prompt does not read as English', () => {
  expect(isInLanguage('タブを追加するショートカットを教えて', 'en')).toBe(false)
})

test('text without letters needs no translation', () => {
  expect(isInLanguage('1234 + 5', 'ja')).toBe(true)
})

test('languages other than ja and en are always translated', () => {
  expect(isInLanguage('Bonjour', 'fr')).toBe(false)
})

test('the prompt asks for the polite form in Japanese', () => {
  expect(plamoPrompt('Hi', 'ja')).toBe(
    '<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>Hi<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>ja<|plamo:key|>form<|plamo:val|>polite<|plamo:msg|>',
  )
})

test('the prompt names the target language alone for English', () => {
  expect(plamoPrompt('やあ', 'en')).toBe(
    '<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>やあ<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>',
  )
})

test('code spans and URLs are sent as slots and put back', async () => {
  const sent: string[] = []
  const out = await translateProse('Run `bun run check` at https://example.com/a now', async text => {
    sent.push(text)
    return 'run {{0}} at {{1}} NOW'
  })
  expect(sent).toStrictEqual(['Run {{0}} at {{1}} now'])
  expect(out).toBe('run `bun run check` at https://example.com/a NOW')
})

test('a translation that drops a slot keeps the source', async () => {
  expect(await translateProse('Run `bun run check` now', async () => 'run check now')).toBe('Run `bun run check` now')
})

test('a translation that repeats a slot keeps the source', async () => {
  expect(await translateProse('Open `a` and `b`', async () => '{{0}} {{0}}')).toBe('Open `a` and `b`')
})

test('a failed translation keeps the source', async () => {
  expect(await translateProse('Hello', async () => null)).toBe('Hello')
})

test('an empty translation keeps the source', async () => {
  expect(await translateProse('Hello', async () => '  ')).toBe('Hello')
})

test('text made only of code is not sent', async () => {
  expect(await translateProse('`ls -la`', async () => 'never')).toBe('`ls -la`')
})

test('text holding a reserved PLaMo token is not sent', async () => {
  expect(await translateProse('see <|plamo:msg|>', async () => 'never')).toBe('see <|plamo:msg|>')
})

test('fenced blocks are marked, tildes included, and a backtick fence inside tildes does not close them', () => {
  expect(fencedLines(['a', '```sh', 'ls', '```', '~~~', '```', 'x', '~~~', 'b'])).toStrictEqual([
    false,
    true,
    true,
    true,
    true,
    true,
    true,
    true,
    false,
  ])
})

test('markdown keeps markers, blank lines, code blocks and table rules', async () => {
  const source = [
    '## Title',
    '',
    '- item one',
    '1. step',
    '> quote',
    '```js',
    'const a = 1',
    '```',
    '| key | what |',
    '|---|---|',
    '| `c` | new tab |',
  ].join('\n')
  expect(await translateMarkdown(source, upper)).toBe(
    [
      '## TITLE',
      '',
      '- ITEM ONE',
      '1. STEP',
      '> QUOTE',
      '```js',
      'const a = 1',
      '```',
      '| KEY | WHAT |',
      '|---|---|',
      '| `c` | NEW TAB |',
    ].join('\n'),
  )
})
