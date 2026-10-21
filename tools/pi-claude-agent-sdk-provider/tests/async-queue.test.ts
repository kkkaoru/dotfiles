// This file runs with Bun.
import { expect, it } from "vitest";
import { AsyncQueue } from "../src/async-queue.ts";

it("returns buffered items in order, including undefined values", async () => {
  const queue = new AsyncQueue<string | undefined>();
  ["a", undefined].map((item) => queue.push(item));
  expect(await queue.next()).toStrictEqual({ done: false, value: "a" });
  expect(await queue.next()).toStrictEqual({ done: false, value: undefined });
});

it("delivers an item to a waiting reader", async () => {
  const queue = new AsyncQueue<string>();
  const pending = queue.next();
  queue.push("late");
  expect(await pending).toStrictEqual({ done: false, value: "late" });
});

it("finishes waiting readers on end and ignores later pushes", async () => {
  const queue = new AsyncQueue<string>();
  const pending = queue.next();
  queue.end();
  queue.push("ignored");
  expect(await pending).toStrictEqual({ done: true, value: undefined });
  expect(await queue.next()).toStrictEqual({ done: true, value: undefined });
  expect(queue.ended).toBe(true);
});

it("drains buffered items before reporting the end", async () => {
  const queue = new AsyncQueue<number>();
  queue.push(1);
  queue.end();
  const seen: number[] = [];
  for await (const item of queue) {
    seen.push(item);
  }
  expect(seen).toStrictEqual([1]);
});

it("has no return hook, so leaving a for-await loop keeps unread items", async () => {
  const queue = new AsyncQueue<number>();
  [1, 2].map((item) => queue.push(item));
  const iterator = queue[Symbol.asyncIterator]();
  expect(iterator.return).toBe(undefined);
  expect(await iterator.next()).toStrictEqual({ done: false, value: 1 });
  expect(await queue.next()).toStrictEqual({ done: false, value: 2 });
});
