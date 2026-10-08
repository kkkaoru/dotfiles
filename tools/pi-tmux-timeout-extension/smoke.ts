// Runs with Bun. Real tmux and Durable JSONL; disposable fixtures, no model or network calls.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { DefaultResourceLoader, SettingsManager } from "@earendil-works/pi-coding-agent";
import { lock } from "proper-lockfile";
import { registerTmux, type TmuxExtensionHost, type TmuxToolDefinition } from "./index.ts";
import { durableTmuxHost } from "./src/durable-host.ts";
import type { CompletionDeliveryContext } from "./src/delivery.ts";
import type { TmuxLaunch } from "./src/tmux.ts";

interface Fixture {
  readonly host: TmuxExtensionHost;
  readonly handlers: Map<string, (event: unknown, context?: CompletionDeliveryContext) => unknown>;
  readonly tools: Map<string, TmuxToolDefinition>;
}
const directory = await mkdtemp(path.join(tmpdir(), "pi-tmux-durable-smoke-"));
const file = path.join(directory, "session.jsonl");
const sessionId = randomUUID();
const entries: unknown[] = [];
const notices: string[] = [];
const errors: string[] = [];
const launches: TmuxLaunch[] = [];
const opened: Fixture[] = [];
const originalFetch = globalThis.fetch;
globalThis.fetch = (): never => {
  throw new Error("Network disabled in smoke");
};
const context: CompletionDeliveryContext = {
  isIdle: () => true,
  sessionManager: {
    getEntries: () => entries,
    getSessionId: () => sessionId,
    getSessionFile: () => file,
  },
  ui: {
    notify: (message, level) => {
      if (level === "error") {
        errors.push(message);
      }
    },
    setStatus: () => {
      /* Headless smoke has no status UI. */
    },
  },
};

async function open(): Promise<Fixture> {
  const handlers: Fixture["handlers"] = new Map();
  const tools: Fixture["tools"] = new Map();
  const host = durableTmuxHost({
    appendEntry: (customType, data) => {
      entries.push({ type: "custom", customType, data });
    },
    exec: async (command, args) => ({
      code: 0,
      stderr: "",
      stdout: execFileSync(command, [...args], { encoding: "utf8", timeout: 5000 }),
    }),
    on: (event, handler) => {
      handlers.set(event, handler);
    },
    registerTool: (tool) => {
      tools.set(tool.name, tool);
    },
    sendUserMessage: (text) => {
      notices.push(text);
    },
  });
  registerTmux(host, { cleanup: { rootDirectory: directory } });
  const fixture = { host, handlers, tools };
  opened.push(fixture);
  await handlers.get("session_start")?.({}, context);
  assert.deepEqual(errors, []);
  return fixture;
}
async function close(fixture: Fixture): Promise<void> {
  await fixture.handlers.get("session_shutdown")?.({}, context);
  opened.splice(opened.indexOf(fixture), 1);
}
async function waitUntil(
  check: () => Promise<boolean>,
  deadline = Date.now() + 5000,
): Promise<void> {
  if (await check()) {
    return;
  }
  assert.ok(Date.now() < deadline, "Smoke timed out");
  await delay(25);
  await waitUntil(check, deadline);
}
try {
  const loader = new DefaultResourceLoader({
    cwd: directory,
    agentDir: directory,
    settingsManager: SettingsManager.inMemory({ packages: [], extensions: [] }),
    additionalExtensionPaths: [fileURLToPath(new globalThis.URL("index.ts", import.meta.url))],
    noSkills: true,
    noThemes: true,
    noPromptTemplates: true,
    noContextFiles: true,
  });
  await loader.reload();
  assert.deepEqual(loader.getExtensions().errors, []);
  const first = await open();
  const stored = await stat(`${file}.tmux-durable`);
  assert.ok(stored.isDirectory());
  await assert.rejects(lock(`${file}.tmux-durable`), /already being held/u);
  const tool = first.tools.get("tmux_exec");
  assert.ok(tool);
  const result = await tool.execute(
    "fixture",
    { command: "sleep 0.5; printf 'OFFLINE_OK\\n'", estimatedDurationSeconds: 10 },
    new globalThis.AbortController().signal,
  );
  launches.push(result.details);
  await close(first);
  await waitUntil(async () => {
    try {
      const status = await readFile(result.details.statusPath, "utf8");
      return status.trim() === "0";
    } catch {
      return false;
    }
  });
  const output = await readFile(result.details.logPath, "utf8");
  assert.equal(output.trim(), "OFFLINE_OK");
  const second = await open();
  await waitUntil(async () => notices.length === 1);
  await second.host.flush?.();
  assert.match(notices[0] ?? "", /OFFLINE_OK/u);
  await close(second);
  const third = await open();
  await delay(50);
  assert.equal(notices.length, 1, "Delivered completion must not replay");
  await close(third);
  assert.deepEqual(errors, []);
  process.stdout.write(
    "PASS: native extension loading, durable lock, real detached command, closed-session completion recovery and delivery deduplication; no model/network calls.\n",
  );
} finally {
  await Promise.all([...opened].map(async (fixture) => close(fixture)));
  globalThis.fetch = originalFetch;
  await Promise.all(
    launches.map(async (launch) => {
      try {
        execFileSync("tmux", ["-L", launch.socketName, "kill-server"], { stdio: "ignore" });
      } catch {
        /* Fixture already exited. */
      }
      await rm(path.dirname(launch.logPath), { recursive: true, force: true });
    }),
  );
  await rm(directory, { recursive: true, force: true });
}
