// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.
import { spawn } from "node:child_process";
import process from "node:process";
import { mkdirSync, openSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { isColabModel, PROVIDER } from "./src/policy.ts";
import {
  ColabLifecycleRuntime,
  type Config,
  type Deps,
  type EnsureOutcome,
  type RunResult,
  type State,
} from "./src/runtime.ts";

type NotifyFn = (message: string, level?: "error" | "info" | "warning") => void;

export interface ColabContext {
  readonly model: { readonly provider: string } | undefined;
  readonly ui: {
    readonly notify: NotifyFn;
    readonly setStatus: (key: string, text: string | undefined) => void;
  };
}

interface ExecResultLike extends RunResult {
  readonly killed?: boolean;
}

/** Narrow view of pi's ExtensionAPI: only what this extension needs. */
export interface ColabLifecycleHost {
  readonly exec: (
    command: string,
    args: string[],
    options?: { readonly timeout?: number },
  ) => Promise<ExecResultLike>;
  readonly on: (
    event: string,
    handler: (event: unknown, ctx: ColabContext) => Promise<void> | void,
  ) => void;
  readonly registerProvider: (name: string, config: { readonly baseUrl?: string }) => void;
}

// Resolve the real path first: pi loads this through a symlink under ~/.pi/agent/extensions.
const ROOT = path.resolve(realpathSync(import.meta.dirname), "..", "..");
const STATE_FILE =
  process.env["COLAB_QWEN_STATE"] ?? path.join(homedir(), ".cache", "colab-qwen", "state.json");

const CONFIG: Config = {
  colabCommand: path.join(homedir(), ".local", "bin", "colab"),
  upScript: path.join(ROOT, "scripts", "colab-qwen-up.sh"),
  upTimeoutMs: 25 * 60 * 1000,
  watcherScript: path.join(ROOT, "scripts", "colab-qwen-idle-stop.sh"),
};

export function parseState(raw: string): State {
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) {
      return {};
    }
    const url = "url" in parsed && typeof parsed.url === "string" ? parsed.url : undefined;
    const last =
      "lastRequestAt" in parsed && typeof parsed.lastRequestAt === "number"
        ? parsed.lastRequestAt
        : undefined;
    return {
      ...(url === undefined ? {} : { url }),
      ...(last === undefined ? {} : { lastRequestAt: last }),
    };
  } catch {
    return {};
  }
}

export function createDeps(host: Pick<ColabLifecycleHost, "exec">, stateFile: string): Deps {
  return {
    run: async (command, args, timeoutMs) => host.exec(command, [...args], { timeout: timeoutMs }),
    spawnDetached: (command, args) => {
      mkdirSync(path.dirname(stateFile), { recursive: true });
      const log = openSync(path.join(path.dirname(stateFile), "watcher.log"), "a");
      // Double fork through sh so the watcher is reparented to launchd and survives pi exiting.
      const child = spawn("/bin/sh", ["-c", 'nohup "$@" </dev/null &', "sh", command, ...args], {
        detached: true,
        stdio: ["ignore", log, log],
      });
      child.on("error", () => {
        child.unref();
      });
      child.unref();
    },
    readState: () => {
      try {
        return parseState(readFileSync(stateFile, "utf8"));
      } catch {
        return {};
      }
    },
    writeState: (state) => {
      mkdirSync(path.dirname(stateFile), { recursive: true });
      writeFileSync(stateFile, JSON.stringify(state));
    },
    now: () => Date.now(),
  };
}

export function applyOutcome(
  host: Pick<ColabLifecycleHost, "registerProvider">,
  ctx: ColabContext,
  outcome: EnsureOutcome,
): void {
  if (!outcome.ok || outcome.url === undefined) {
    ctx.ui.notify(`google-colab unavailable: ${outcome.reason ?? "unknown error"}`, "warning");
    return;
  }
  host.registerProvider(PROVIDER, { baseUrl: `${outcome.url}/v1` });
}

export function createColabExtension(host: ColabLifecycleHost, deps: Deps, config: Config): void {
  const runtime = new ColabLifecycleRuntime(deps, config);

  const refreshStatus = async (ctx: ColabContext): Promise<void> => {
    ctx.ui.setStatus(PROVIDER, isColabModel(ctx.model) ? await runtime.usageText() : undefined);
  };

  const touchIfColab = (_event: unknown, ctx: ColabContext): void => {
    if (isColabModel(ctx.model)) {
      runtime.touch();
    }
  };

  // Fires before every prompt: starts Colab if it is down, then points the provider at its URL.
  host.on("before_agent_start", async (_event, ctx) => {
    if (!isColabModel(ctx.model)) {
      return;
    }
    ctx.ui.notify("google-colab: checking the Colab runtime (a cold start takes ~10 min)", "info");
    applyOutcome(host, ctx, await runtime.ensureUp());
    await refreshStatus(ctx);
  });

  // Switching into google-colab pre-warms Colab without blocking the model switch.
  host.on("model_select", (event, ctx) => {
    const model =
      typeof event === "object" && event !== null && "model" in event ? event.model : undefined;
    const provider =
      typeof model === "object" && model !== null && "provider" in model
        ? model.provider
        : undefined;
    if (provider !== PROVIDER) {
      ctx.ui.setStatus(PROVIDER, undefined);
    }
    if (provider === PROVIDER) {
      runtime
        .ensureUp()
        .then((outcome) => applyOutcome(host, ctx, outcome))
        .catch(() => false);
    }
  });

  for (const event of ["before_provider_request", "tool_execution_end", "agent_end"]) {
    host.on(event, touchIfColab);
  }

  // Keep the footer balance fresh while a google-colab model is in use.
  host.on("agent_end", async (_event, ctx) => {
    await refreshStatus(ctx);
  });
}

export default function colabLifecycleExtension(host: ColabLifecycleHost): void {
  createColabExtension(host, createDeps(host, STATE_FILE), CONFIG);
}
