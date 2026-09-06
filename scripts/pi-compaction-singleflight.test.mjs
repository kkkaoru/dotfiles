import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  installCompactionEventReliability,
  installCompactionPreparationReliability,
  installCompactionSingleFlight,
  installCompactCommandVisibility,
  installExtensionUiRenderDedupe,
  installFooterRenderCache,
  installLargeSessionStatusRenderThrottle,
  installLatestCompactionEventEntry,
} from "./pi-compaction-singleflight.mjs";

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

    get isIdle() {
      return !this.isCompacting;
    }

    async waitForIdle() {
      while (!this.isIdle) {
        await new Promise((resolve) => setTimeout(resolve, 1));
      }
    }

    async abort() {
      await this.waitForIdle();
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
      await this.abort();
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

function interactiveModeClass(entryCount) {
  return class InteractiveMode {
    sessionManager = {
      getEntries: () => Array.from({ length: entryCount }),
    };
    shownIndicators = [];

    showStatusIndicator(indicator) {
      this.shownIndicators.push(indicator);
      return "shown";
    }
  };
}

function statusIndicator(kind, intervalMs = 80) {
  return {
    intervalMs,
    kind,
    restartCalls: 0,
    restartAnimation() {
      this.restartCalls += 1;
    },
  };
}

function footerComponentClass() {
  return class FooterComponent {
    autoCompactEnabled = true;
    footerData = {
      getAvailableProviderCount: () => 2,
      getExtensionStatuses: () => this.statuses,
      getGitBranch: () => "main",
    };
    renderCalls = 0;
    session = {
      sessionManager: {
        getCwd: () => "/project",
        getLeafId: () => this.leafId,
        getSessionId: () => "session-1",
        getSessionName: () => undefined,
      },
      state: {
        model: { id: "model", provider: "provider", reasoning: true },
        thinkingLevel: "medium",
      },
    };
    leafId = "leaf-1";
    statuses = new Map();

    render(width) {
      this.renderCalls += 1;
      return [`render:${width}:${this.renderCalls}`];
    }
  };
}

function extensionUiClass() {
  return class InteractiveMode {
    extensionWidgetsAbove = new Map();
    extensionWidgetsBelow = new Map();
    footerDataProvider = {
      getExtensionStatuses: () => this.statuses,
    };
    statusCalls = 0;
    statuses = new Map();
    widgetCalls = 0;

    setExtensionStatus(key, value) {
      this.statusCalls += 1;
      if (value === undefined) this.statuses.delete(key);
      else this.statuses.set(key, value);
    }

    setExtensionWidget(key, content) {
      this.widgetCalls += 1;
      if (content === undefined) this.extensionWidgetsAbove.delete(key);
      else this.extensionWidgetsAbove.set(key, content);
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
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(session.manualCalls, 1);
  assert.equal(session.isCompacting, true);

  session.work.resolve();
  assert.equal(await first, false);
  assert.equal(session.isCompacting, false);
});

test("manual compaction does not wait on its own single-flight reservation", async () => {
  const AgentSession = vulnerableSessionClass();
  installCompactionSingleFlight(AgentSession);
  const session = new AgentSession();

  const compaction = session.compact();
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(session.manualCalls, 1);
  assert.equal(session.isCompacting, true);

  session.work.resolve();
  assert.equal(await compaction, false);
  assert.equal(session.isIdle, true);
});

test("manual compaction announces start before abort returns", async () => {
  class AgentSession {
    events = [];
    _autoCompactionAbortController;
    _compactionAbortController;
    work = deferred();

    get isCompacting() {
      return this._compactionAbortController !== undefined;
    }

    _emit(event) {
      this.events.push(event.type);
    }

    async abort() {
      await this.work.promise;
    }

    async compact() {
      await this.abort();
      this._compactionAbortController = new AbortController();
      this._emit({ type: "compaction_start", reason: "manual" });
      return this._compactionAbortController.signal.aborted;
    }

    async _runAutoCompaction() {
      return this._autoCompactionAbortController.signal.aborted;
    }
  }

  installCompactionSingleFlight(AgentSession);
  const session = new AgentSession();
  const compaction = session.compact();
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(session.events, ["compaction_start"]);
  assert.equal(session._compactionAbortController, undefined);
  session.work.resolve();
  assert.equal(await compaction, false);
});

test("slash compact shows status without clearing first", async () => {
  class InteractiveMode {
    events = [];
    renders = 0;
    session = {
      compactCalls: 0,
      _emit: (event) => this.events.push(event.type),
      async compact() {
        this.compactCalls += 1;
      },
    };
    ui = {
      requestRender: () => {
        this.renders += 1;
      },
    };

    async handleCompactCommand() {
      this.events.push("cleared");
      await this.session.compact();
    }
  }

  assert.equal(installCompactCommandVisibility(InteractiveMode), "installed");
  assert.equal(installCompactCommandVisibility(InteractiveMode), "already-installed");
  const mode = new InteractiveMode();
  await mode.handleCompactCommand();
  assert.deepEqual(mode.events, ["compaction_start"]);
  assert.equal(mode.renders, 1);
  assert.equal(mode.session.compactCalls, 1);
});

test("skips compact-command patching when Pi no longer exposes the method", () => {
  class InteractiveMode {}
  assert.equal(installCompactCommandVisibility(InteractiveMode), "not-needed");
});

test("merges a split turn without reducing the reasoning and output budget", async () => {
  class AgentSession {
    calls = [];

    _runDefaultCompaction(...args) {
      this.calls.push(args);
      return { summary: "compacted" };
    }
  }
  installCompactionPreparationReliability(AgentSession);
  const session = new AgentSession();
  const preparation = {
    firstKeptEntryId: "kept",
    messagesToSummarize: ["history"],
    turnPrefixMessages: ["turn-prefix"],
    isSplitTurn: true,
    tokensBefore: 100,
    settings: { enabled: true, keepRecentTokens: 20_000, reserveTokens: 16_384 },
  };

  assert.deepEqual(
    await session._runDefaultCompaction(
      preparation,
      "model",
      "key",
      {},
      "Preserve horse-racing decisions.",
      new AbortController().signal,
      {},
      "manual",
    ),
    { summary: "compacted" },
  );
  assert.deepEqual(session.calls[0][0], {
    firstKeptEntryId: "kept",
    messagesToSummarize: ["history", "turn-prefix"],
    turnPrefixMessages: [],
    isSplitTurn: false,
    tokensBefore: 100,
    settings: { enabled: true, keepRecentTokens: 20_000, reserveTokens: 16_384 },
  });
  assert.match(session.calls[0][4], /^Preserve horse-racing decisions\./);
  assert.match(session.calls[0][4], /prefix of the currently retained turn/);
  assert.match(session.calls[0][4], /under 4,000 words/);
});

test("reclassifies an aborted automatic compaction as cancellation", async () => {
  class AgentSession {
    emitted = [];
    failed = [];
    _autoCompactionAbortController = new AbortController();

    _emit(event) {
      this.emitted.push(event);
    }

    async _emitSessionCompactFailed(event) {
      this.failed.push(event);
    }
  }
  installCompactionEventReliability(AgentSession);
  const session = new AgentSession();
  session._autoCompactionAbortController.abort();

  session._emit({
    type: "compaction_end",
    reason: "threshold",
    aborted: false,
    errorMessage: "Auto-compaction failed: This operation was aborted",
    willRetry: false,
  });
  await session._emitSessionCompactFailed({
    reason: "threshold",
    aborted: false,
    errorMessage: "Auto-compaction failed: This operation was aborted",
    willRetry: false,
  });

  assert.deepEqual(session.emitted, [
    {
      type: "compaction_end",
      reason: "threshold",
      aborted: true,
      errorMessage: undefined,
      willRetry: false,
    },
  ]);
  assert.deepEqual(session.failed, [
    {
      reason: "threshold",
      aborted: true,
      errorMessage: undefined,
      willRetry: false,
    },
  ]);
});

test("isolates compaction lifecycle listener errors after persistence", () => {
  class AgentSession {
    _emit(event) {
      if (event.type === "compaction_end") {
        throw new Error("renderer failed");
      }
    }

    async _emitSessionCompactFailed() {}
  }
  installCompactionEventReliability(AgentSession);
  const originalError = console.error;
  const errors = [];
  console.error = (message) => errors.push(message);
  try {
    assert.doesNotThrow(() =>
      new AgentSession()._emit({ type: "compaction_end", reason: "manual", aborted: false }),
    );
  } finally {
    console.error = originalError;
  }
  assert.equal(errors.length, 1);
  assert.match(errors[0], /renderer failed/);
});

test("uses the latest persisted compaction entry for extension events", async () => {
  class ExtensionRunner {
    sessionManager = {
      getEntries: () => [
        { id: "old", type: "compaction", summary: "same summary" },
        { id: "latest", type: "compaction", summary: "same summary" },
      ],
    };
    events = [];

    async emit(event) {
      this.events.push(event);
    }
  }
  installLatestCompactionEventEntry(ExtensionRunner);
  const runner = new ExtensionRunner();

  await runner.emit({
    type: "session_compact",
    compactionEntry: { id: "old", type: "compaction", summary: "same summary" },
  });

  assert.equal(runner.events[0].compactionEntry.id, "latest");
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

test("slows compaction status rendering for a large session", () => {
  const InteractiveMode = interactiveModeClass(10_000);
  assert.equal(installLargeSessionStatusRenderThrottle(InteractiveMode), "installed");
  assert.equal(installLargeSessionStatusRenderThrottle(InteractiveMode), "already-installed");
  const mode = new InteractiveMode();
  const indicator = statusIndicator("compaction");

  assert.equal(mode.showStatusIndicator(indicator), "shown");
  assert.deepEqual(mode.shownIndicators, [indicator]);
  assert.equal(indicator.intervalMs, 750);
  assert.equal(indicator.restartCalls, 1);
});

test("also slows working and branch-summary rendering for a large session", () => {
  for (const kind of ["working", "branchSummary"]) {
    const InteractiveMode = interactiveModeClass(20_000);
    installLargeSessionStatusRenderThrottle(InteractiveMode);
    const indicator = statusIndicator(kind);

    new InteractiveMode().showStatusIndicator(indicator);

    assert.equal(indicator.intervalMs, 750);
    assert.equal(indicator.restartCalls, 1);
  }
});

test("keeps the normal render cadence for small sessions and unrelated statuses", () => {
  const SmallInteractiveMode = interactiveModeClass(9_999);
  installLargeSessionStatusRenderThrottle(SmallInteractiveMode);
  const smallCompaction = statusIndicator("compaction");
  new SmallInteractiveMode().showStatusIndicator(smallCompaction);

  const LargeInteractiveMode = interactiveModeClass(20_000);
  installLargeSessionStatusRenderThrottle(LargeInteractiveMode);
  const retry = statusIndicator("retry");
  new LargeInteractiveMode().showStatusIndicator(retry);

  assert.equal(smallCompaction.intervalMs, 80);
  assert.equal(smallCompaction.restartCalls, 0);
  assert.equal(retry.intervalMs, 80);
  assert.equal(retry.restartCalls, 0);
});

test("does not speed up an intentionally slower custom indicator", () => {
  const InteractiveMode = interactiveModeClass(20_000);
  installLargeSessionStatusRenderThrottle(InteractiveMode);
  const indicator = statusIndicator("working", 1_000);

  new InteractiveMode().showStatusIndicator(indicator);

  assert.equal(indicator.intervalMs, 1_000);
  assert.equal(indicator.restartCalls, 0);
});

test("skips status render patching when Pi no longer exposes the method", () => {
  class InteractiveMode {}

  assert.equal(installLargeSessionStatusRenderThrottle(InteractiveMode), "not-needed");
});

test("caches footer rendering until visible session state changes", () => {
  const FooterComponent = footerComponentClass();
  assert.equal(installFooterRenderCache(FooterComponent), "installed");
  assert.equal(installFooterRenderCache(FooterComponent), "already-installed");
  const footer = new FooterComponent();

  assert.deepEqual(footer.render(120), ["render:120:1"]);
  assert.deepEqual(footer.render(120), ["render:120:1"]);
  assert.equal(footer.renderCalls, 1);

  footer.statuses.set("agmsg", "agmsg: alice");
  assert.deepEqual(footer.render(120), ["render:120:2"]);
  footer.leafId = "leaf-2";
  assert.deepEqual(footer.render(120), ["render:120:3"]);
  assert.deepEqual(footer.render(80), ["render:80:4"]);
});

test("skips footer caching when Pi no longer exposes the required shape", () => {
  class FooterComponent {}

  assert.equal(installFooterRenderCache(FooterComponent), "not-needed");
});

test("skips unchanged extension status and absent widget removals", () => {
  const InteractiveMode = extensionUiClass();
  assert.equal(installExtensionUiRenderDedupe(InteractiveMode), "installed");
  assert.equal(installExtensionUiRenderDedupe(InteractiveMode), "already-installed");
  const mode = new InteractiveMode();

  mode.setExtensionStatus("agmsg", undefined);
  mode.setExtensionStatus("agmsg", "agmsg: alice");
  mode.setExtensionStatus("agmsg", "agmsg: alice");
  assert.equal(mode.statusCalls, 1);

  mode.setExtensionWidget("loop", undefined);
  mode.setExtensionWidget("loop", ["scheduled"]);
  mode.setExtensionWidget("loop", undefined);
  mode.setExtensionWidget("loop", undefined);
  assert.equal(mode.widgetCalls, 2);
});

test("skips extension UI dedupe when either Pi method is unavailable", () => {
  class InteractiveMode {
    setExtensionStatus() {}
  }

  assert.equal(installExtensionUiRenderDedupe(InteractiveMode), "not-needed");
});
