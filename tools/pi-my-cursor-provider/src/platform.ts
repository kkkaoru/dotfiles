import {
  createAgentPlatform,
  type AgentOptions,
  type CursorAgentPlatform,
  type ToolName,
} from "@cursor/sdk";
import { getProcessCursorStore } from "./store.ts";

const PREWARM_TOOLS: ToolName[] = ["mcp", "webSearch", "semSearch", "shell"];

let enabled = true;
let platformPromise: Promise<CursorAgentPlatform | undefined> | undefined;
let releasePrewarm: (() => Promise<void>) | undefined;

async function init(
  apiKey: string | undefined,
  cwd: string,
): Promise<CursorAgentPlatform | undefined> {
  try {
    const platform = await createAgentPlatform({
      localStore: getProcessCursorStore(),
    });
    const options: AgentOptions = {
      model: { id: "auto" },
      tools: [...PREWARM_TOOLS],
      ...(apiKey ? { apiKey } : {}),
      local: {
        cwd,
        settingSources: [],
        store: getProcessCursorStore(),
      },
    };
    releasePrewarm = await platform.prewarmLocalWorkspace(options);
    return platform;
  } catch (error) {
    console.warn("Failed to prewarm Cursor workspace:", error);
    return undefined;
  }
}

export async function ensureCursorPlatform(
  apiKey: string | undefined,
  cwd: string,
): Promise<CursorAgentPlatform | undefined> {
  if (!enabled) return undefined;
  platformPromise ??= init(apiKey, cwd);
  return platformPromise;
}

export async function warmCursorWorkspace(apiKey?: string, cwd = process.cwd()): Promise<void> {
  await ensureCursorPlatform(apiKey, cwd);
}

export const cursorPlatformTestApi = {
  reset(): void {
    platformPromise = undefined;
    const release = releasePrewarm;
    releasePrewarm = undefined;
    void release?.().catch(() => undefined);
  },
  setEnabled(value: boolean): void {
    enabled = value;
  },
};
