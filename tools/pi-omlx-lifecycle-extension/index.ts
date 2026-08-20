// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.
import { homedir } from "node:os";
import path from "node:path";
import {
  OmlxLifecycleRuntime,
  type OmlxCommandRunner,
  type OmlxLifecycleConfig,
  type OmlxLifecycleOutcome,
} from "./src/runtime.ts";
import type { ModelSwitchInput } from "./src/policy.ts";

interface ExecOptionsLike {
  readonly timeout?: number;
}

interface ExecResultLike {
  readonly code: number;
  readonly stderr: string;
  readonly stdout: string;
}

type NotifyFn = (message: string, level?: "error" | "info" | "warning") => void;

type LifecycleEvent = "model_select" | "session_shutdown";

/** Narrow view of pi's ExtensionContext: only what notifyOutcome/session_shutdown need. */
export interface OmlxContext {
  readonly model: { readonly provider: string } | undefined;
  readonly ui: { readonly notify: NotifyFn };
}

/**
 * Narrow view of pi's ExtensionAPI: only what this extension needs.
 * A real ExtensionAPI (from "@earendil-works/pi-coding-agent") structurally satisfies this,
 * so pi's extension loader can call this module's default export directly. `on` mirrors the
 * loop/agmsg extensions' pattern of one union-typed signature with an untyped `event`, because
 * pi's own event-specific overloads are not assignable to a differently-shaped overload set here.
 */
export interface OmlxLifecycleHost {
  readonly exec: (
    command: string,
    args: string[],
    options?: ExecOptionsLike,
  ) => Promise<ExecResultLike>;
  readonly on: (
    event: LifecycleEvent,
    handler: (event: unknown, ctx: OmlxContext) => Promise<void> | void,
  ) => void;
}

const START_TIMEOUT_MS = 200_000;
const STOP_TIMEOUT_MS = 10_000;

const CONFIG: OmlxLifecycleConfig = {
  ensureCommand: path.join(homedir(), ".local", "bin", "ensure-omlx"),
  ensureTimeoutMs: START_TIMEOUT_MS,
  idleStopCommand: path.join(homedir(), ".local", "bin", "omlx-idle-stop"),
  idleStopTimeoutMs: STOP_TIMEOUT_MS,
};

function readProviderId(candidate: unknown): string | undefined {
  if (typeof candidate !== "object" || candidate === null || !("provider" in candidate)) {
    return undefined;
  }
  return typeof candidate.provider === "string" ? candidate.provider : undefined;
}

/** Pull the two provider ids pi's real ModelSelectEvent carries out of an untyped event payload. */
export function parseModelSwitchEvent(event: unknown): ModelSwitchInput | undefined {
  if (typeof event !== "object" || event === null || !("model" in event)) {
    return undefined;
  }
  const nextProvider: string | undefined = readProviderId(event.model);
  if (nextProvider === undefined) {
    return undefined;
  }
  const previousProvider: string | undefined =
    "previousModel" in event ? readProviderId(event.previousModel) : undefined;
  return { nextProvider, previousProvider };
}

/**
 * Only pi's real "quit" reason means the process is actually exiting. The other reasons
 * (reload, new, resume, fork) replace the running session in the same still-alive process, and
 * that replacement session's initial model does not itself fire model_select (see model_select
 * doc above), so nudging idle-stop on those reasons could stop a server the next session still
 * needs with nothing left to restart it.
 */
export function isProcessQuit(event: unknown): boolean {
  if (typeof event !== "object" || event === null || !("reason" in event)) {
    return false;
  }
  return event.reason === "quit";
}

/** Adapt the host's exec() to the runtime's minimal OmlxCommandRunner surface. */
export function createRunner(host: Pick<OmlxLifecycleHost, "exec">): OmlxCommandRunner {
  return {
    async run(command, args, timeoutMs): Promise<ExecResultLike> {
      return host.exec(command, [...args], { timeout: timeoutMs });
    },
  };
}

/** Surface a failed start/stop attempt (e.g. omlx not installed on this machine) as a soft warning. */
export function notifyOutcome(ctx: OmlxContext, outcome: OmlxLifecycleOutcome): void {
  if (outcome.action === "none" || outcome.ok) {
    return;
  }
  const verb: string = outcome.action === "start" ? "start" : "idle-stop check";
  ctx.ui.notify(`omlx ${verb} skipped: ${outcome.reason ?? "unknown error"}`, "warning");
}

export default function omlxLifecycleExtension(host: OmlxLifecycleHost): void {
  const runtime = new OmlxLifecycleRuntime(createRunner(host), CONFIG);

  host.on("model_select", async (event, ctx) => {
    const input: ModelSwitchInput | undefined = parseModelSwitchEvent(event);
    if (input === undefined) {
      return;
    }
    const outcome: OmlxLifecycleOutcome = await runtime.onModelSelect(input);
    notifyOutcome(ctx, outcome);
  });

  host.on("session_shutdown", async (event, ctx) => {
    if (!isProcessQuit(event)) {
      return;
    }
    const outcome: OmlxLifecycleOutcome = await runtime.onSessionShutdown(ctx.model?.provider);
    notifyOutcome(ctx, outcome);
  });
}
