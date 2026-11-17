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

type SetStatusFn = (key: string, text: string | undefined) => void;

/** Pi fills `model` on both `model_select` and `before_provider_headers` contexts. */
export interface StrataContext {
  readonly model?: { readonly provider: string } | undefined;
  readonly ui: { readonly notify: NotifyFn; readonly setStatus?: SetStatusFn };
}

export interface StrataHost {
  readonly exec: (
    command: string,
    args: string[],
    options?: ExecOptionsLike,
  ) => Promise<ExecResultLike>;
  readonly on: (
    event: "model_select" | "before_provider_headers",
    handler: (event: unknown, ctx: StrataContext) => Promise<void>,
  ) => void;
}

// The Mac's engine warms up in ~30 s; the GPU host also has to boot its scheduled task.
const START_TIMEOUT_MS = 400_000;

// A server that just answered stays up for its idle window, so skip re-probing it every turn.
const SERVING_TRUST_MS = 60_000;
const STATUS_KEY = "strata";

const ENSURE_BY_PROVIDER: Readonly<Record<string, string>> = {
  strata: "strata-local",
  "strata-mac": "ensure-strata-mac",
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
  const servingUntil = new Map<string, number>();

  async function ensure(provider: string | undefined, ctx: StrataContext): Promise<void> {
    const script: string | undefined = ensureScriptFor(provider);
    if (script === undefined || (servingUntil.get(script) ?? 0) > Date.now()) {
      return;
    }
    ctx.ui.setStatus?.(STATUS_KEY, `starting ${provider}...`);
    try {
      const result: ExecResultLike = await host.exec(script, [], { timeout: START_TIMEOUT_MS });
      if (result.code === 0) {
        servingUntil.set(script, Date.now() + SERVING_TRUST_MS);
        return;
      }
      const detail: string = (result.stderr || result.stdout).trim().split("\n").pop() ?? "";
      ctx.ui.notify(`strata start failed: ${detail || `exit ${result.code}`}`, "warning");
    } finally {
      ctx.ui.setStatus?.(STATUS_KEY, undefined);
    }
  }

  host.on("model_select", async (event, ctx) => ensure(providerOf(event), ctx));
  // The idle-stop timers kill a server mid-session, so also warm before each request.
  // The hook is awaited, so the first attempt waits instead of failing with a 502.
  host.on("before_provider_headers", async (_event, ctx) => ensure(ctx.model?.provider, ctx));
}
