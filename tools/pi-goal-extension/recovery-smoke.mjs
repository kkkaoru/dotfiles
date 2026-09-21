// Runs with Bun. Offline end-to-end recovery: real Pi SDK, no user history or real provider.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
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
import { latestLoopState } from "../pi-loop-extension/src/state.ts";
import { restoreGoal } from "./src/state.ts";

const root = path.dirname(fileURLToPath(import.meta.url));
const temporary = await mkdtemp(path.join(tmpdir(), "pi-recovery-smoke-"));
const originalFetch = globalThis.fetch;
const cost = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 };
const overflow =
  "Codex error: Your input exceeds the context window of this model. Please adjust your input and try again.";
const source = `ORIGINAL USER GOAL\n${"historical detail ".repeat(120_000)}\nLATEST TASK`;
const summaryFailureSource = `ORIGINAL USER GOAL\n${"historical detail ".repeat(800)}\nLATEST TASK`;

function historySource(mode) {
  return mode === "summary-failure" ? summaryFailureSource : source;
}

function assistant(block, model) {
  return {
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
}
function streamFor(scenario) {
  return (model, context) => {
    const summary = JSON.stringify(context.messages[0]).includes(
      "Summarize this transcript segment",
    );
    const events = createAssistantMessageEventStream();
    if (summary) scenario.summaryCalls += 1;
    else scenario.calls += 1;
    const fails = summary
      ? scenario.mode === "summary-failure"
      : scenario.mode !== "autonomous" &&
        (scenario.calls === 1 || scenario.mode === "retry-failure");
    const block = summary
      ? {
          type: "text",
          text: "Goal: finish the existing task. Next: complete loop and audit goal. Original history is available.",
        }
      : taskBlock(scenario.calls, scenario.mode);
    const message = assistant(block, model);
    events.push({ type: "start", partial: message });
    if (fails) {
      message.content = [];
      message.stopReason = "error";
      message.errorMessage = summary
        ? "Simulated summary provider failure"
        : overflow;
      events.push({ type: "error", reason: "error", error: message });
    } else {
      if (summary)
        assert.ok(Buffer.byteLength(JSON.stringify(context), "utf8") < 50_000);
      events.push({ type: "done", reason: message.stopReason, message });
    }
    events.end();
    return events;
  };
}
function taskBlock(call, mode) {
  if (mode === "autonomous") {
    if (call === 1)
      return {
        type: "toolCall",
        id: "agent-goal",
        name: "start_goal",
        arguments: {
          objective: "Implement and verify the user's requested work",
        },
      };
    if (call === 2)
      return {
        type: "toolCall",
        id: "agent-loop",
        name: "start_loop",
        arguments: {
          prompt: "Complete the user's authorized work and verification",
        },
      };
    return taskBlock(call - 1, "success");
  }
  if (call === 2)
    return {
      type: "toolCall",
      id: "loop-done",
      name: "loop_complete",
      arguments: { reason: "Offline recovery work verified." },
    };
  if (call === 3)
    return {
      type: "toolCall",
      id: "goal-done",
      name: "update_goal",
      arguments: {
        status: "complete",
        reason: "Automatic recovery and resumed work verified.",
      },
    };
  return { type: "text", text: "Recovery work complete." };
}
function goal(session) {
  return restoreGoal({
    entries: session.sessionManager.getBranch(),
    sessionId: session.sessionId,
    now: Date.now(),
  });
}
async function until(predicate, scenario) {
  const deadline = Date.now() + 15_000;
  while (!predicate()) {
    if (Date.now() > deadline)
      throw new Error(`Recovery timed out: ${JSON.stringify(scenario)}`);
    await delay(25);
  }
}
async function createSession(scenario) {
  const settings = SettingsManager.inMemory({
    packages: [],
    extensions: [],
    compaction: {
      enabled: true,
      reserveTokens: 16_384,
      keepRecentTokens: 1000,
    },
    retry: { enabled: false },
  });
  const loader = new DefaultResourceLoader({
    cwd: temporary,
    agentDir: temporary,
    settingsManager: settings,
    noExtensions: true,
    noSkills: true,
    noThemes: true,
    noPromptTemplates: true,
    noContextFiles: true,
    additionalExtensionPaths: [
      path.join(root, "index.ts"),
      path.resolve(root, "../pi-loop-extension/index.ts"),
      path.resolve(root, "../pi-effort-manager/src/context-guard.ts"),
    ],
  });
  await loader.reload();
  assert.deepEqual(loader.getExtensions().errors, []);
  assert.equal(loader.getExtensions().extensions.length, 3);
  const models = await ModelRuntime.create({
    credentials: new InMemoryCredentialStore(),
    modelsStore: new InMemoryModelsStore(),
    modelsPath: null,
    allowModelNetwork: false,
    refreshOnCreate: false,
  });
  models.registerProvider("recovery-smoke", {
    baseUrl: "https://invalid.local",
    apiKey: "offline-placeholder",
    api: "recovery-smoke-api",
    streamSimple: streamFor(scenario),
    models: [
      {
        id: "fake",
        name: "Offline recovery fake",
        reasoning: false,
        input: ["text"],
        cost,
        contextWindow: 100_000,
        maxTokens: 4096,
      },
    ],
  });
  const model = models.getModel("recovery-smoke", "fake");
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
      "get_goal",
      "start_goal",
      "start_loop",
      "update_goal",
      "goal_wait",
      "loop_complete",
      "loop_wakeup",
    ],
  });
  session.subscribe((event) => {
    if (event.type === "compaction_start" || event.type === "compaction_end")
      scenario.events.push(event);
    if (event.type === "agent_settled")
      scenario.events.push({
        type: event.type,
        goal: goal(session)?.status,
        loop: latestLoopState(session.sessionManager.getBranch())?.paused,
        settings: session.settingsManager.getCompactionSettings(),
        model: session.model?.id,
        provider: session.model?.provider,
      });
    if (event.type === "agent_end")
      scenario.events.push({
        type: event.type,
        messages: event.messages
          .filter((message) => message.role === "assistant")
          .map((message) => ({
            stopReason: message.stopReason,
            errorMessage: message.errorMessage,
            provider: message.provider,
            model: message.model,
          })),
      });
  });
  await session.bindExtensions({
    onError: (error) => {
      scenario.errors.push(error.message);
    },
  });
  if (scenario.mode === "autonomous") return session;
  await session.prompt("/goal Automatically recover existing work");
  // Seed a historical oversized turn without contacting any model. Keep usage deliberately low
  // so the first request exercises the native overflow path, not proactive threshold compaction.
  session.sessionManager.appendMessage({
    role: "user",
    content: historySource(scenario.mode),
    timestamp: Date.now(),
  });
  session.sessionManager.appendMessage(
    assistant({ type: "text", text: "Historical checkpoint." }, model),
  );
  session.sessionManager.appendMessage({
    role: "user",
    content: "Finish the remaining verified work. ".repeat(200),
    timestamp: Date.now(),
  });
  session.agent.state.messages =
    session.sessionManager.buildSessionContext().messages;
  return session;
}
async function verifyAutonomy() {
  const scenario = {
    mode: "autonomous",
    calls: 0,
    summaryCalls: 0,
    errors: [],
    events: [],
  };
  const session = await createSession(scenario);
  try {
    await session.prompt(
      "Implement the requested work and verify it. Choose useful goal and loop tracking yourself.",
    );
    await until(
      () => goal(session)?.status === "complete" && !session.isStreaming,
      scenario,
    );
    assert.equal(scenario.calls, 5);
    assert.equal(
      latestLoopState(session.sessionManager.getBranch())?.paused,
      false,
    );
    await delay(5500);
    assert.equal(
      scenario.calls,
      5,
      "Agent-defined tracking must not enqueue a duplicate turn",
    );
    assert.deepEqual(scenario.errors, []);
    console.log(
      "PASS: agent-defined goal and loop tools, same-turn work, audited completion, no duplicate continuation",
    );
  } finally {
    await session.extensionRunner.emit({
      type: "session_shutdown",
      reason: "quit",
    });
    session.dispose();
  }
}

