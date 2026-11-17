// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.
import { spawn } from "node:child_process";
import process from "node:process";
import { mkdirSync, openSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import {
  isColabModel,
  isStrataColabModel,
  PROVIDER,
  STRATA_PROVIDER,
  STRATA_SESSION,
} from "./src/policy.ts";
export { STRATA_PROVIDER } from "./src/policy.ts";
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

/** The Strata engine on Colab publishes its tunnel and key per launch, outside pi's config. */
export const strataStatePath = (): string =>
  process.env["STRATA_STATE"] ?? path.join(homedir(), ".cache", "strata", "state.json");

// Resolve the real path first: pi loads this through a symlink under ~/.pi/agent/extensions.
const ROOT = path.resolve(realpathSync(import.meta.dirname), "..", "..");
const STATE_FILE =
  process.env["COLAB_QWEN_STATE"] ?? path.join(homedir(), ".cache", "colab-qwen", "state.json");

// The Colab CLI wrapper `scripts/colab` adds `--auth=adc`.
// The CLI's default oauth2 strategy needs an authorization code on stdin, which a spawned extension cannot answer.
const COLAB_COMMAND = process.env["COLAB_BIN"] ?? path.join(ROOT, "scripts", "colab");
const CONFIG: Config = {
  colabCommand: COLAB_COMMAND,
  upScript: path.join(ROOT, "scripts", "colab-qwen-up.sh"),
  upTimeoutMs: 25 * 60 * 1000,
  watcherScript: path.join(ROOT, "scripts", "colab-qwen-idle-stop.sh"),
};
const STRATA_CONFIG: Config = {
  colabCommand: COLAB_COMMAND,
  session: STRATA_SESSION,
  upScript: path.join(ROOT, "scripts", "strata-colab-up.sh"),
  upTimeoutMs: 25 * 60 * 1000,
  watcherScript: path.join(ROOT, "scripts", "strata-colab-idle-stop.sh"),
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
  provider: string = PROVIDER,
): void {
  if (!outcome.ok || outcome.url === undefined) {
    ctx.ui.notify(`${provider} unavailable: ${outcome.reason ?? "unknown error"}`, "warning");
    return;
  }
  host.registerProvider(provider, { baseUrl: `${outcome.url}/v1` });
}

export function createColabExtension(
  host: ColabLifecycleHost,
  deps: Deps,
  config: Config,
  strataDeps: Deps = createDeps(host, strataStatePath()),
  strataConfig: Config = STRATA_CONFIG,
): void {
  const runtime = new ColabLifecycleRuntime(deps, config);
  const strataRuntime = new ColabLifecycleRuntime(strataDeps, strataConfig);

  const refreshStatus = async (ctx: ColabContext): Promise<void> => {
    const active = isColabModel(ctx.model) || isStrataColabModel(ctx.model);
    ctx.ui.setStatus(PROVIDER, active ? await runtime.usageText() : undefined);
  };

  const touchIfColab = (_event: unknown, ctx: ColabContext): void => {
    if (isColabModel(ctx.model)) {
      runtime.touch();
    } else if (isStrataColabModel(ctx.model)) {
      strataRuntime.touch();
    }
  };

  // Fires before every prompt: starts Colab if it is down, then points the provider at its URL.
  host.on("before_agent_start", async (_event, ctx) => {
    if (isStrataColabModel(ctx.model)) {
      ctx.ui.notify(
        "strata-colab: checking the Colab L4 runtime (cold start takes several min)",
        "info",
      );
      applyOutcome(host, ctx, await strataRuntime.ensureUp(), STRATA_PROVIDER);
      await refreshStatus(ctx);
      return;
    }
    if (!isColabModel(ctx.model)) {
      return;
    }
    ctx.ui.notify("google-colab: checking the Colab runtime (a cold start takes ~10 min)", "info");
    applyOutcome(host, ctx, await runtime.ensureUp(), PROVIDER);
    await refreshStatus(ctx);
  });

  // Switching into a Colab provider pre-warms its runtime without blocking the model switch.
  host.on("model_select", (event, ctx) => {
    const model =
      typeof event === "object" && event !== null && "model" in event ? event.model : undefined;
    const provider =
      typeof model === "object" && model !== null && "provider" in model
        ? model.provider
        : undefined;
    if (provider !== PROVIDER && provider !== STRATA_PROVIDER) {
      ctx.ui.setStatus(PROVIDER, undefined);
      return;
    }
    if (provider === STRATA_PROVIDER) {
      strataRuntime
        .ensureUp()
        .then((outcome) => applyOutcome(host, ctx, outcome, STRATA_PROVIDER))
        .catch(() => false);
      return;
    }
    runtime
      .ensureUp()
      .then((outcome) => applyOutcome(host, ctx, outcome, PROVIDER))
      .catch(() => false);
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
