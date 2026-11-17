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
    event: "model_select" | "before_provider_headers" | "session_start" | "session_shutdown",
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

const ENSURE_BY_PROVIDER: Readonly<Record<string, string>> = {
  strata: "strata-local",
  "strata-mac": "ensure-strata-mac",
  "strata-win": "ensure-strata-win",
};

const BIN = path.join(homedir(), ".local", "bin");
const AWAKE_SCRIPT = path.join(BIN, "strata-awake");

/** Endpoints `strata-awake` reports on, in footer order. */
export const WATCHED: readonly string[] = ["strata-win", "strata-mac", "strata-colab"];

export type AwakeState = "awake" | "asleep" | "unknown";

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
  provider: string,
  state: AwakeState | undefined,
  idleMs: number | undefined,
): string {
  if (state === "awake") {
    const left = IDLE_STOP_MS - (idleMs ?? 0);
    return left <= STOPS_SOON_MS
      ? `${provider} awake (idle stops it in ~${Math.ceil(left / 60_000)}m)`
      : `${provider} awake`;
  }
  if (state === "asleep") {
    return `${provider} cold`;
  }
  return `${provider} unknown`;
}

/** One footer line: which endpoints answer now, and which the idle timer is about to stop. */
export function footerText(
  states: Readonly<Record<string, AwakeState>>,
  lastUsed: ReadonlyMap<string, number>,
  now: number,
): string {
  const parts: string[] = [];
  for (const provider of WATCHED) {
    const last = lastUsed.get(provider);
    parts.push(describe(provider, states[provider], last === undefined ? undefined : now - last));
  }
  return `strata: ${parts.join(" · ")}`;
}

export default function strataLifecycle(host: StrataHost): void {
  const servingUntil = new Map<string, number>();
  const lastUsed = new Map<string, number>();
  let states: Readonly<Record<string, AwakeState>> = {};
  let timer: ReturnType<typeof globalThis.setInterval> | undefined = undefined;

  const show = (ctx: StrataContext, text: string | undefined): void => {
    ctx.ui.setStatus?.(STATUS_KEY, text);
  };

  async function refresh(ctx: StrataContext): Promise<void> {
    const result: ExecResultLike = await host.exec(AWAKE_SCRIPT, [], { timeout: PROBE_TIMEOUT_MS });
    const next = parseAwake(result.stdout);
    for (const provider of WATCHED) {
      if (states[provider] === "awake" && next[provider] === "asleep") {
        ctx.ui.notify(`${provider}: went cold; the next request starts it again`, "info");
      }
    }
    states = next;
    show(ctx, footerText(states, lastUsed, Date.now()));
  }

  async function ensure(provider: string | undefined, ctx: StrataContext): Promise<void> {
    const script: string | undefined = ensureScriptFor(provider);
    if (provider !== undefined) {
      lastUsed.set(provider, Date.now());
    }
    if (script === undefined || (servingUntil.get(script) ?? 0) > Date.now()) {
      return;
    }
    show(ctx, `strata: starting ${provider}...`);
    try {
      const result: ExecResultLike = await host.exec(script, [], { timeout: START_TIMEOUT_MS });
      if (result.code === 0) {
        servingUntil.set(script, Date.now() + SERVING_TRUST_MS);
        return;
      }
      const detail: string = (result.stderr || result.stdout).trim().split("\n").pop() ?? "";
      ctx.ui.notify(`strata start failed: ${detail || `exit ${result.code}`}`, "warning");
    } finally {
      await refresh(ctx);
    }
  }

  host.on("model_select", async (event, ctx) => ensure(providerOf(event), ctx));
  // The idle-stop timers kill a server mid-session, so also warm before each request.
  // The hook is awaited, so the first attempt waits instead of failing with a 502.
  host.on("before_provider_headers", async (_event, ctx) => ensure(ctx.model?.provider, ctx));

  host.on("session_start", async (_event, ctx) => {
    await refresh(ctx);
    timer = globalThis.setInterval(() => {
      refresh(ctx).catch((error: unknown) => {
        ctx.ui.notify(`strata status: ${String(error)}`, "warning");
      });
    }, PROBE_INTERVAL_MS);
  });

  host.on("session_shutdown", async () => {
    if (timer !== undefined) {
      globalThis.clearInterval(timer);
      timer = undefined;
    }
  });
}
