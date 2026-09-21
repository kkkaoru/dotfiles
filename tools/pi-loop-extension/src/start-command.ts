// This TypeScript file is executed with Bun.
import { AUTONOMOUS_PROMPT, commandPrompt } from "./helpers.ts";
import { formatInterval, type LoopCommand } from "./parser.ts";
import type { LoopJobState as LoopJob } from "./state.ts";

export interface StartCommandScheduleInput {
  readonly delayMs: number;
  readonly intervalMs?: number;
  readonly prompt: string;
  readonly reason: string;
}

export interface StartCommandHost {
  readonly notify: (message: string, level: "info") => void;
  readonly now: () => number;
  readonly schedule: (input: StartCommandScheduleInput) => LoopJob;
  readonly send: (
    prompt: string,
    identity: string,
    submittedAt: number,
    completedAt: number,
  ) => void;
}

export function startLoopCommand(
  command: Extract<LoopCommand, { readonly kind: "start" }>,
  host: StartCommandHost,
): void {
  const prompt: string = command.prompt.length === 0 ? AUTONOMOUS_PROMPT : command.prompt;
  if (command.intervalMs === undefined) {
    const now: number = host.now();
    host.send(
      commandPrompt(command.prompt),
      `self-paced | ${command.prompt.length === 0 ? "continue established work" : command.prompt}`,
      now,
      now,
    );
    host.notify("Started a self-paced loop.", "info");
    return;
  }
  const job: LoopJob = host.schedule({
    delayMs: command.intervalMs,
    intervalMs: command.intervalMs,
    prompt,
    reason: `Recurring every ${formatInterval(command.intervalMs)}`,
  });
  host.send(prompt, `#${String(job.id)} | ${job.reason}`, job.submittedAt, job.submittedAt);
  host.notify(
    `Started loop #${String(job.id)} every ${formatInterval(command.intervalMs)} (session-scoped).`,
    "info",
  );
}
