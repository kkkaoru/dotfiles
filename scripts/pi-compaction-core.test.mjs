import assert from "node:assert/strict";
import { realpathSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import {
  installCompactionSingleFlight,
  installCompactionPreparationReliability,
  installCompactionEventReliability,
  waitForCancellation,
} from "./pi-compaction-singleflight.mjs";

// Integration tests use the installed, unbundled Pi and never persist session files.
const root =
  process.env.PI_TEST_PACKAGE_ROOT ??
  dirname(dirname(dirname(realpathSync(join(homedir(), ".bun/bin/pi")))));
const { AgentSession, SessionManager } = await import(pathToFileURL(join(root, "dist/index.js")));
installCompactionSingleFlight(AgentSession);
installCompactionPreparationReliability(AgentSession);
installCompactionEventReliability(AgentSession);

function harness(stream) {
  const session = Object.create(AgentSession.prototype);
  const manager = SessionManager.inMemory();
  manager.appendMessage({
    role: "user",
    content: "Original request " + "history ".repeat(100),
    timestamp: 1,
  });
  manager.appendMessage({ role: "user", content: "Retained request", timestamp: 2 });
  const events = [];
  const hooks = [];
  session.sessionManager = manager;
  session._isAgentRunActive = false;
  session._eventListeners = [(event) => events.push(event)];
  session.agent = {
    state: {
      model: { id: "test", reasoning: true, maxTokens: 16384 },
      thinkingLevel: "max",
      messages: [],
    },
    streamFunction: stream,
    abort() {},
    hasQueuedMessages: () => true,
  };
  session.abortRetry = () => {};
  session.abortBranchSummary = () => {};
  session.settingsManager = {
    getCompactionSettings: () => ({ enabled: true, reserveTokens: 16384, keepRecentTokens: 1 }),
    getRetrySettings: () => ({ enabled: false, maxRetries: 0 }),
  };
  session._getSummarizationRequestAuth = async (model) => ({ model, apiKey: "test" });
  session._summarizationRetryCallbacks = () => undefined;
  session._extensionRunner = {
    hasHandlers: () => true,
    async emit(event) {
      hooks.push(event);
    },
  };
  return { session, manager, events, hooks };
}

function response(stopReason) {
  return {
    role: "assistant",
    content: [{ type: "text", text: "Concise checkpoint" }],
    stopReason,
    usage: {
      input: 10,
      output: 5,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 15,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
  };
}

test("unresponsive provider cannot hold manual cancellation or persist a late result", async () => {
  const reply = Promise.withResolvers();
  const started = Promise.withResolvers();
  const { session, manager } = harness(() => {
    started.resolve();
    return { result: () => reply.promise };
  });
  const run = session.compact();
  const rejected = assert.rejects(run, /cancelled/);
  await started.promise;
  session.abortCompaction();
  await rejected;
  assert.equal(session.isIdle, true);
  reply.resolve(response("stop"));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
});

test("unresponsive provider cannot hold automatic cancellation", async () => {
  const reply = Promise.withResolvers();
  const started = Promise.withResolvers();
  const { session, manager } = harness(() => {
    started.resolve();
    return { result: () => reply.promise };
  });
  const run = session._runAutoCompaction("threshold", false);
  await started.promise;
  session.abortCompaction();
  assert.equal(await run, false);
  assert.equal(session.isIdle, true);
  reply.reject(new Error("late transport failure"));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
});

test("provider-only aborted response is not saved even if it contains partial text", async () => {
  const { session, manager, events } = harness(() => ({ result: async () => response("aborted") }));
  await assert.rejects(session.compact(), /cancelled/);
  assert.equal(events.at(-1).aborted, true);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
  assert.equal(session.isIdle, true);
});

test("provider-only automatic abort is cancellation without retry or checkpoint", async () => {
  const { session, manager, events } = harness(() => ({ result: async () => response("aborted") }));
  assert.equal(await session._runAutoCompaction("overflow", true), false);
  assert.equal(events.at(-1).aborted, true);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
});

test("a cancelled auto auth run can be followed by a successful manual compaction", async () => {
  const { session, manager } = harness(() => ({ result: async () => response("stop") }));
  const normalAuth = session._getSummarizationRequestAuth;
  const auth = Promise.withResolvers();
  session._getSummarizationRequestAuth = () => auth.promise;
  const run = session._runAutoCompaction("threshold", false);
  session.abortCompaction();
  await run;
  session._getSummarizationRequestAuth = normalAuth;
  auth.reject(new Error("late auth rejection after immediate cancellation"));
  await new Promise((resolve) => setImmediate(resolve));
  await session.compact();
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 1);
  assert.equal(session.isIdle, true);
});

test("manual auth cancellation settles while auth is still pending", async () => {
  const { session, manager, events } = harness(() => {
    throw new Error("must not generate");
  });
  const auth = Promise.withResolvers();
  session._getSummarizationRequestAuth = () => auth.promise;
  const run = session.compact();
  const rejection = assert.rejects(run, /cancelled/);
  await new Promise((resolve) => setImmediate(resolve));
  session.abortCompaction();
  await rejection;
  assert.equal(session.isIdle, true);
  assert.equal(events.at(-1).aborted, true);
  auth.reject(new Error("late auth failure"));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
});

test("cancel from start notification survives core controller reassignment", async () => {
  const { session, manager, events } = harness(() => {
    throw new Error("must not generate");
  });
  session._eventListeners.push((event) => {
    if (event.type === "compaction_start") session.abortCompaction();
  });
  assert.equal(await session._runAutoCompaction("threshold", false), false);
  assert.equal(events.at(-1).aborted, true);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
  assert.equal(session.isIdle, true);
});

test("whitespace summary cannot replace context", async () => {
  const { session, manager } = harness(() => ({
    result: async () => ({ ...response("stop"), content: [{ type: "text", text: "  \n " }] }),
  }));
  await assert.rejects(session.compact(), /empty summary/);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
});

test("empty summary cannot replace context", async () => {
  const { session, manager } = harness(() => ({
    result: async () => ({ ...response("stop"), content: [] }),
  }));
  await assert.rejects(session.compact(), /empty summary/);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
  assert.equal(session.isIdle, true);
});

test("automatic auth cancellation settles before auth and prevents later persistence", async () => {
  const calls = [];
  const { session, manager } = harness(() => {
    calls.push("request");
    return { result: async () => response("stop") };
  });
  const auth = Promise.withResolvers();
  session._getSummarizationRequestAuth = () => auth.promise;
  const run = session._runAutoCompaction("threshold", false);
  await Promise.resolve();
  session.abortCompaction();
  assert.equal(await run, false);
  assert.equal(session.isIdle, true);
  auth.resolve({ model: session.model, apiKey: "test" });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(calls, []);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
});

test("actual split-turn generator includes history, prefix and previous summary in one request", async () => {
  const requests = [];
  const { session } = harness((_model, context, options) => {
    requests.push({ context, options });
    return { result: async () => response("stop") };
  });
  const result = await session._runDefaultCompaction(
    {
      firstKeptEntryId: "kept",
      tokensBefore: 100,
      messagesToSummarize: [{ role: "user", content: "History marker", timestamp: 1 }],
      turnPrefixMessages: [{ role: "user", content: "Prefix marker", timestamp: 2 }],
      previousSummary: "Previous marker",
      isSplitTurn: true,
      fileOps: { read: new Set(["read.ts"]), edited: new Set(["edit.ts"]), written: new Set() },
      settings: { reserveTokens: 16384 },
    },
    session.model,
    "test",
    undefined,
    undefined,
    new AbortController().signal,
    undefined,
    "manual",
  );
  assert.equal(requests.length, 1);
  const prompt = JSON.stringify(requests[0].context);
  assert.match(prompt, /History marker/);
  assert.match(prompt, /Prefix marker/);
  assert.match(prompt, /Previous marker/);
  assert.match(prompt, /currently retained turn/);
  assert.equal(result.firstKeptEntryId, "kept");
  assert.match(result.summary, /edit.ts/);
  assert.equal(requests[0].options.reasoning, "max");
});

test("actual automatic cancellation avoids persistence and continuation", async () => {
  const { session, manager, events, hooks } = harness(() => ({
    result: async () => {
      session.abortCompaction();
      throw new Error("transport closed");
    },
  }));
  assert.equal(await session._runAutoCompaction("overflow", true), false);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
  assert.equal(events.at(-1).aborted, true);
  assert.equal(hooks.at(-1).aborted, true);
  assert.equal(session.isIdle, true);
});

test("actual manual compaction persists once and exposes idle before end notification", async () => {
  const calls = [];
  const { session, manager, events } = harness((_model, _context, options) => {
    calls.push(options);
    return { result: async () => response("stop") };
  });
  const idleAtEnd = [];
  session._eventListeners.push((event) => {
    if (event.type === "compaction_end") idleAtEnd.push(session.isIdle);
  });
  const result = await session.compact();
  assert.equal(result.summary.startsWith("Concise checkpoint"), true);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 1);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].reasoning, "max");
  assert.equal(calls[0].maxTokens, 13107);
  assert.deepEqual(idleAtEnd, [true]);
  assert.equal(events.at(-1).aborted, false);
  await session.waitForIdle();
});

