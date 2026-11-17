// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.
// Strata stops itself after 10 idle minutes, so nothing runs when Pi needs it again.
// This starts it on the model switch without a manual step: `strata-local` here, and
// `ensure-strata-win` on the GPU host, which runs its scheduled task over SSH.
import { homedir } from "node:os";
import path from "node:path";

interface ExecOptionsLike {
  readonly timeout?: number;
}

interface ExecResultLike {
  readonly code: number;
  readonly stderr: string;
  readonly stdout: string;
}

type NotifyFn = (message: string, level?: "error" | "info" | "warning") => void;

export interface StrataContext {
  readonly ui: { readonly notify: NotifyFn };
}

export interface StrataHost {
  readonly exec: (
    command: string,
    args: string[],
    options?: ExecOptionsLike,
  ) => Promise<ExecResultLike>;
  readonly on: (
    event: "model_select",
    handler: (event: unknown, ctx: StrataContext) => Promise<void>,
  ) => void;
}

// The Mac's engine warms up in ~30 s; the GPU host also has to boot its scheduled task.
const START_TIMEOUT_MS = 400_000;

const ENSURE_BY_PROVIDER: Readonly<Record<string, string>> = {
  strata: "strata-local",
  "strata-win": "ensure-strata-win",
};

/** The ensure script that must run before a model of this provider can answer, if any. */
export function ensureScriptFor(provider: string | undefined): string | undefined {
  if (provider === undefined) {
    return undefined;
  }
  const script: string | undefined = ENSURE_BY_PROVIDER[provider];
  return script === undefined ? undefined : path.join(homedir(), ".local", "bin", script);
}

/** Pi's ModelSelectEvent carries the newly active model under `model`. */
export function providerOf(event: unknown): string | undefined {
  if (typeof event !== "object" || event === null || !("model" in event)) {
    return undefined;
  }
  const model: unknown = event.model;
  if (typeof model !== "object" || model === null || !("provider" in model)) {
    return undefined;
  }
  return typeof model.provider === "string" ? model.provider : undefined;
}

export default function strataLifecycle(host: StrataHost): void {
  host.on("model_select", async (event, ctx) => {
    const script: string | undefined = ensureScriptFor(providerOf(event));
    if (script === undefined) {
      return;
    }
    const result: ExecResultLike = await host.exec(script, [], { timeout: START_TIMEOUT_MS });
    if (result.code !== 0) {
      const detail: string = (result.stderr || result.stdout).trim().split("\n").pop() ?? "";
      ctx.ui.notify(`strata start failed: ${detail || `exit ${result.code}`}`, "warning");
    }
  });
}
