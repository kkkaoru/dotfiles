// This TypeScript file is executed with Bun.

const PI_LONG_RUNNING_TIMEOUT_SECONDS = 30;
const CLAUDE_LONG_RUNNING_TIMEOUT_MILLISECONDS = 30_000;
export const CLAUDEX_BACKGROUND_BASH_GUIDANCE =
  "Prefer run_in_background=true for any Bash command that may block, has uncertain duration, accesses external services, or performs tests, builds, deploys, containers, database/data work, or broad repository inspection. Use foreground Bash only for bounded local commands confidently expected to finish within 30 seconds, and give intentionally foregrounded external or otherwise risky commands an explicit timeout below 30 seconds. Claude Code will return a task ID and output path, accept user input while background work runs, and deliver a completion notification. End the launch turn promptly and do not poll on a timer.";
const LONG_RUNNING_COMMAND = /(?:^|[;&|]\s*)(?:gh\s+run\s+watch|tail\s+-f\b|watch\b)/imu;
const PACKAGE_WORK_COMMAND =
  /\b(?:bun|npm|pnpm|yarn|deno)\s+(?:run\s+)?(?:build|check|ci|coverage|install|lint|test|typecheck)\b/iu;
const LANGUAGE_WORK_COMMAND =
  /\b(?:actionlint|cargo\s+(?:build|check|clippy|test)|go\s+test|mypy|oxfmt\s+--check|oxlint|pytest|pyright|ruff\s+check|swift\s+(?:build|test)|tsc\b|uv\s+run\s+(?:mypy|python|pytest|ruff)|xcodebuild)\b/iu;
const INFRASTRUCTURE_WORK_COMMAND =
  /\b(?:(?:docker|podman)\s+(?:build|compose|pull|push|run)|git\s+(?:clone|fetch|pull|push)|kubectl\s+(?:apply|rollout|wait)|(?:pulumi|terraform)\s+(?:apply|destroy|plan|preview|up)|wrangler\s+(?:deploy|dev|tail))\b/iu;
const DATA_WORK_COMMAND =
  /\b(?:alembic|backfill|dbt\b|flyway|liquibase|migration|prisma\s+migrate|train(?:ing)?)\b/iu;
const EXTERNAL_COMMAND = /\b(?:curl|rsync|scp|ssh|wget)\b/iu;
const BROAD_INSPECTION_COMMAND = /\b(?:du\s+-|find\s+[^;&|]*|git\s+diff\s+--stat\b)/iu;
const TMUX_COMMAND = /(?:^|\s)tmux(?:\s|$)/iu;
const CONFIDENTLY_SHORT_LOCAL_COMMAND =
  /^\s*(?:(?:(?:date|echo|false|ls|printf|pwd|true|uname|which)\b|command\s+-v\b|git\s+(?:branch|rev-parse|status)\b)[^;&|]*(?:\s*(?:&&|\|\||;)\s*|$))+\s*$/iu;
const POTENTIALLY_BLOCKING_COMMANDS = [
  LONG_RUNNING_COMMAND,
  PACKAGE_WORK_COMMAND,
  LANGUAGE_WORK_COMMAND,
  INFRASTRUCTURE_WORK_COMMAND,
  DATA_WORK_COMMAND,
  EXTERNAL_COMMAND,
  BROAD_INSPECTION_COMMAND,
] as const satisfies readonly RegExp[];

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

function isPotentiallyBlocking(command: string): boolean {
  return POTENTIALLY_BLOCKING_COMMANDS.some((pattern: RegExp): boolean => pattern.test(command));
}

function isConfidentlyShortLocal(command: string): boolean {
  return CONFIDENTLY_SHORT_LOCAL_COMMAND.test(command);
}

function shouldRunInBackground(input: MutableBashInput, longRunningTimeout: number): boolean {
  const hasLongTimeout = input.timeout !== undefined && input.timeout >= longRunningTimeout;
  const hasNoForegroundBudget = input.timeout === undefined;
  return (
    isEligibleCommand(input.command) &&
    (hasLongTimeout ||
      (hasNoForegroundBudget &&
        (isPotentiallyBlocking(input.command) || !isConfidentlyShortLocal(input.command))))
  );
}

export function formatLocalTimestamp(date: Date, format: "completed" | "submitted"): string {
  const time = `${twoDigits(date.getHours())}:${twoDigits(date.getMinutes())}`;
  return format === "completed"
    ? time
    : `${twoDigits(date.getMonth() + 1)}-${twoDigits(date.getDate())} ${time}`;
}

export function shouldDetachBash(input: MutableBashInput): boolean {
  return shouldRunInBackground(input, PI_LONG_RUNNING_TIMEOUT_SECONDS);
}

export function shouldBackgroundClaudexBash(input: ClaudexBashInput): boolean {
  return (
    input.run_in_background !== true &&
    shouldRunInBackground(input, CLAUDE_LONG_RUNNING_TIMEOUT_MILLISECONDS)
  );
}
