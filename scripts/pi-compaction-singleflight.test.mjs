import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { installCompactionSingleFlight } from "./pi-compaction-singleflight.mjs";

const WRAPPER = join(dirname(fileURLToPath(import.meta.url)), "pi");

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

test("wrapper forwards an empty argument list under Bash nounset", () => {
  const home = mkdtempSync(join(tmpdir(), "pi-wrapper-empty-"));
  const realPi = join(home, ".bun/bin/pi");
  mkdirSync(dirname(realPi), { recursive: true });
  writeFileSync(realPi, "#!/bin/bash\nprintf '%s\\n' \"$#\"\n", { mode: 0o700 });

  try {
    const result = spawnSync("/bin/bash", [WRAPPER], {
      cwd: home,
      encoding: "utf8",
      env: { ...process.env, HOME: home, PI_PROVIDER: "noop" },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "0");
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("wrapper starts with a configured Node binary outside PATH", () => {
  const home = mkdtempSync(join(tmpdir(), "pi-wrapper-node-"));
  const realPi = join(home, ".bun/bin/pi");
  const node = join(home, "node");
  mkdirSync(dirname(realPi), { recursive: true });
  writeFileSync(realPi, "#!/bin/bash\nprintf 'started\\n'\n", { mode: 0o700 });
  writeFileSync(
    node,
    "#!/bin/bash\n/usr/bin/python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' \"$3\"\n",
    { mode: 0o700 },
  );

  try {
    const result = spawnSync("/bin/bash", [WRAPPER], {
      cwd: home,
      encoding: "utf8",
      env: {
        HOME: home,
        PATH: home,
        PI_NODE_BINARY: node,
        PI_PROVIDER: "noop",
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "started");
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

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
