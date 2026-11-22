import { expect, test, vi } from "vitest";
import fetchRetry, { BACKOFF_MS, install, isStaleConnection, withRetry } from "./index.ts";

const ok = (): Response => new Response("ok");

const noWait = async (): Promise<void> => {
  await Promise.resolve();
};

/** The shape Pi reports when Cloudflare's edge has already dropped a pooled socket. */
const socketClosed = (): Error =>
  Object.assign(new Error("Connection error."), {
    cause: Object.assign(new Error("other side closed"), { code: "UND_ERR_SOCKET" }),
  });

test("recognises the failures the gateway produced", () => {
  expect(isStaleConnection(socketClosed())).toBe(true);
  expect(isStaleConnection(new Error("Connection error.: read ECONNRESET"))).toBe(true);
  expect(
    isStaleConnection(Object.assign(new Error("boom"), { code: "UND_ERR_CONNECT_TIMEOUT" })),
  ).toBe(true);
});

test("ignores a failure that a fresh connection cannot fix", () => {
  expect(isStaleConnection(new Error('429: {"httpCode":429,"message":"rate limiting"}'))).toBe(
    false,
  );
  expect(isStaleConnection("read ECONNRESET")).toBe(false);
  expect(isStaleConnection(undefined)).toBe(false);
  expect(
    isStaleConnection(
      Object.assign(new Error("a"), {
        cause: Object.assign(new Error("b"), { cause: new Error("c") }),
      }),
    ),
  ).toBe(false);
});

test("gives up walking a cause chain that is deeper than a transport failure is", () => {
  let chained = new Error("deep");
  for (let depth = 0; depth < 6; depth += 1) {
    chained = Object.assign(new Error(`wrap ${depth}`), { cause: chained });
  }

  expect(isStaleConnection(chained)).toBe(false);
});

test("retries a stale socket and returns the fresh response", async () => {
  const waits: number[] = [];
  const inner = vi
    .fn<typeof fetch>()
    .mockRejectedValueOnce(socketClosed())
    .mockResolvedValueOnce(ok());
  const retried = withRetry(inner, {
    wait: async (ms: number) => {
      waits.push(ms);
      await noWait();
    },
  });

  const response = await retried("https://example.test/v1/chat", { method: "POST", body: "{}" });

  expect(await response.text()).toBe("ok");
  expect(inner).toHaveBeenCalledTimes(2);
  expect(waits).toEqual([BACKOFF_MS[0]]);
});

test("waits its own backoff when the caller passes none", async () => {
  const inner = vi
    .fn<typeof fetch>()
    .mockRejectedValueOnce(socketClosed())
    .mockResolvedValueOnce(ok());

  const response = await withRetry(inner)("https://example.test");

  expect(await response.text()).toBe("ok");
  expect(inner).toHaveBeenCalledTimes(2);
});

test("gives up after the last attempt", async () => {
  const inner = vi.fn<typeof fetch>().mockRejectedValue(socketClosed());

  await expect(
    withRetry(inner, { attempts: 1, wait: noWait })("https://example.test"),
  ).rejects.toThrow("Connection error.");
  expect(inner).toHaveBeenCalledTimes(2);
});

test("does not retry an answer that is not a dead socket", async () => {
  const inner = vi.fn<typeof fetch>().mockRejectedValue(new Error("429: rate limiting"));

  await expect(withRetry(inner, { wait: noWait })("https://example.test")).rejects.toThrow("429");
  expect(inner).toHaveBeenCalledTimes(1);
});

test("does not retry a body that cannot be sent twice", async () => {
  const inner = vi.fn<typeof fetch>().mockRejectedValue(socketClosed());

  await expect(
    withRetry(inner, { wait: noWait })("https://example.test", { body: new ReadableStream() }),
  ).rejects.toThrow("Connection error.");
  expect(inner).toHaveBeenCalledTimes(1);
});

test("does not retry after the caller aborted", async () => {
  const inner = vi.fn<typeof fetch>().mockRejectedValue(socketClosed());

  await expect(
    withRetry(inner, { wait: noWait })("https://example.test", { signal: AbortSignal.abort() }),
  ).rejects.toThrow("Connection error.");
  expect(inner).toHaveBeenCalledTimes(1);
});

test("does not retry a Request, whose body is already used up", async () => {
  const inner = vi.fn<typeof fetch>().mockRejectedValue(socketClosed());

  await expect(
    withRetry(inner, { wait: noWait })(new Request("https://example.test", { method: "POST" })),
  ).rejects.toThrow("Connection error.");
  expect(inner).toHaveBeenCalledTimes(1);
});

test("installs once per holder", () => {
  const holder = { fetch: (async () => ok()) as typeof fetch };

  expect(install(holder, { wait: noWait })).toBe(true);
  const wrapped = holder.fetch;
  expect(install(holder, { wait: noWait })).toBe(false);
  expect(holder.fetch).toBe(wrapped);
});

test("patches the global fetch that the provider client reads", () => {
  const original = globalThis.fetch;
  try {
    fetchRetry({} as unknown as Parameters<typeof fetchRetry>[0]);

    expect(globalThis.fetch).not.toBe(original);
  } finally {
    globalThis.fetch = original;
  }
});
