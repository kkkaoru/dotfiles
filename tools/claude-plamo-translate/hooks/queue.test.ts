// This TypeScript file is executed with Bun.
import { expect, test, vi } from 'vitest'

import { newestFirst } from './queue'

interface Gate {
  readonly promise: Promise<string>
  readonly open: (value: string) => void
}

function gate(): Gate {
  const opener: { open: (value: string) => void } = { open: () => undefined }
  const promise = new Promise<string>(resolve => {
    opener.open = resolve
  })
  return { promise, open: value => opener.open(value) }
}

test('runs at most the limit at once and gives a free slot to the newest waiting task', async () => {
  const enqueue = newestFirst(2)
  const started: string[] = []
  const gates = { a: gate(), b: gate(), c: gate(), d: gate() }
  const run = (name: 'a' | 'b' | 'c' | 'd'): Promise<string> =>
    enqueue(() => {
      started.push(name)
      return gates[name].promise
    })
  const results = [run('a'), run('b'), run('c'), run('d')]
  await vi.waitFor(() => expect(started).toStrictEqual(['a', 'b']))
  gates.a.open('A')
  await vi.waitFor(() => expect(started).toStrictEqual(['a', 'b', 'd']))
  gates.b.open('B')
  gates.d.open('D')
  await vi.waitFor(() => expect(started).toStrictEqual(['a', 'b', 'd', 'c']))
  gates.c.open('C')
  expect(await Promise.all(results)).toStrictEqual(['A', 'B', 'C', 'D'])
})

test('a failing task frees its slot and rejects to its caller', async () => {
  const enqueue = newestFirst(1)
  const failed = enqueue(() => Promise.reject(new Error('server down')))
  const next = enqueue(async () => 'ok')
  await expect(failed).rejects.toThrow('server down')
  expect(await next).toBe('ok')
})
