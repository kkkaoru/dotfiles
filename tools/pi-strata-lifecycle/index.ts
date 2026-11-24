// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.
// Strata stops itself after 10 idle minutes, so nothing runs when Pi needs it again.
// This starts it on the model switch without a manual step: `strata-local` here, and
// `ensure-strata-win` on the GPU host, which runs its scheduled task over SSH.
// It also mirrors `strata-awake` into the footer, so an endpoint that is up, and one the idle
// Timer is about to stop, are both visible before a request can fail on them.
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
    event:
      | "model_select"
      | "before_provider_headers"
      | "agent_end"
      | "session_start"
      | "session_shutdown",
    handler: (event: unknown, ctx: StrataContext) => Promise<void>,
  ) => void;
}

// The Mac's engine warms up in ~30 s; the GPU host also has to boot its scheduled task.
const START_TIMEOUT_MS = 400_000;
const PROBE_TIMEOUT_MS = 40_000;
const PROBE_INTERVAL_MS = 60_000;

// A server that just answered stays up for its idle window, so skip re-probing it every turn.
const SERVING_TRUST_MS = 60_000;
const STATUS_KEY = "strata";

// Every host stops its Strata after 10 idle minutes; warn once that is close.
const IDLE_STOP_MS = 600_000;
const STOPS_SOON_MS = 180_000;

const BIN = path.join(homedir(), ".local", "bin");
const AWAKE_SCRIPT = path.join(BIN, "strata-awake");

const ENSURE_BY_PROVIDER: Readonly<Record<string, string>> = {
  "bonsai-mac": "ensure-bonsai-mac",
  "inkling-mini": "inkling-mini",
  strata: "strata-local",
  "strata-mac": "ensure-strata-mac",
  "strata-win": "ensure-strata-win",
};

/** Endpoints `strata-awake` reports on, in footer order. */
export const WATCHED = ["strata-win", "strata-mac", "bonsai-mac", "strata-colab"] as const;

export type AwakeState = "awake" | "asleep" | "unknown";

/** The name the footer uses, which has to fit beside the other extensions' statuses. */
const SHORT: Readonly<Record<(typeof WATCHED)[number], string>> = {
  "strata-win": "win",
  "strata-mac": "mac",
  "bonsai-mac": "bonsai",
  "strata-colab": "colab",
};

