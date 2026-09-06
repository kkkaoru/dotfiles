import assert from "node:assert/strict";
import test from "node:test";
import {
  installCompactionSingleFlight,
  installCompactionEventReliability,
  installLatestCompactionEventEntry,
} from "./pi-compaction-singleflight.mjs";

test("a pending before-compaction hook can be cancelled without accepting its late result", async () => {
  const reply = Promise.withResolvers();
  class Runner {
    emit() {
      return reply.promise;
    }
  }
  installLatestCompactionEventEntry(Runner);
  const controller = new AbortController();
  const pending = new Runner().emit({ type: "session_before_compact", signal: controller.signal });
  const rejected = assert.rejects(pending, /cancelled/);
  controller.abort();
  await rejected;
  reply.resolve({ compaction: { summary: "late" } });
  await new Promise((resolve) => setImmediate(resolve));
});

test("empty extension checkpoint is rejected before persistence", async () => {
  class Runner {
    async emit() {
      return { compaction: { summary: "" } };
    }
  }
  installLatestCompactionEventEntry(Runner);
  await assert.rejects(new Runner().emit({ type: "session_before_compact" }), /empty summary/);
});

test("auto completion wakes a waiter after the wrapper releases its lock", async () => {
  class Session {
    _autoCompactionAbortController;
    _compactionAbortController;
    resolveIdle;
    finish;
    get isCompacting() {
      return this._autoCompactionAbortController !== undefined;
    }
    _resolveIdleWaitIfIdle() {
      if (!this.isCompacting) this.resolveIdle?.();
    }
    async compact() {
      return this._compactionAbortController.signal.aborted;
    }
    async _runAutoCompaction() {
      this._autoCompactionAbortController = new AbortController();
      try {
        await new Promise((resolve) => {
          this.finish = resolve;
        });
        return this._autoCompactionAbortController.signal.aborted;
      } finally {
        this._autoCompactionAbortController = undefined;
        this._resolveIdleWaitIfIdle();
      }
    }
  }
  installCompactionSingleFlight(Session);
  const session = new Session();
  const run = session._runAutoCompaction();
  const idle = new Promise((resolve) => {
    session.resolveIdle = resolve;
  });
  const notifications = [];
  void idle.then(() => notifications.push("idle"));
  session.finish();
  await run;
  assert.deepEqual(notifications, ["idle"]);
});

test("a failed listener cannot suppress later listeners or reject asynchronously", async (t) => {
  class Session {
    _eventListeners = [];
    _emit(event) {
      this._eventListeners.forEach((listener) => listener(event));
    }
    async _emitSessionCompactFailed() {}
  }
  installCompactionEventReliability(Session);
  const log = t.mock.method(console, "error", () => {});
  const session = new Session();
  const delivered = [];
  session._eventListeners.push(
    () => {
      throw new Error("sync failure");
    },
    async () => {
      throw new Error("async failure");
    },
    (event) => delivered.push(event.type),
  );
  session._emit({ type: "compaction_end", reason: "manual", result: {} });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(delivered, ["compaction_end"]);
  assert.equal(log.mock.callCount(), 2);
});

test("a late abort cannot relabel an already committed result", () => {
  class Session {
    _autoCompactionAbortController = new AbortController();
    events = [];
    _emit(event) {
      this.events.push(event);
    }
    async _emitSessionCompactFailed() {}
  }
  installCompactionEventReliability(Session);
  const session = new Session();
  session._autoCompactionAbortController.abort();
  session._emit({ type: "compaction_end", reason: "threshold", result: {}, aborted: false });
  assert.equal(session.events[0].aborted, false);
});

test("post-save notification rejection is reported without undoing success", async (t) => {
  class Runner {
    sessionManager = { getEntries: () => [] };
    async emit() {
      throw new Error("notification failed");
    }
  }
  installLatestCompactionEventEntry(Runner);
  const log = t.mock.method(console, "error", () => {});
  const runner = new Runner();
  await assert.doesNotReject(runner.emit({ type: "session_compact" }));
  assert.equal(log.mock.callCount(), 1);
  await assert.rejects(runner.emit({ type: "session_before_compact" }), /notification failed/);
});

test("latest matching checkpoint is selected even after an extension appends state", async () => {
  class Runner {
    sessionManager = {
      getEntries: () => [
        { type: "compaction", summary: "same", id: "old" },
        { type: "compaction", summary: "same", id: "new" },
        { type: "custom", id: "extension-state" },
      ],
    };
    async emit(event) {
      return event.compactionEntry.id;
    }
  }
  installLatestCompactionEventEntry(Runner);
  assert.equal(
    await new Runner().emit({
      type: "session_compact",
      compactionEntry: { type: "compaction", summary: "same", id: "old" },
    }),
    "new",
  );
});