async function verify(mode) {
  const scenario = { mode, calls: 0, summaryCalls: 0, errors: [], events: [] };
  const session = await createSession(scenario);
  try {
    // No /compact, continue or resume command is submitted by this test.
    await session.prompt("/loop Finish the existing task");
    const expected = mode === "success" ? "complete" : "paused";
    await until(
      () => goal(session)?.status === expected && !session.isStreaming,
      scenario,
    );
    const entries = session.sessionManager.getBranch();
    const compactions = entries.filter((entry) => entry.type === "compaction");
    assert.equal(latestLoopState(entries)?.paused, mode !== "success");
    if (mode === "summary-failure") {
      assert.equal(scenario.summaryCalls, 1);
      assert.equal(scenario.calls, 1);
      assert.equal(compactions.length, 0);
      assert.match(goal(session)?.reason, /safe mode/u);
    } else {
      assert.equal(scenario.summaryCalls, 0);
      assert.equal(compactions.length, 1);
      assert.match(compactions[0].summary, /^Emergency recovery compaction:/u);
      assert.ok(JSON.stringify(session.messages).length < 100_000);
      assert.equal(scenario.calls, mode === "success" ? 4 : 2);
    }
    assert.ok(
      entries.some(
        (entry) =>
          entry.type === "message" &&
          entry.message.role === "user" &&
          entry.message.content === historySource(mode),
      ),
      "Original history must remain intact",
    );
    const calls = scenario.calls;
    await delay(5500);
    assert.equal(
      scenario.calls,
      calls,
      "No duplicate continuation or endless recovery loop",
    );
    assert.deepEqual(scenario.errors, []);
    console.log(
      `PASS: automatic recovery ${mode}; task requests=${scenario.calls}, summary requests=${scenario.summaryCalls}`,
    );
  } finally {
    await session.extensionRunner.emit({
      type: "session_shutdown",
      reason: "quit",
    });
    session.dispose();
  }
}
try {
  globalThis.fetch = () => {
    throw new Error("Network is disabled in recovery smoke test.");
  };
  await verifyAutonomy();
  await verify("success");
  await verify("summary-failure");
  await verify("retry-failure");
} finally {
  globalThis.fetch = originalFetch;
  await rm(temporary, { recursive: true, force: true });
}
