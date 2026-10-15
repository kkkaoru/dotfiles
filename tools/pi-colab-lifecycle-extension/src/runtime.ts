// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.
import { formatUsage, parseUsage, SESSION } from "./policy.ts";

const USAGE_TTL_MS = 60_000;

export interface RunResult {
  readonly code: number;
  readonly stderr: string;
  readonly stdout: string;
}

export interface State {
  readonly url?: string;
  readonly lastRequestAt?: number;
}

/** Everything the runtime touches outside itself, injected so tests need no Colab, files or clock. */
export interface Deps {
  readonly run: (command: string, args: readonly string[], timeoutMs: number) => Promise<RunResult>;
  readonly spawnDetached: (command: string, args: readonly string[]) => void;
  readonly readState: () => State;
  readonly writeState: (state: State) => void;
  readonly now: () => number;
}

export interface Config {
  readonly colabCommand: string;
  readonly upScript: string;
  readonly upTimeoutMs: number;
  readonly watcherScript: string;
}

export interface EnsureOutcome {
  readonly ok: boolean;
  readonly started: boolean;
  readonly url?: string;
  readonly reason?: string;
}

/**
 * Starts Colab on demand and keeps a detached idle watcher running. Never throws: failures come
 * back as a non-ok outcome so a failing Colab can never break a model switch or a prompt.
 */
export class ColabLifecycleRuntime {
  private readonly deps: Deps;
  private readonly config: Config;
  private starting: Promise<EnsureOutcome> | undefined;
  private usage: { readonly text: string | undefined; readonly at: number } | undefined;

  constructor(deps: Deps, config: Config) {
    this.deps = deps;
    this.config = config;
  }

  /** Record that the google-colab model is being used; the watcher measures idleness from this. */
  touch(): void {
    this.deps.writeState({ ...this.deps.readState(), lastRequestAt: this.deps.now() });
  }

  /** Footer text with the remaining Colab compute units; cached so frequent events stay cheap. */
  async usageText(): Promise<string | undefined> {
    const now = this.deps.now();
    if (this.usage !== undefined && now - this.usage.at < USAGE_TTL_MS) {
      return this.usage.text;
    }
    const result = await this.deps.run(this.config.colabCommand, ["usage"], 30_000);
    const info = result.code === 0 ? parseUsage(result.stdout) : undefined;
    this.usage = { text: info === undefined ? undefined : formatUsage(info), at: now };
    return this.usage.text;
  }

  async ensureUp(): Promise<EnsureOutcome> {
    this.starting ??= this.start().finally(() => {
      this.starting = undefined;
    });
    return this.starting;
  }

  private async sessionAlive(): Promise<boolean> {
    const result = await this.deps.run(this.config.colabCommand, ["sessions"], 30_000);
    return result.code === 0 && result.stdout.includes(`[${SESSION}]`);
  }

  private async start(): Promise<EnsureOutcome> {
    this.touch();
    const known = this.deps.readState().url;
    if (known !== undefined && (await this.sessionAlive())) {
      this.ensureWatcher();
      return { ok: true, started: false, url: known };
    }
    this.deps.writeState({ lastRequestAt: this.deps.now() });
    const result = await this.deps.run(this.config.upScript, [], this.config.upTimeoutMs);
    const { url } = this.deps.readState();
    if (result.code !== 0 || url === undefined) {
      return { ok: false, started: true, reason: `colab start failed (exit ${result.code})` };
    }
    this.touch();
    this.ensureWatcher();
    return { ok: true, started: true, url };
  }

  private ensureWatcher(): void {
    this.deps.spawnDetached(this.config.watcherScript, []);
  }
}
