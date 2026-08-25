// This TypeScript file is executed with Bun.

const PI_LONG_RUNNING_TIMEOUT_SECONDS = 120;
const CLAUDE_LONG_RUNNING_TIMEOUT_MILLISECONDS = 120_000;
export const CLAUDEX_BACKGROUND_BASH_GUIDANCE =
  "For Bash commands expected to run at least 120 seconds, gh run watch, watch, or tail -f, set run_in_background=true. Claude Code will return a task ID and output path, accept user input while it runs, and deliver a completion notification. End the launch turn promptly and do not poll on a timer.";
const LONG_RUNNING_COMMAND = /(?:^|[;&|]\s*)(?:gh\s+run\s+watch|tail\s+-f\b|watch\b)/imu;
const TMUX_COMMAND = /(?:^|\s)tmux(?:\s|$)/iu;

export interface MutableBashInput {
  command: string;
  timeout?: number;
}

export interface ClaudexBashInput extends MutableBashInput {
  run_in_background?: boolean;
}

function twoDigits(value: number): string {
  return String(value).padStart(2, "0");
}

function isEligibleCommand(command: string): boolean {
  return !TMUX_COMMAND.test(command);
}

export function formatLocalTimestamp(date: Date, format: "completed" | "submitted"): string {
  const time = `${twoDigits(date.getHours())}:${twoDigits(date.getMinutes())}`;
  return format === "completed"
    ? time
    : `${twoDigits(date.getMonth() + 1)}-${twoDigits(date.getDate())} ${time}`;
}

export function shouldDetachBash(input: MutableBashInput): boolean {
  const hasLongTimeout =
    input.timeout !== undefined && input.timeout >= PI_LONG_RUNNING_TIMEOUT_SECONDS;
  return (
    isEligibleCommand(input.command) && (hasLongTimeout || LONG_RUNNING_COMMAND.test(input.command))
  );
}

export function shouldBackgroundClaudexBash(input: ClaudexBashInput): boolean {
  const hasLongTimeout =
    input.timeout !== undefined && input.timeout >= CLAUDE_LONG_RUNNING_TIMEOUT_MILLISECONDS;
  return (
    input.run_in_background !== true &&
    isEligibleCommand(input.command) &&
    (hasLongTimeout || LONG_RUNNING_COMMAND.test(input.command))
  );
}
