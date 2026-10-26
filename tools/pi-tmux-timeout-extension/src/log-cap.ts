// This TypeScript file is executed with Bun.
import fs from "node:fs";
import path from "node:path";
import type { TmuxLaunch } from "./tmux.ts";

// Caps output.log so a command that reads its own log (e.g. `cat pi-tmux-*/output.log`) cannot fill the disk.
export const MAX_LOG_BYTES: number = 512 * 1024 * 1024;
export const LOG_CAP_EXIT_CODE = 1;
export const LOG_TRUNCATED_FILENAME = "log-truncated";
const LOG_CAP_ERROR = `ERROR: output exceeded the ${String(MAX_LOG_BYTES)}-byte log cap, so the job was terminated. A runaway loop, such as a command reading its own output.log, is the usual cause.`;
// Matches for/while/until only in command position, not as words inside arguments.
const SHELL_LOOP_PATTERN = /(?:^|[;&|({\n]|\b(?:do|then|else)\s)\s*(?:for|while|until)\s/u;
const SHELL_LOOP_WARNING =
  "WARNING: the command contains a shell loop (for/while/until). Make sure it terminates and never reads its own output.log or other pi-tmux logs; output beyond the log cap terminates the job with an error.";

export interface CappedLogCommandInput {
  readonly command: string;
  readonly commandExitPath: string;
  readonly logPath: string;
  readonly maxBytes: number;
}

export function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

export function shellLoopWarnings(command: string): readonly string[] {
  return SHELL_LOOP_PATTERN.test(command) ? [SHELL_LOOP_WARNING] : [];
}

function logTruncatedPath(logPath: string): string {
  return path.join(path.dirname(logPath), LOG_TRUNCATED_FILENAME);
}

function isLogTruncated(launch: TmuxLaunch): boolean {
  return fs.existsSync(logTruncatedPath(launch.logPath));
}

export function logCapNotice(launch: TmuxLaunch): string {
  return isLogTruncated(launch) ? `\n${LOG_CAP_ERROR}` : "";
}

// On overflow the writer exits, closing the pipe so the command dies of SIGPIPE on its next write.
// Shortcut: a command that ignores SIGPIPE and stops writing keeps running; use timeoutSeconds for that.
export function cappedLogCommand(input: CappedLogCommandInput): string {
  const limit = String(input.maxBytes);
  const marker: string = shellQuote(logTruncatedPath(input.logPath));
  const exitPath: string = shellQuote(input.commandExitPath);
  const writer = `{ head -c ${limit}; [ -z "$(head -c 1)" ] || { : > ${marker}; printf '\\n[pi-tmux] error: output exceeded %s bytes; job terminated\\n' ${limit}; }; }`;
  return `{ (${input.command}) 2>&1; printf '%s\\n' "$?" > ${exitPath}; } | ${writer} > ${shellQuote(input.logPath)}\n[ ! -e ${marker} ] || printf '%s\\n' ${String(LOG_CAP_EXIT_CODE)} > ${exitPath}`;
}
