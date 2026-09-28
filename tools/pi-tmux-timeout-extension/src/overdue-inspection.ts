// This TypeScript file is executed with Bun.
import type { TmuxLaunch } from "./tmux.ts";

const MAX_COMPLETION_IDENTITY_CHARACTERS = 160;

export function completionIdentity(command: string): string {
  return command.replaceAll(/\s+/gu, " ").trim().slice(0, MAX_COMPLETION_IDENTITY_CHARACTERS);
}

export function overduePrompt(launches: readonly TmuxLaunch[]): string {
  return [
    `tmux overdue check-in: ${String(launches.length)} task(s) exceeded their estimated duration and have not completed. They are still tracked, not failed or stopped.`,
    "Use read on the exact log path below and inspect process state now. A successful log read acknowledges this check-in, not job completion; failed reads and merely receiving this notice do not. For an open-ended watcher, finish the observation and stop only that specific job when appropriate. For useful ongoing work, arrange a bounded next check. Do not just repeat this notice, launch a duplicate, or wait forever for an exit-status file.",
    ...launches.map((launch: TmuxLaunch): string =>
      [
        `task: ${completionIdentity(launch.taskCommand)}`,
        `tmux socket: ${launch.socketName}; session: ${launch.sessionName}`,
        `log: ${launch.logPath}`,
        `status: ${launch.statusPath}`,
      ].join("\n"),
    ),
  ].join("\n");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function inspectedLogPath(event: unknown): string | undefined {
  if (
    !isRecord(event) ||
    event["toolName"] !== "read" ||
    event["isError"] !== false ||
    !isRecord(event["input"]) ||
    typeof event["input"]["path"] !== "string"
  ) {
    return undefined;
  }
  return event["input"]["path"].replace(/^@/u, "");
}
