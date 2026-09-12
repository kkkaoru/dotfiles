// Runs with Bun. Offline integration smoke: real Pi SDK and one disposable tmux job.
// Only a newly-created temporary directory is written; no real model or credentials are used.
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import {
  createAssistantMessageEventStream,
  InMemoryCredentialStore,
  InMemoryModelsStore,
} from "@earendil-works/pi-ai";
import {
  createAgentSession,
  DefaultResourceLoader,
  ModelRuntime,
  SessionManager,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";
import { restoreGoal } from "./src/state.ts";

const root = path.dirname(fileURLToPath(import.meta.url));
const temporary = await mkdtemp(path.join(tmpdir(), "pi-goal-smoke-"));
const original = {
  fetch: globalThis.fetch,
  tmpdir: process.env.TMPDIR,
  offline: process.env.PI_OFFLINE,
};
const scenario = {
  mode: "complete",
  calls: 0,
  artifact: null,
  session: null,
  errors: [],
};
const cost = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 };

function state() {
  assert.ok(scenario.session);
  return restoreGoal({
    entries: scenario.session.sessionManager.getBranch(),
    sessionId: scenario.session.sessionId,
    now: Date.now(),
  });
}
async function until(predicate, description) {
  const deadline = Date.now() + 15000;
  while (!predicate()) {
    if (Date.now() > deadline)
      throw new Error(
        `Timed out: ${description}; errors=${JSON.stringify(scenario.errors)}`,
      );
    await delay(25);
  }
}
function tool(name, args) {
  return {
    type: "toolCall",
    id: `smoke-${scenario.mode}-${scenario.calls}`,
    name,
    arguments: args,
  };
}
function response(context) {
  assert.match(context.systemPrompt, /objective is user data/);
  scenario.calls += 1;
  if (scenario.mode === "retry" && scenario.calls === 1)
    return { type: "error", text: "503 Service Unavailable" };
  if (scenario.mode === "loop")
    return {
      type: "text",
      text: "Waiting for the next authorized loop check.",
    };
  if (scenario.mode === "tmux") {
    if (scenario.calls === 1)
      return tool("tmux_exec", {
        command: "sleep 1; printf goal-smoke-ok",
        estimatedDurationSeconds: 3,
        timeoutSeconds: 5,
      });
    if (scenario.calls === 2)
      return {
        type: "text",
        text: "Waiting for the existing task notification.",
      };
    if (scenario.calls === 3)
      return tool("read", { path: scenario.artifact.statusPath });
    if (scenario.calls === 4)
      return tool("read", { path: scenario.artifact.logPath });
    if (scenario.calls === 5) {
      assert.match(JSON.stringify(context.messages), /goal-smoke-ok/);
      return tool("update_goal", {
        status: "complete",
        reason: "Inspected the existing task exit status and output.",
      });
    }
    return { type: "text", text: "Verified complete." };
  }
  const completionCall = scenario.mode === "retry" ? 2 : 1;
  return scenario.calls === completionCall
    ? tool("update_goal", {
        status: "complete",
        reason: "Offline SDK goal audit verified.",
      })
    : { type: "text", text: "Verified complete." };
}
function stream(model, context) {
  const events = createAssistantMessageEventStream();
  const block = response(context);
  const message = {
    role: "assistant",
    content: [block],
    api: model.api,
    provider: model.provider,
    model: model.id,
    usage: {
      input: 1,
      output: 1,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 2,
      cost,
    },
    stopReason: block.type === "toolCall" ? "toolUse" : "stop",
    timestamp: Date.now(),
  };
  events.push({ type: "start", partial: message });
  if (block.type === "error") {
    message.content = [];
    message.stopReason = "error";
    message.errorMessage = block.text;
    events.push({ type: "error", reason: "error", error: message });
    events.end();
    return events;
  }
  events.push({ type: "done", reason: message.stopReason, message });
  events.end();
  return events;
}
async function prepare() {
  process.env.TMPDIR = temporary;
  process.env.PI_OFFLINE = "1";
  globalThis.fetch = () => {
    throw new Error("Network is disabled in goal smoke test.");
  };
  await mkdir(path.join(temporary, "extensions"));
  await Promise.all([
    symlink(root, path.join(temporary, "extensions/goal"), "dir"),
    symlink(
      path.resolve(root, "../pi-loop-extension"),
      path.join(temporary, "extensions/loop"),
      "dir",
    ),
    symlink(
      path.resolve(root, "../pi-tmux-timeout-extension"),
      path.join(temporary, "extensions/tmux-timeout"),
      "dir",
    ),
  ]);
  const settings = SettingsManager.inMemory({
    packages: [],
    extensions: [],
    compaction: { enabled: false },
    retry: { enabled: true, maxRetries: 1, baseDelayMs: 10 },
  });
  const loader = new DefaultResourceLoader({
    cwd: temporary,
    agentDir: temporary,
    settingsManager: settings,
    noExtensions: false,
    noSkills: true,
    noThemes: true,
    noPromptTemplates: true,
    noContextFiles: true,
  });
  await loader.reload();
  assert.deepEqual(loader.getExtensions().errors, []);
  assert.deepEqual(
    loader
      .getExtensions()
      .extensions.map((extension) => extension.path)
      .sort(),
    ["goal", "loop", "tmux-timeout"]
      .map((name) => path.join(temporary, "extensions", name, "index.ts"))
      .sort(),
  );
  const models = await ModelRuntime.create({
    credentials: new InMemoryCredentialStore(),
    modelsStore: new InMemoryModelsStore(),
    modelsPath: null,
    allowModelNetwork: false,
    refreshOnCreate: false,
  });
  models.registerProvider("goal-smoke", {
    baseUrl: "https://invalid.local",
    apiKey: "offline-placeholder",
    api: "goal-smoke-api",
    streamSimple: stream,
    models: [
      {
        id: "fake",
        name: "Offline fake model",
        reasoning: false,
        input: ["text"],
        cost,
        contextWindow: 100000,
        maxTokens: 1000,
      },
    ],
  });
  const model = models.getModel("goal-smoke", "fake");
  assert.ok(model);
  const { session } = await createAgentSession({
    cwd: temporary,
    agentDir: temporary,
    model,
    modelRuntime: models,
    settingsManager: settings,
    resourceLoader: loader,
    sessionManager: SessionManager.inMemory(temporary),
    tools: [
      "read",
      "get_goal",
      "update_goal",
      "goal_wait",
      "loop_wakeup",
      "loop_complete",
      "tmux_exec",
    ],
  });
  scenario.session = session;
  session.subscribe((event) => {
    if (event.type === "tool_execution_end" && event.toolName === "tmux_exec")
      scenario.artifact = event.result.details;
  });
  await session.bindExtensions({
    onError: (error) => {
      scenario.errors.push(error.message);
    },
  });
}
async function verify() {
  const session = scenario.session;
  await session.prompt("/goal Offline SDK completion");
  assert.equal(state()?.status, "active");
  await session.prompt("/goal pause");
  await delay(5200);
  assert.equal(scenario.calls, 0);
  await session.prompt("/goal resume");
  await until(
    () => state()?.status === "complete" && !session.isStreaming,
    "goal completion",
  );
  assert.equal(scenario.calls, 2);
  assert.equal(state()?.tokensUsed, 4);

  scenario.mode = "retry";
  scenario.calls = 0;
  await session.prompt("/goal Recover an SDK retry");
  await until(
    () => state()?.status === "complete" && !session.isStreaming,
    "native retry recovery",
  );
  assert.equal(scenario.calls, 3);
  assert.equal(state()?.tokensUsed, 6);
  assert.equal(state()?.turn, 2);

  scenario.mode = "loop";
  scenario.calls = 0;
  await session.prompt("/goal Shared loop pacing");
  await session.prompt("/loop 1h Observe authorized work");
  await until(
    () => scenario.calls === 1 && !session.isStreaming,
    "initial loop turn",
  );
  await delay(5200);
  assert.equal(
    scenario.calls,
    1,
    "goal must not duplicate loop-owned continuation",
  );
  await session.prompt("/loop clear");
  scenario.mode = "complete";
  scenario.calls = 0;
  await until(
    () => state()?.status === "complete" && !session.isStreaming,
    "goal resumes after loop clear",
  );
  assert.equal(scenario.calls, 2);

  scenario.mode = "tmux";
  scenario.calls = 0;
  await session.prompt("/goal Verify one disposable tmux job");
  await until(
    () => state()?.status === "complete" && !session.isStreaming,
    "tmux notification and evidence audit",
  );
  assert.equal(
    scenario.calls,
    6,
    "one launch run plus one completion run; no duplicate goal prompt",
  );
  assert.equal(
    (await readFile(scenario.artifact.statusPath, "utf8")).trim(),
    "0",
  );
  assert.equal(
    await readFile(scenario.artifact.logPath, "utf8"),
    "goal-smoke-ok",
  );
  assert.deepEqual(scenario.errors, []);
  console.log(
    "PASS: native SDK symlink loading, pause/resume, goal completion/accounting, native retry recovery, loop precedence, real tmux notification/evidence audit.",
  );
}
try {
  await prepare();
  await verify();
} finally {
  if (scenario.session) {
    await scenario.session.extensionRunner.emit({
      type: "session_shutdown",
      reason: "quit",
    });
    scenario.session.dispose();
  }
  globalThis.fetch = original.fetch;
  if (original.tmpdir === undefined) delete process.env.TMPDIR;
  else process.env.TMPDIR = original.tmpdir;
  if (original.offline === undefined) delete process.env.PI_OFFLINE;
  else process.env.PI_OFFLINE = original.offline;
  await rm(temporary, { recursive: true, force: true });
}
