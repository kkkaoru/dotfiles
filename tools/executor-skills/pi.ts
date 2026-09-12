// Runs with Bun. Keep automatic delivery and share only the current session's selected agmsg identity.
import { z } from "zod";

interface RoutingContext {
  cwd: string;
  sessionManager: { getEntries(): readonly unknown[] };
}
export interface ToolRoutingHost {
  on(
    event: "session_start" | "before_agent_start",
    handler: (event: unknown, context?: RoutingContext) => unknown,
  ): void;
  getActiveTools(): string[];
  setActiveTools(names: string[]): void;
}
interface ActiveIdentity {
  agent: string;
  teams: string[];
}

const identityEntry = z.object({
  type: z.literal("custom"),
  customType: z.literal("agmsg-active-identity"),
  data: z.union([
    z.object({ state: z.literal("cleared") }),
    z.object({ identity: z.null() }),
    z.object({
      identity: z.object({ agent: z.string(), teams: z.array(z.string()) }),
    }),
  ]),
});

const activeIdentity = (
  entries: readonly unknown[],
): ActiveIdentity | undefined => {
  const latest = entries
    .flatMap((entry) => {
      const parsed = identityEntry.safeParse(entry);
      return parsed.success ? [parsed.data.data] : [];
    })
    .at(-1);
  return latest && "identity" in latest
    ? (latest.identity ?? undefined)
    : undefined;
};

export default function executorToolRouting(host: ToolRoutingHost): void {
  host.on("session_start", () => {
    host.setActiveTools(
      host.getActiveTools().filter((name) => name !== "agmsg"),
    );
  });
  host.on("before_agent_start", (event, context) => {
    if (
      !context ||
      typeof event !== "object" ||
      event === null ||
      !("systemPrompt" in event) ||
      typeof event.systemPrompt !== "string"
    )
      return;
    const identity: ActiveIdentity | undefined = activeIdentity(
      context.sessionManager.getEntries(),
    );
    if (!identity) return;
    return {
      systemPrompt: `${event.systemPrompt}\n\nFor agmsg via Executor, use this Pi session's selected identity (not another registered name): ${JSON.stringify({ project: context.cwd, ...identity })}. Verify it with agmsg_whoami; do not change automatic inbox delivery.`,
    };
  });
}