/** The ensure script that must run before a model of this provider can answer, if any. */
export function ensureScriptFor(provider: string | undefined): string | undefined {
  if (provider === undefined) {
    return undefined;
  }
  const script: string | undefined = ENSURE_BY_PROVIDER[provider];
  return script === undefined ? undefined : path.join(BIN, script);
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

/** Parse `strata-awake` output, ignoring anything that is not a provider state line. */
export function parseAwake(stdout: string): Readonly<Record<string, AwakeState>> {
  const states: Record<string, AwakeState> = {};
  for (const line of stdout.split("\n")) {
    const [provider, state] = line.trim().split(/\s+/u);
    if (
      provider !== undefined &&
      (state === "awake" || state === "asleep" || state === "unknown")
    ) {
      states[provider] = state;
    }
  }
  return states;
}

function describe(
  provider: (typeof WATCHED)[number],
  last: number | undefined,
  now: number,
): string {
  const name = SHORT[provider];
  if (last === undefined) {
    return name;
  }
  const left = IDLE_STOP_MS - (now - last);
  return left <= STOPS_SOON_MS ? `${name} ~${Math.max(1, Math.ceil(left / 60_000))}m` : name;
}

/** Last stderr line of a failed start, so the reason reaches the UI. */
function failureText(result: ExecResultLike): string {
  const detail = (result.stderr || result.stdout).trim().split("\n").pop() ?? "";
  return `strata start failed: ${detail || `exit ${result.code}`}`;
}

/** One footer line: which endpoints answer now, and which the idle timer is about to stop. Only
 *  those: a cold endpoint is the normal state and would cost the line room the awake ones need. */
export function footerText(
  states: Readonly<Record<string, AwakeState>>,
  lastUsed: ReadonlyMap<string, number>,
  now: number,
): string | undefined {
  const parts = WATCHED.flatMap((provider) =>
    states[provider] === "awake" ? [describe(provider, lastUsed.get(provider), now)] : [],
  );
  return parts.length === 0 ? undefined : `awake: ${parts.join(" ")}`;
}

const stopInterval = (timer: ReturnType<typeof globalThis.setInterval> | undefined): undefined => {
  if (timer !== undefined) {
    globalThis.clearInterval(timer);
  }
  return undefined;
};

export default function strataLifecycle(host: StrataHost): void {
  const servingUntil = new Map<string, number>();
  const lastUsed = new Map<string, number>();
  let states: Readonly<Record<string, AwakeState>> = {};
  let timer: ReturnType<typeof globalThis.setInterval> | undefined = undefined;
  // No ctx may outlive its hook: after /reload every ctx property throws, so a timer captured one.
  let pending: { readonly message: string; readonly level: "info" | "warning" }[] = [];

  const show = (ctx: StrataContext): void => {
    ctx.ui.setStatus?.(STATUS_KEY, footerText(states, lastUsed, Date.now()));
  };

  async function probe(): Promise<void> {
    const result: ExecResultLike = await host.exec(AWAKE_SCRIPT, [], { timeout: PROBE_TIMEOUT_MS });
    const next = parseAwake(result.stdout);
    for (const provider of WATCHED) {
      if (states[provider] === "awake" && next[provider] === "asleep") {
        pending.push({
          message: `${provider}: went cold; the next request starts it again`,
          level: "info",
        });
      }
    }
    states = next;
  }

  /** Publishes what the last probe found; only a hook has a ctx that is still live. */
  const publish = (ctx: StrataContext): void => {
    for (const { message, level } of pending) {
      ctx.ui.notify(message, level);
    }
    pending = [];
    show(ctx);
  };

  // A probe that fails stops its own timer, so a replaced instance stops spawning `strata-awake`.
  const probeQuietly = (): void => {
    probe().catch((error: unknown) => {
      timer = stopInterval(timer);
      pending.push({ message: `strata status: ${String(error)}`, level: "warning" });
    });
  };

  async function ensure(provider: string | undefined, ctx: StrataContext): Promise<void> {
    if (provider === undefined) {
      return;
    }
    lastUsed.set(provider, Date.now());
    const script: string | undefined = ensureScriptFor(provider);
    if (script === undefined || (servingUntil.get(script) ?? 0) > Date.now()) {
      return;
    }
    ctx.ui.setStatus?.(STATUS_KEY, `${provider}: starting...`);
    try {
      const result: ExecResultLike = await host.exec(script, [], { timeout: START_TIMEOUT_MS });
      if (result.code === 0) {
        servingUntil.set(script, Date.now() + SERVING_TRUST_MS);
        states = { ...states, [provider]: "awake" };
      } else {
        states = { ...states, [provider]: "asleep" };
        ctx.ui.notify(failureText(result), "warning");
      }
    } finally {
      show(ctx);
    }
  }

  host.on("model_select", async (event, ctx) => {
    await ensure(providerOf(event), ctx);
    publish(ctx);
  });
  // Warm before each request too: the hook is awaited, so a server the idle timer stopped returns.
  host.on("before_provider_headers", async (_event, ctx) => {
    await ensure(ctx.model?.provider, ctx);
    publish(ctx);
  });
  // Probing three endpoints must not hold up startup, so this is not awaited: the next hook publishes.
  host.on("session_start", async () => {
    timer = stopInterval(timer);
    probeQuietly();
    timer = globalThis.setInterval(probeQuietly, PROBE_INTERVAL_MS);
  });

  host.on("agent_end", async (_event, ctx) => {
    publish(ctx);
  });

  host.on("session_shutdown", async () => {
    timer = stopInterval(timer);
  });
}
