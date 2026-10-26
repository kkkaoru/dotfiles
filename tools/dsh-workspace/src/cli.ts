#!/usr/bin/env bun
/** Runs with bun. Process boundary: real effects for dsh-workspace. */

import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  accessSync,
  constants,
  openSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
} from "node:fs";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { run } from "./app.ts";
import type { DirectoryKind, Effects, FetchRequest, FetchResponse, SpawnedHost } from "./types.ts";

const HARNESS_HOME_ENVIRONMENT = "DSH_HOME";
const PATH_ENVIRONMENT = "PATH";
const DEFAULT_HARNESS_HOME = ".dsh";
const BUNDLED_DSH = "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh";
const DSH_COMMAND = "dsh";
const HOST_PROFILE_ARGUMENTS = ["--profile", "web", "--no-open"];
const PORT_ARGUMENT = "--port";
const COOKIE_HEADER = "cookie";
const SET_COOKIE_HEADER = "set-cookie";
const CONTENT_TYPE_HEADER = "content-type";
const JSON_CONTENT_TYPE = "application/json";
const PATH_SEPARATOR = ":";
const NO_CONTENT = 0;
const LOOPBACK = "127.0.0.1";
const LOG_SUFFIX = ".log";

function harnessHome(): string {
  return process.env[HARNESS_HOME_ENVIRONMENT] ?? path.join(homedir(), DEFAULT_HARNESS_HOME);
}

function canonicalDirectory(directory: string): string {
  try {
    return realpathSync(directory);
  } catch {
    return directory;
  }
}

function directoryKind(directory: string): DirectoryKind {
  try {
    return statSync(directory).isDirectory() ? "directory" : "file";
  } catch {
    return "missing";
  }
}

function temporaryFile(prefix: string): string {
  return path.join(tmpdir(), `${prefix}${randomUUID()}${LOG_SUFFIX}`);
}

function readText(file: string): string | undefined {
  try {
    return readFileSync(file, "utf8");
  } catch {
    return undefined;
  }
}

function removeFile(file: string): void {
  rmSync(file, { force: true });
}

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(NO_CONTENT, LOOPBACK, () => {
      const address = server.address();
      const port = typeof address === "object" && address !== null ? address.port : NO_CONTENT;
      server.close(() => {
        resolve(port);
      });
    });
  });
}

function isExecutable(candidate: string): boolean {
  try {
    accessSync(candidate, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function findInPath(command: string): string | undefined {
  const directories = (process.env[PATH_ENVIRONMENT] ?? "").split(PATH_SEPARATOR);
  return directories
    .map((directory) => path.join(directory, command))
    .find((candidate) => isExecutable(candidate));
}

function dshBinary(): string {
  return findInPath(DSH_COMMAND) ?? BUNDLED_DSH;
}

function startHost(options: { readonly port: number; readonly logPath: string }): SpawnedHost {
  const descriptor = openSync(options.logPath, "w");
  const child = spawn(
    dshBinary(),
    [...HOST_PROFILE_ARGUMENTS, PORT_ARGUMENT, String(options.port)],
    { detached: true, stdio: ["ignore", descriptor, descriptor] },
  );
  return {
    running: () => child.exitCode === null && child.signalCode === null,
    stop: () => {
      child.kill("SIGTERM");
    },
  };
}

function requestHeaders(request: FetchRequest): Record<string, string> {
  const headers: Record<string, string> = {};
  if (request.body !== undefined) {
    headers[CONTENT_TYPE_HEADER] = JSON_CONTENT_TYPE;
  }
  if (request.cookie !== undefined) {
    headers[COOKIE_HEADER] = request.cookie;
  }
  return headers;
}

async function fetchResponse(request: FetchRequest): Promise<FetchResponse> {
  const init: RequestInit = {
    method: request.method,
    headers: requestHeaders(request),
    redirect: "manual",
  };
  const response = await fetch(
    request.url,
    request.body === undefined ? init : { ...init, body: request.body },
  );
  const setCookie = response.headers.get(SET_COOKIE_HEADER) ?? undefined;
  return {
    status: response.status,
    body: await response.text(),
    ...(setCookie === undefined ? {} : { setCookie }),
  };
}

async function sleep(milliseconds: number): Promise<void> {
  await new Promise((resolve) => {
    setTimeout(resolve, milliseconds);
  });
}

const effects: Effects = {
  harnessHome,
  canonicalDirectory,
  directoryKind,
  temporaryFile,
  readText,
  removeFile,
  freePort,
  startHost,
  fetchResponse,
  sleep,
};

const result = await run({ argv: process.argv.slice(2), cwd: process.cwd(), effects });
if (result.stdout !== "") {
  process.stdout.write(`${result.stdout}\n`);
}
if (result.stderr !== "") {
  process.stderr.write(`${result.stderr}\n`);
}
process.exitCode = result.exitCode;
