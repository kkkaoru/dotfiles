// This file runs with Bun.
import {
  query,
  type EffortLevel,
  type Options,
  type Query,
  type SDKMessage,
  type SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import type { Tool } from "@earendil-works/pi-ai";
import { AsyncQueue } from "./async-queue.ts";
import { createPiMcpServer, PendingToolCalls, PI_MCP_SERVER, sdkToolNames } from "./tool-bridge.ts";

export interface SessionSettings {
  readonly cwd: string;
  readonly effort: EffortLevel | undefined;
  readonly model: string;
  readonly systemPrompt: string;
  readonly thinking: boolean;
  readonly tools: readonly Tool[];
}

export interface PumpFailure {
  readonly type: "pump_failure";
  readonly error: unknown;
}

export type SessionEvent = PumpFailure | SDKMessage;
/** The part of the SDK `Query` a session uses; tests substitute a scripted fake. */
export type SessionQuery = AsyncIterable<SDKMessage> & Pick<Query, "applyFlagSettings" | "close">;
export type StartQuery = (params: {
  prompt: AsyncIterable<SDKUserMessage>;
  options: Options;
}) => SessionQuery;

/**
 * Credentials that would move the child off the Claude plan login: an API key bills the API,
 * and a base URL (e.g. a claudex adapter shell) would send the request elsewhere.
 */
const PLAN_OVERRIDING_ENV: ReadonlySet<string> = new Set([
  "ANTHROPIC_API_KEY",
  "ANTHROPIC_AUTH_TOKEN",
  "ANTHROPIC_BASE_URL",
]);

/* An enclosing Claude Code session's identity (CLAUDECODE, CLAUDE_CODE_ENTRYPOINT=cli, ...)
   must not leak into the child; the SDK labels its own traffic. */
const PARENT_SESSION_PREFIX = "CLAUDE_CODE_";

function isInherited(key: string): boolean {
  return (
    PLAN_OVERRIDING_ENV.has(key) || key === "CLAUDECODE" || key.startsWith(PARENT_SESSION_PREFIX)
  );
}

export function childEnv(env: NodeJS.ProcessEnv): Record<string, string | undefined> {
  return Object.fromEntries(Object.entries(env).filter(([key]) => !isInherited(key)));
}

export function sessionOptions(settings: SessionSettings, pending: PendingToolCalls): Options {
  return {
    model: settings.model,
    cwd: settings.cwd,
    /* Pi supplies every tool and the whole system prompt; Claude Code's own tools,
       settings, hooks, plugins and MCP config stay out of the child. */
    tools: [],
    settingSources: [],
    strictMcpConfig: true,
    systemPrompt: settings.systemPrompt,
    mcpServers: { [PI_MCP_SERVER]: createPiMcpServer(settings.tools, pending) },
    allowedTools: sdkToolNames(settings.tools),
    includePartialMessages: true,
    persistSession: false,
    thinking: settings.thinking ? { type: "adaptive" } : { type: "disabled" },
    ...(settings.effort === undefined ? {} : { effort: settings.effort }),
    env: childEnv(process.env),
  };
}

/** One long-lived Agent SDK conversation that pi drives turn by turn. */
export class ClaudeSession {
  readonly events = new AsyncQueue<SessionEvent>();
  readonly pending = new PendingToolCalls();
  readonly settings: SessionSettings;
  /** Timestamp of this provider's last reply, used to detect a branched pi transcript. */
  replyTimestamp: number | undefined;
  /** Tool calls handed to pi whose results must arrive on the next turn. */
  awaitedToolIds: readonly string[] = [];
  effort: EffortLevel | undefined;
  readonly #input = new AsyncQueue<SDKUserMessage>();
  readonly #query: SessionQuery;
  #closed = false;

  constructor(settings: SessionSettings, startQuery: StartQuery = query) {
    this.settings = settings;
    this.effort = settings.effort;
    this.#query = startQuery({
      prompt: this.#input,
      options: sessionOptions(settings, this.pending),
    });
    this.#pump().catch(() => null);
  }

  get closed(): boolean {
    return this.#closed;
  }

  send(message: SDKUserMessage): void {
    this.#input.push(message);
  }

  async setEffort(effort: EffortLevel): Promise<void> {
    await this.#query.applyFlagSettings({ effortLevel: effort });
    this.effort = effort;
  }

  close(reason: string): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.pending.rejectAll(new Error(reason));
    this.#input.end();
    this.#query.close();
    this.events.end();
  }

  async #pump(): Promise<void> {
    try {
      for await (const message of this.#query) {
        this.events.push(message);
      }
    } catch (error) {
      this.events.push({ type: "pump_failure", error });
    } finally {
      this.events.end();
    }
  }
}
