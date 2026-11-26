// This file runs with bun.
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";

import { CLI_NAME } from "./constants.ts";

export interface RunCliOptions {
  signal?: AbortSignal;
  env?: NodeJS.ProcessEnv;
}

export interface CliResult {
  stdout: string;
  stderr: string;
}

export class CliError extends Error {
  readonly code: number;
  readonly detail: string;

  constructor(code: number, detail: string) {
    super(`${CLI_NAME} exited with ${String(code)}: ${detail.trim().slice(0, 500)}`);
    this.name = "CliError";
    this.code = code;
    this.detail = detail;
  }
}

/**
 * Resolve the CLI: `GEMINI_API_CLI`, then `PATH`, then the installer's default directory.
 * pi's own environment does not always carry `~/.local/bin`, hence the last step.
 */
export function findCli(
  env: NodeJS.ProcessEnv = process.env,
  exists: (path: string) => boolean = existsSync,
): string | undefined {
  const override = env["GEMINI_API_CLI"];
  if (override) {
    return override;
  }
  const onPath = (env["PATH"] ?? "")
    .split(delimiter)
    .filter((dir) => dir.length > 0)
    .map((dir) => join(dir, CLI_NAME))
    .find((candidate) => exists(candidate));
  if (onPath) {
    return onPath;
  }
  const installed = join(homedir(), ".local", "bin", CLI_NAME);
  return exists(installed) ? installed : undefined;
}

/** Run the CLI and collect its output; a non-zero exit rejects with `CliError`. */
export function runCli(
  binary: string,
  args: readonly string[],
  options: RunCliOptions = {},
): Promise<CliResult> {
  return new Promise<CliResult>((resolve, reject) => {
    const child = spawn(binary, [...args], {
      env: options.env ?? process.env,
      ...(options.signal ? { signal: options.signal } : {}),
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) {
        resolve({ stdout, stderr });
        return;
      }
      reject(new CliError(code ?? 1, stderr || stdout));
    });
  });
}
