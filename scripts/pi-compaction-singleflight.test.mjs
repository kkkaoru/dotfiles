import assert from "node:assert/strict";
import test from "node:test";
import { installCompactionSingleFlight } from "./pi-compaction-singleflight.mjs";

function deferred() {
  const state = {};
  state.promise = new Promise((resolve) => {
    state.resolve = resolve;
  });
  return state;
}

function vulnerableSessionClass() {
  return class VulnerableSession {
    _autoCompactionAbortController;
    _compactionAbortController;
    autoCalls = 0;
    manualCalls = 0;
    preflight = deferred();
    work = deferred();

    get isCompacting() {
      return (
        this._autoCompactionAbortController !== undefined ||
        this._compactionAbortController !== undefined
      );
    }

    async _runAutoCompaction() {
      this.autoCalls += 1;
      await this.preflight.promise;
      this._autoCompactionAbortController = new AbortController();
      try {
        await this.work.promise;
        return this._autoCompactionAbortController.signal.aborted;
      } finally {
        this._autoCompactionAbortController = undefined;
      }
    }

    async compact() {
      this.manualCalls += 1;
      this._compactionAbortController = new AbortController();
      try {
        await this.work.promise;
        return this._compactionAbortController.signal.aborted;
      } finally {
        this._compactionAbortController = undefined;
      }
    }
  };
}

test("installs once on the vulnerable AgentSession shape", () => {
  const AgentSession = vulnerableSessionClass();

  assert.equal(installCompactionSingleFlight(AgentSession), "installed");
  assert.equal(installCompactionSingleFlight(AgentSession), "already-installed");
});

test("skips a second auto-compaction before the core controller exists", async () => {
  const AgentSession = vulnerableSessionClass();
  installCompactionSingleFlight(AgentSession);
  const session = new AgentSession();

  const first = session._runAutoCompaction("threshold", false);
  assert.equal(session.isCompacting, true);
  assert.equal(await session._runAutoCompaction("threshold", false), false);
  assert.equal(session.autoCalls, 1);

  session.preflight.resolve();
  session.work.resolve();
  assert.equal(await first, false);
  assert.equal(session.isCompacting, false);
});

test("rejects manual compaction while auto-compaction is active", async () => {
  const AgentSession = vulnerableSessionClass();
  installCompactionSingleFlight(AgentSession);
  const session = new AgentSession();

  const first = session._runAutoCompaction("threshold", false);
  await assert.rejects(
    session.compact(),
    new Error("Compaction is already in progress. Wait for it to finish and retry."),
  );
  assert.equal(session.manualCalls, 0);

  session.preflight.resolve();
  session.work.resolve();
  await first;
});

test("rejects overlapping manual compactions and releases the lock", async () => {
  const AgentSession = vulnerableSessionClass();
  installCompactionSingleFlight(AgentSession);
  const session = new AgentSession();

  const first = session.compact();
  await assert.rejects(
    session.compact(),
    new Error("Compaction is already in progress. Wait for it to finish and retry."),
  );
  assert.equal(session.manualCalls, 1);
  assert.equal(session.isCompacting, true);

  session.work.resolve();
  assert.equal(await first, false);
  assert.equal(session.isCompacting, false);
});

test("does not patch a core that no longer has mutable controller reads", () => {
  class FixedSession {
    get isCompacting() {
      return false;
    }

    async _runAutoCompaction() {
      return false;
    }

    async compact() {
      return undefined;
    }
  }

  assert.equal(installCompactionSingleFlight(FixedSession), "not-needed");
});