test("actual auto compaction persists and requests queued-message continuation", async () => {
  const { session, manager } = harness(() => ({ result: async () => response("stop") }));
  assert.equal(await session._runAutoCompaction("threshold", false), true);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 1);
  assert.equal(session.isIdle, true);
  await session.waitForIdle();
});

test("actual truncated compaction is rejected without a checkpoint and releases state", async () => {
  const { session, manager, events } = harness(() => ({ result: async () => response("length") }));
  await assert.rejects(session.compact(), /incomplete/);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
  assert.equal(events.at(-1).aborted, false);
  assert.equal(session.isIdle, true);
});

test("a hanging session_before_compact hook is aborted and releases input", async () => {
  const { session, manager, events } = harness(() => ({ result: async () => response("stop") }));
  session._extensionRunner = {
    hasHandlers: () => true,
    emit(event) {
      if (event.type !== "session_before_compact") return undefined;
      return waitForCancellation(new Promise(() => {}), event.signal);
    },
  };
  const run = session.compact();
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(session._compactionAbortController !== undefined, true);
  assert.equal(
    events.some((event) => event.type === "compaction_start"),
    true,
  );
  session.abortCompaction();
  await assert.rejects(run, /cancelled|aborted/i);
  assert.equal(session.isIdle, true);
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
});

test("actual manual cancellation normalizes a provider's ordinary rejection", async () => {
  const { session, manager, events, hooks } = harness((_model, _context, options) => ({
    result: async () => {
      session.abortCompaction();
      assert.equal(options.signal.aborted, true);
      throw new Error("transport closed");
    },
  }));
  await assert.rejects(session.compact());
  assert.equal(manager.getEntries().filter((entry) => entry.type === "compaction").length, 0);
  assert.equal(events.at(-1).aborted, true);
  assert.equal(hooks.at(-1).aborted, true);
  assert.equal(session.isIdle, true);
});
