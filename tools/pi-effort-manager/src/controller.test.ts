import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import type { Api, Model, ModelThinkingLevel } from "@earendil-works/pi-ai";
import type * as CodingAgent from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { EffortController } from "./controller.ts";

vi.mock("@earendil-works/pi-coding-agent", async (importOriginal) => {
  const original = await importOriginal<typeof CodingAgent>();
  return {
    ...original,
    getAgentDir: (): string => process.env["PI_EFFORT_MANAGER_TEST_DIR"] ?? os.tmpdir(),
  };
});

const directories: string[] = [];
let thinking: ModelThinkingLevel = "medium";
let contextTokenCount = 10;
let branchEntries: unknown[] = [];
const appended: { customType: string; data: unknown }[] = [];
const notify = vi.fn<ExtensionContext["ui"]["notify"]>();
const setStatus = vi.fn<ExtensionContext["ui"]["setStatus"]>();
const setWorkingMessage = vi.fn<ExtensionContext["ui"]["setWorkingMessage"]>();

const reasoningModel = {
  api: "openai-responses",
  baseUrl: "https://example.test",
  contextWindow: 110,
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

const pi = {
  appendEntry: (customType: string, data: unknown): void => {
    appended.push({ customType, data });
  },
  getThinkingLevel: (): ModelThinkingLevel => thinking,
  setThinkingLevel: (level: ModelThinkingLevel): void => {
    thinking = level;
  },
} as ExtensionAPI;

const context = {
  cwd: "/tmp/project",
  getContextUsage: (): { contextWindow: number; percent: number; tokens: number } => ({
    contextWindow: 110,
    percent: contextTokenCount / 1.1,
    tokens: contextTokenCount,
  }),
  isProjectTrusted: (): boolean => false,
  model: reasoningModel,
  sessionManager: { getBranch: (): readonly unknown[] => branchEntries },
  ui: { notify, setStatus, setWorkingMessage },
} as unknown as ExtensionContext;

beforeEach(() => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "pi-effort-controller-test-"));
  directories.push(directory);
  process.env["PI_EFFORT_MANAGER_TEST_DIR"] = directory;
  fs.writeFileSync(
    path.join(directory, "settings.json"),
    JSON.stringify({ compaction: { reserveTokens: 10 } }),
  );
  thinking = "medium";
  contextTokenCount = 10;
  branchEntries = [];
  appended.length = 0;
});

afterEach(() => {
  delete process.env["PI_EFFORT_MANAGER_TEST_DIR"];
  directories.splice(0).map((directory): void => fs.rmSync(directory, { recursive: true }));
});

it("ramps effort, reserves max for compaction, and restores after failure", () => {
  const controller = new EffortController(pi);
  controller.sessionStart(context, "on");
  expect(thinking).toBe("medium");
  expect(controller.dynamicEnabled).toBe(true);
  expect(setStatus).toHaveBeenCalledWith("pi-effort-thinking", "think:medium · dynamic");
  expect(setWorkingMessage).toHaveBeenCalledWith("Working (medium effort · dynamic)...");

  contextTokenCount = 90;
  controller.beforeAgentStart(context);
  expect(thinking).toBe("xhigh");
  controller.beforeCompaction(context);
  expect(thinking).toBe("max");
  controller.compactionFailed(context);
  expect(thinking).toBe("xhigh");

  controller.beforeCompaction(context);
  contextTokenCount = 20;
  controller.compacted(context);
  expect(thinking).toBe("medium");
  expect(controller.status(context)).toContain("compactions=1");
});

it("resets to baseline after compaction and handles model changes", () => {
  branchEntries = [{ type: "compaction" }, { type: "compaction" }];
  const controller = new EffortController(pi);
  contextTokenCount = 90;
  controller.sessionStart(context, "on");
  controller.beforeAgentStart(context);
  expect(thinking).toBe("xhigh");
  controller.beforeCompaction(context);
  controller.compacted(context);
  expect(thinking).toBe("medium");

  controller.beforeAgentStart(context);
  expect(thinking).toBe("medium");
  controller.turnEnded(context);
  expect(thinking).toBe("xhigh");
  controller.modelSelected(context);
  expect(thinking).toBe("xhigh");
});

it("supports dynamic toggles and reasoning observations", () => {
  const controller = new EffortController(pi);
  controller.sessionStart(context, undefined);
  controller.setDynamic(context, true);
  expect(controller.dynamicEnabled).toBe(true);
  controller.setDynamic(context, false);
  expect(controller.dynamicEnabled).toBe(false);
  controller.observeReasoning(context, 100);
  controller.observeReasoning(context, 300);
  expect(controller.status(context)).toContain("medium:200/300");
  expect(appended).toContainEqual({
    customType: "pi-effort-manager-state-v1",
    data: {
      compactionCount: 0,
      enabled: false,
      overrides: {},
      resetEffort: "xhigh",
      resetInterval: 1,
    },
  });
});

