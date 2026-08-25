// This TypeScript file is executed with Bun.
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";

const LONG_RUNNING_TIMEOUT_SECONDS = 120;
const LONG_RUNNING_COMMAND = /(?:^|[;&|]\s*)(?:gh\s+run\s+watch|tail\s+-f\b|watch\b)/imu;
const TMUX_COMMAND = /(?:^|\s)tmux(?:\s|$)/iu;
const TMUX_LAUNCH_TIMEOUT_SECONDS = 30;

export interface MutableBashInput {
  command: string;
  timeout?: number;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function shouldDetachBash(input: MutableBashInput): boolean {
  const hasLongTimeout =
    input.timeout !== undefined && input.timeout >= LONG_RUNNING_TIMEOUT_SECONDS;
  return (
    !TMUX_COMMAND.test(input.command) &&
    (hasLongTimeout || LONG_RUNNING_COMMAND.test(input.command))
  );
}

export function detachBashInTmux(input: MutableBashInput, id: number): boolean {
  if (!shouldDetachBash(input)) {
    return false;
  }
  const sessionName = `pi-loop-${String(process.pid)}-${String(id)}`;
  const outputDirectory = path.join(tmpdir(), sessionName);
  const logPath = path.join(outputDirectory, "output.log");
  const statusPath = path.join(outputDirectory, "exit-status");
  const detachedScript = `(${input.command}) > ${shellQuote(logPath)} 2>&1\nexit_code=$?\nprintf '%s\\n' "$exit_code" > ${shellQuote(statusPath)}`;
  const launchMessage = [
    "Started detached loop command.",
    `tmux session: ${sessionName}`,
    `log: ${logPath}`,
    `exit status: ${statusPath}`,
    "Schedule a loop_wakeup to inspect these files later.",
  ].join("\n");
  input.command = `mkdir -p ${shellQuote(outputDirectory)} && tmux new-session -d -s ${shellQuote(sessionName)} -- sh -lc ${shellQuote(detachedScript)} && printf '%s\\n' ${shellQuote(launchMessage)}`;
  input.timeout = TMUX_LAUNCH_TIMEOUT_SECONDS;
  return true;
}
