// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.
import {
  decideModelSwitchAction,
  decideShutdownAction,
  type ModelSwitchInput,
  type OmlxAction,
} from "./policy.ts";

export interface OmlxRunResult {
  readonly code: number;
  readonly stderr: string;
  readonly stdout: string;
}

/** Minimal shell-exec surface the runtime needs. Adapted from pi's ExtensionAPI.exec in index.ts. */
export interface OmlxCommandRunner {
  readonly run: (
    command: string,
    args: readonly string[],
    timeoutMs: number,
  ) => Promise<OmlxRunResult>;
}

export interface OmlxLifecycleConfig {
  readonly ensureCommand: string;
  readonly ensureTimeoutMs: number;
  readonly idleStopCommand: string;
  readonly idleStopTimeoutMs: number;
}

export interface OmlxLifecycleOutcome {
  readonly action: OmlxAction;
  readonly ok: boolean;
  readonly reason?: string;
}

interface CommandPlan {
  readonly command: string;
  readonly timeoutMs: number;
}

/**
 * Starts/stops the managed omlx server in response to pi lifecycle events.
 *
 * Never throws: a missing or failing ensure-omlx/omlx-idle-stop script (e.g. omlx not installed
 * on this machine) is reported back as a non-ok outcome instead of propagating, so a model switch
 * or session shutdown can never be broken by omlx being absent.
 */
export class OmlxLifecycleRuntime {
  private readonly config: OmlxLifecycleConfig;
  private readonly runner: OmlxCommandRunner;

  constructor(runner: OmlxCommandRunner, config: OmlxLifecycleConfig) {
    this.runner = runner;
    this.config = config;
  }

  async onModelSelect(input: ModelSwitchInput): Promise<OmlxLifecycleOutcome> {
    return this.apply(decideModelSwitchAction(input));
  }

  async onSessionShutdown(currentProvider: string | undefined): Promise<OmlxLifecycleOutcome> {
    return this.apply(decideShutdownAction(currentProvider));
  }

  private planFor(action: "start" | "stop"): CommandPlan {
    if (action === "start") {
      return { command: this.config.ensureCommand, timeoutMs: this.config.ensureTimeoutMs };
    }
    return { command: this.config.idleStopCommand, timeoutMs: this.config.idleStopTimeoutMs };
  }

  private async apply(action: OmlxAction): Promise<OmlxLifecycleOutcome> {
    if (action === "none") {
      return { action, ok: true };
    }
    const plan: CommandPlan = this.planFor(action);
    try {
      const result: OmlxRunResult = await this.runner.run(plan.command, [], plan.timeoutMs);
      if (result.code === 0) {
        return { action, ok: true };
      }
      return {
        action,
        ok: false,
        reason: result.stderr.trim() || `exit code ${String(result.code)}`,
      };
    } catch (error) {
      return { action, ok: false, reason: error instanceof Error ? error.message : String(error) };
    }
  }
}