it("applies session effort boundaries and a session reset interval", () => {
  const controller = new EffortController(pi);
  controller.sessionStart(context, undefined);
  controller.setSessionPolicy(context, "startEffort", "low");
  controller.setSessionPolicy(context, "endEffort", "high");
  controller.setSessionPolicy(context, "compactionEffort", "xhigh");
  controller.setSessionPolicy(context, "compactionResetEffort", "high");
  controller.setSessionPolicy(context, "compactionResetInterval", "1");
  controller.setDynamic(context, true);
  expect(thinking).toBe("low");
  contextTokenCount = 90;
  controller.beforeAgentStart(context);
  expect(thinking).toBe("low");
  controller.turnEnded(context);
  expect(thinking).toBe("high");
  controller.beforeCompaction(context);
  expect(thinking).toBe("xhigh");
  controller.compacted(context);
  expect(thinking).toBe("low");
  expect(controller.status(context)).toContain("start=low end=high compact=xhigh reset=1@high");
});

it("counts reset intervals only at or above the configured effort depth", () => {
  const controller = new EffortController(pi);
  controller.sessionStart(context, "on");
  controller.beforeCompaction(context);
  controller.compacted(context);
  expect(controller.status(context)).toContain("reset=1@xhigh context=10/110 compactions=0");

  contextTokenCount = 90;
  controller.beforeAgentStart(context);
  expect(thinking).toBe("xhigh");
  controller.beforeCompaction(context);
  controller.compacted(context);
  expect(thinking).toBe("medium");
  expect(controller.status(context)).toContain("compactions=1");
});

it("handles unavailable thinking models and disabled compaction paths", () => {
  const controller = new EffortController(pi);
  const plainContext = {
    ...context,
    getContextUsage: (): undefined => undefined,
    model: { ...reasoningModel, reasoning: false },
  } as unknown as ExtensionContext;

  controller.sessionStart(plainContext, "on");
  controller.setSessionPolicy(plainContext, "compactionResetInterval", "zero");
  controller.beforeCompaction(plainContext);
  controller.compacted(plainContext);
  controller.compactionFailed(plainContext);
  thinking = "off";
  controller.updateUi(plainContext);

  expect(controller.status(plainContext)).toContain("levels=unavailable");
  expect(notify).toHaveBeenCalledWith(
    "Compaction reset interval must be a positive integer.",
    "error",
  );
  expect(setWorkingMessage).toHaveBeenLastCalledWith(undefined);

  const missingModelContext = { ...plainContext, model: undefined } as unknown as ExtensionContext;
  controller.modelSelected(missingModelContext);
  expect(controller.status(missingModelContext)).toContain("context=0/0");
});

it("restores session state, applies the dynamic flag, and persists a default", () => {
  branchEntries = [
    null,
    { customType: "other", data: {}, type: "custom" },
    { customType: "pi-effort-manager-state-v1", data: "invalid", type: "custom" },
    { customType: "pi-effort-manager-state-v1", data: { enabled: "invalid" }, type: "custom" },
    { customType: "pi-effort-manager-state-v1", data: { enabled: false }, type: "custom" },
    {
      customType: "pi-effort-manager-state-v1",
      data: { compactionCount: 2, enabled: true, resetEffort: "xhigh", resetInterval: 1 },
      type: "custom",
    },
  ];
  const controller = new EffortController(pi);
  controller.sessionStart(context, "off");
  expect(thinking).toBe("medium");
  expect(controller.status(context)).toContain("compactions=2");
  controller.setSessionPolicy(context, "compactionResetEffort", "high");
  expect(controller.status(context)).toContain("compactions=0");
  controller.setDynamic(context, true, true);
  expect(controller.dynamicEnabled).toBe(true);

  const settings: unknown = JSON.parse(
    fs.readFileSync(
      path.join(process.env["PI_EFFORT_MANAGER_TEST_DIR"] ?? "", "settings.json"),
      "utf8",
    ),
  );
  expect(settings).toMatchObject({ "pi-effort-manager": { dynamicDefault: true } });
});

it("manages fast mode and priority payloads", () => {
  const controller = new EffortController(pi);
  controller.setFast(context, true);
  expect(controller.fastMode()).toBe(true);
  expect(controller.providerPayload({ model: "gpt-5-test" }, context)).toStrictEqual({
    model: "gpt-5-test",
    service_tier: "priority",
  });
  expect(
    controller.providerPayload({ model: "gpt-5-test", service_tier: "default" }, context),
  ).toBeUndefined();
  controller.setFast(context, false);
  expect(controller.providerPayload({}, context)).toBeUndefined();
  expect(controller.providerPayload(null, context)).toBeUndefined();
  const plainContext = {
    ...context,
    model: { ...reasoningModel, id: "plain", provider: "other" },
  } as unknown as ExtensionContext;
  controller.setFast(plainContext, true);
  expect(controller.providerPayload({}, plainContext)).toBeUndefined();
});
