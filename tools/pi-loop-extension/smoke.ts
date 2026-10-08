// Runs with Bun. Offline native Pi loading and real JSONL/lock recovery in a disposable directory.
import assert from "node:assert/strict";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { lock } from "proper-lockfile";
import {
  type AgentSession,
  createAgentSession,
  DefaultResourceLoader,
  ModelRuntime,
  SessionManager,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";
import { createLoopState, latestLoopState, LOOP_STATE_ENTRY_TYPE } from "./src/state.ts";

const directory = await mkdtemp(path.join(tmpdir(), "pi-durable-loop-smoke-"));
const sessions: AgentSession[] = [];
const errors: string[] = [];
const originalFetch = globalThis.fetch;
globalThis.fetch = (): never => {
  throw new Error("Network is disabled in loop smoke.");
};

async function open(manager: SessionManager): Promise<AgentSession> {
  const settingsManager = SettingsManager.inMemory({ packages: [], extensions: [] });
  const resourceLoader = new DefaultResourceLoader({
    cwd: directory,
    agentDir: directory,
    settingsManager,
    additionalExtensionPaths: [fileURLToPath(new globalThis.URL("index.ts", import.meta.url))],
    noSkills: true,
    noThemes: true,
    noPromptTemplates: true,
    noContextFiles: true,
  });
  await resourceLoader.reload();
  assert.deepEqual(resourceLoader.getExtensions().errors, []);
  const modelRuntime = await ModelRuntime.create({
    authPath: path.join(directory, "auth.json"),
    modelsPath: path.join(directory, "models.json"),
    allowModelNetwork: false,
    refreshOnCreate: false,
  });
  const { session } = await createAgentSession({
    cwd: directory,
    agentDir: directory,
    settingsManager,
    resourceLoader,
    modelRuntime,
    sessionManager: manager,
    tools: [],
  });
  sessions.push(session);
  await session.bindExtensions({
    onError: (error) => {
      errors.push(error.error);
    },
  });
  return session;
}

async function close(session: AgentSession): Promise<void> {
  await session.extensionRunner.emit({ type: "session_shutdown", reason: "quit" });
  session.dispose();
  sessions.splice(sessions.indexOf(session), 1);
}

try {
  const manager = SessionManager.create(directory, directory);
  manager.appendMessage({
    role: "user",
    content: "Offline migration fixture; do not call a model.",
    timestamp: Date.now(),
  });
  manager.appendCustomEntry(
    LOOP_STATE_ENTRY_TYPE,
    createLoopState({
      jobs: [
        {
          id: 1,
          nextRunAt: Date.now() + 3_600_000,
          remainingMs: 3_600_000,
          prompt: "offline",
          reason: "fixture",
          submittedAt: Date.now(),
        },
      ],
      nextId: 2,
      paused: true,
      pendingContinuations: [],
      runningContinuation: undefined,
    }),
  );
  const file = manager.getSessionFile();
  assert.ok(file !== undefined);
  const first = await open(manager);
  assert.deepEqual(errors, []);
  const stored = await stat(`${file}.loop-durable`);
  assert.ok(stored.isDirectory());
  await assert.rejects(lock(`${file}.loop-durable`), /already being held/u);
  await first.prompt("/loop resume");
  await first.prompt("/loop pause");
  assert.equal(latestLoopState(manager.getEntries())?.paused, true);
  await close(first);

  const reopened = SessionManager.open(file);
  const second = await open(reopened);
  assert.equal(latestLoopState(reopened.getEntries())?.paused, true);
  await second.prompt("/loop resume");
  assert.equal(latestLoopState(reopened.getEntries())?.jobs.length, 1);
  await second.prompt("/loop clear");
  assert.deepEqual(latestLoopState(reopened.getEntries())?.jobs, []);
  await close(second);
  const release = await lock(`${file}.loop-durable`);
  await release();
  assert.deepEqual(errors, []);
  process.stdout.write(
    "PASS: native Pi extension loading, legacy migration, durable JSONL reopen, pause/resume/clear, exclusive lock and cleanup; no model or network calls.\n",
  );
} finally {
  await Promise.all([...sessions].map(async (session) => close(session)));
  globalThis.fetch = originalFetch;
  await rm(directory, { recursive: true, force: true });
}
