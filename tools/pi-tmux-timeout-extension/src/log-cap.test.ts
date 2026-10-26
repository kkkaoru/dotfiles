// This TypeScript file is executed with Bun.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, it } from "vitest";
import { cappedLogCommand, shellLoopWarnings } from "./log-cap.ts";

it("terminates an endless writer with an error once the log cap is reached", () => {
  const directory: string = mkdtempSync(path.join(tmpdir(), "pi-tmux-cap-"));
  const logPath: string = path.join(directory, "output.log");
  const commandExitPath: string = path.join(directory, "command-exit");
  execFileSync(
    "sh",
    [
      "-c",
      cappedLogCommand({
        command: "while :; do echo loop; done",
        commandExitPath,
        logPath,
        maxBytes: 8,
      }),
    ],
    { timeout: 10_000 },
  );
  const log: string = readFileSync(logPath, "utf8");
  const exitCode: string = readFileSync(commandExitPath, "utf8");
  const truncated: boolean = existsSync(path.join(directory, "log-truncated"));
  rmSync(directory, { recursive: true });

  expect(log).toBe("loop\nloo\n[pi-tmux] error: output exceeded 8 bytes; job terminated\n");
  expect(exitCode).toBe("1\n");
  expect(truncated).toBe(true);
});

it("warns about shell loops only in command position", () => {
  expect(shellLoopWarnings("for s in a b; do cat $s; done")).toStrictEqual([
    "WARNING: the command contains a shell loop (for/while/until). Make sure it terminates and never reads its own output.log or other pi-tmux logs; output beyond the log cap terminates the job with an error.",
  ]);
  expect(shellLoopWarnings("cd /tmp && while true; do sleep 1; done")).toHaveLength(1);
  expect(shellLoopWarnings("make; until ready; do sleep 1; done")).toHaveLength(1);
  expect(shellLoopWarnings("if x; then for i in 1; do :; done; fi")).toHaveLength(1);
  expect(shellLoopWarnings("echo waiting for deploy")).toStrictEqual([]);
  expect(shellLoopWarnings("bun run check")).toStrictEqual([]);
});

it("keeps short output and the exit status unchanged", () => {
  const directory: string = mkdtempSync(path.join(tmpdir(), "pi-tmux-cap-"));
  const logPath: string = path.join(directory, "output.log");
  const commandExitPath: string = path.join(directory, "command-exit");
  execFileSync(
    "sh",
    [
      "-c",
      cappedLogCommand({ command: "echo ok; exit 3", commandExitPath, logPath, maxBytes: 64 }),
    ],
    { timeout: 10_000 },
  );
  const log: string = readFileSync(logPath, "utf8");
  const exitCode: string = readFileSync(commandExitPath, "utf8");
  const truncated: boolean = existsSync(path.join(directory, "log-truncated"));
  rmSync(directory, { recursive: true });

  expect(log).toBe("ok\n");
  expect(truncated).toBe(false);
  expect(exitCode).toBe("3\n");
});
