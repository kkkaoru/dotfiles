// This TypeScript file is executed by the Claude Code mod engine (and with Bun under vitest).
// Orders the requests to the PLaMo server; no engine calls.

export type Task<Value> = () => Promise<Value>
export type Enqueue = <Value>(task: Task<Value>) => Promise<Value>

interface Queue {
  readonly waiting: (() => void)[]
  running: number
}

function startNext(queue: Queue, limit: number): void {
  const start = queue.running < limit ? queue.waiting.pop() : undefined
  if (start !== undefined) {
    queue.running += 1
    start()
  }
}

async function runStarted<Value>(queue: Queue, limit: number, task: Task<Value>): Promise<Value> {
  try {
    return await task()
  } finally {
    queue.running -= 1
    startNext(queue, limit)
  }
}

/** Runs at most `limit` tasks at once; a free slot goes to the task queued last (newest first). */
export function newestFirst(limit: number): Enqueue {
  const queue: Queue = { waiting: [], running: 0 }
  return async task => {
    await new Promise<void>(resolve => {
      queue.waiting.push(resolve)
      startNext(queue, limit)
    })
    return runStarted(queue, limit, task)
  }
}
