// This TypeScript file is executed with Bun.
import { expect, test } from 'vitest'

import { translateQuestions } from './questions'

const bracket = async (text: string): Promise<string> => `[${text}]`

test('question, header and descriptions are translated, the label leading its description; other fields are kept', async () => {
  expect(
    await translateQuestions(
      [
        {
          question: 'Pick one?',
          header: 'Pick',
          options: [{ label: 'A', description: 'First', preview: 'code' }],
          multiSelect: true,
        },
      ],
      bracket,
    ),
  ).toStrictEqual([
    {
      question: '[Pick one?]',
      header: '[Pick]',
      options: [{ label: 'A', description: '[A] — [First]', preview: 'code' }],
      multiSelect: true,
    },
  ])
})

test('a header translated past twelve characters keeps the source', async () => {
  expect(await translateQuestions([{ question: 'Q', header: 'Approach' }], async text => `${text} translated`)).toStrictEqual([
    { question: 'Q translated', header: 'Approach' },
  ])
})

test('absent fields stay absent', async () => {
  expect(await translateQuestions([{ question: 'Q', options: [{ label: 'A' }] }], bracket)).toStrictEqual([
    { question: '[Q]', options: [{ label: 'A' }] },
  ])
})

test('a label that stays as it is does not lead its description', async () => {
  expect(
    await translateQuestions([{ question: 'Q', options: [{ label: 'OK', description: 'Fine' }] }], async text =>
      text === 'OK' ? 'OK' : `[${text}]`,
    ),
  ).toStrictEqual([{ question: '[Q]', options: [{ label: 'OK', description: '[Fine]' }] }])
})

test('values that are not objects pass through', async () => {
  expect(await translateQuestions(['plain', null, { question: 'Q', options: ['x'] }], bracket)).toStrictEqual([
    'plain',
    null,
    { question: '[Q]', options: ['x'] },
  ])
})
