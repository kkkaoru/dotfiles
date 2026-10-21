// This file runs with Bun.

type Waiter<Item> = (result: IteratorResult<Item, undefined>) => void;

const DONE: IteratorReturnResult<undefined> = { done: true, value: undefined };

/** Unbounded single-consumer queue that buffers items until they are read. */
export class AsyncQueue<Item> implements AsyncIterableIterator<Item, undefined> {
  // Boxing keeps an `undefined` item distinguishable from an empty queue.
  readonly #items: { value: Item }[] = [];
  readonly #waiters: Waiter<Item>[] = [];
  #ended = false;

  get ended(): boolean {
    return this.#ended;
  }

  push(item: Item): void {
    if (this.#ended) {
      return;
    }
    const waiter = this.#waiters.shift();
    if (waiter === undefined) {
      this.#items.push({ value: item });
      return;
    }
    waiter({ done: false, value: item });
  }

  end(): void {
    this.#ended = true;
    for (const waiter of this.#waiters.splice(0)) {
      waiter(DONE);
    }
  }

  async next(): Promise<IteratorResult<Item, undefined>> {
    const boxed = this.#items.shift();
    if (boxed !== undefined) {
      return { done: false, value: boxed.value };
    }
    if (this.#ended) {
      return DONE;
    }
    return new Promise((resolve) => {
      this.#waiters.push(resolve);
    });
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<Item, undefined> {
    return this;
  }
}
