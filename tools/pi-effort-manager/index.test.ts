import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import type { Api, Model, ModelThinkingLevel } from "@earendil-works/pi-ai";
import type * as CodingAgent from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { afterAll, expect, it, vi } from "vitest";
import effortManager from "./index.ts";

vi.mock("@earendil-works/pi-coding-agent", async (importOriginal) => {
  const original = await importOriginal<typeof CodingAgent>();
  return {
    ...original,
    getAgentDir: (): string => process.env.PI_EFFORT_INDEX_TEST_DIR ?? os.tmpdir(),
  };
});

interface CommandDefinition {
  handler: (args: string, ctx: ExtensionContext) => Promise<void>;
}

interface ShortcutDefinition {
  handler: (ctx: ExtensionContext) => void;
}

type EventHandler = (event: unknown, ctx: ExtensionContext) => unknown;

const directory = fs.mkdtempSync(path.join(os.tmpdir(), "pi-effort-index-test-"));
process.env.PI_EFFORT_INDEX_TEST_DIR = directory;
fs.writeFileSync(path.join(directory, "settings.json"), "{}\n");

const commands = new Map<string, CommandDefinition>();
const events = new Map<string, EventHandler>();
const flags = new Map<string, unknown>();
const shortcuts = new Map<string, ShortcutDefinition>();
let thinking: ModelThinkingLevel = "medium";
const notify = vi.fn<ExtensionContext["ui"]["notify"]>();

const model = {
  api: "openai-responses",
  baseUrl: "https://example.test",
  contextWindow: 100,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  id: "gpt-5-test",
  input: ["text"],
  maxTokens: 100,
  name: "test",
  provider: "openai",
  reasoning: true,
  thinkingLevelMap: {
    off: null,
    minimal: "minimal",
    low: "low",
    medium: "medium",
    high: "high",
    xhigh: "xhigh",
    max: "max",
  },
} as Model<Api>;

const context = {
  cwd: "/tmp/project",
  getContextUsage: (): { contextWindow: number; percent: number; tokens: number } => ({
    contextWindow: 100,
    percent: 10,
    tokens: 10,
  }),
  isProjectTrusted: (): boolean => false,
  model,
  sessionManager: { getBranch: (): readonly unknown[] => [] },
  ui: { notify, setStatus: vi.fn(), setWorkingMessage: vi.fn() },
} as unknown as ExtensionContext;

function required<Value>(value: Value | undefined, name: string): Value {
  if (value === undefined) {
    throw new Error(`missing ${name}`);
  }
  return value;
}

function createPi(): ExtensionAPI {
  return {
    appendEntry: vi.fn(),
    getFlag: (): undefined => undefined,
    getThinkingLevel: (): ModelThinkingLevel => thinking,
    on: (name: string, handler: EventHandler): void => {
      events.set(name, handler);
    },
    registerCommand: (name: string, definition: unknown): void => {
      commands.set(name, definition as CommandDefinition);
    },
    registerFlag: (name: string, definition: unknown): void => {
      flags.set(name, definition);
    },
    registerShortcut: (name: string, definition: unknown): void => {
      shortcuts.set(name, definition as ShortcutDefinition);
    },
    setThinkingLevel: (level: ModelThinkingLevel): void => {
      thinking = level;
    },
  } as unknown as ExtensionAPI;
}

async function exerciseCommands(): Promise<void> {
  const dynamic = required(commands.get("dynamic-effort"), "dynamic effort command");
  const fast = required(commands.get("fast"), "fast command");
  await dynamic.handler("status", context);
  await dynamic.handler("on", context);
  await dynamic.handler("off", context);
  await dynamic.handler("start low", context);
  await dynamic.handler("end high", context);
  await dynamic.handler("compact max", context);
  await dynamic.handler("reset-effort xhigh", context);
  await dynamic.handler("reset-effort default", context);
  await dynamic.handler("reset 4", context);
  await dynamic.handler("reset default", context);
  await dynamic.handler("invalid", context);
  await fast.handler("invalid", context);
  await fast.handler("on", context);
  await fast.handler("", context);
}

function exerciseEvents(): void {
  required(events.get("session_start"), "session_start")({}, context);
  required(events.get("model_select"), "model_select")({}, context);
  required(events.get("before_agent_start"), "before_agent_start")({}, context);
  required(events.get("turn_end"), "turn_end")({}, context);
  required(events.get("message_end"), "message_end")(
    { message: { role: "assistant", usage: { reasoning: 10 } } },
    context,
  );
  required(events.get("message_end"), "message_end")({ message: { role: "user" } }, context);
  required(events.get("session_before_compact"), "session_before_compact")({}, context);
  required(events.get("session_compact"), "session_compact")({}, context);
  required(events.get("session_compact_failed"), "session_compact_failed")({}, context);
}

afterAll(() => {
  delete process.env.PI_EFFORT_INDEX_TEST_DIR;
  fs.rmSync(directory, { recursive: true });
});

it("registers and executes the complete effort management surface", async () => {
  effortManager(createPi());
  expect([...flags.keys()]).toStrictEqual(["dynamic-effort"]);
  expect([...commands.keys()]).toStrictEqual(["dynamic-effort", "fast"]);
  expect([...shortcuts.keys()]).toStrictEqual(["ctrl+shift+e"]);

  exerciseEvents();
  await exerciseCommands();
  required(shortcuts.get("ctrl+shift+e"), "dynamic shortcut").handler(context);

  expect(
    required(events.get("before_provider_request"), "before_provider_request")(
      { payload: {} },
      context,
    ),
  ).toBeUndefined();
  expect(notify).toHaveBeenCalled();
});
