// Runs with Bun. Subprocess output containing credentials never reaches logs.
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { Writable } from "node:stream";
import { check, MAX_BYTES } from "./model";

export interface Command {
  executable: string;
  args: string[];
  input: Uint8Array | string;
  identity: string | null;
}
export interface CommandResult {
  code: number;
  output: Buffer;
}
export async function command(options: Command): Promise<CommandResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(options.executable, options.args, {
      stdio: ["pipe", "pipe", "pipe", "pipe"],
      timeout: 30_000,
    });
    const chunks: Buffer[] = [];
    const size = { bytes: 0 };
    child.stdout?.on("data", (data: Buffer) => {
      size.bytes += data.length;
      if (size.bytes > MAX_BYTES) {
        child.kill();
        return;
      }
      chunks.push(data);
    });
    // Consume but never retain stderr: upstream errors may contain secrets.
    child.stderr?.resume();
    child.stdin?.on("error", () => undefined);
    child.stdin?.end(options.input);
    const identityPipe = child.stdio[3];
    if (identityPipe instanceof Writable) {
      identityPipe.on("error", () => undefined);
      identityPipe.end(options.identity);
    }
    child.on("error", () => reject(new Error("Subprocess could not start")));
    child.on("close", (code, signal) => {
      if (signal || size.bytes > MAX_BYTES) {
        reject(new Error("Subprocess interrupted or output limit exceeded"));
        return;
      }
      resolve({ code: code ?? 1, output: Buffer.concat(chunks) });
    });
  });
}
export async function requiredCommand(
  options: Omit<Command, "identity">,
): Promise<Buffer> {
  const result: CommandResult = await command({ ...options, identity: null });
  check(result.code === 0, "Subprocess failed");
  return result.output;
}
export async function readOptional(path: string): Promise<string | null> {
  try {
    return await readFile(path, "utf8");
  } catch (error: unknown) {
    if (
      error !== null &&
      typeof error === "object" &&
      "code" in error &&
      error.code === "ENOENT"
    )
      return null;
    throw new Error("Local file read failed");
  }
}
export async function atomicWrite(
  path: string,
  data: string | Uint8Array,
): Promise<void> {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temporary: string = `${path}.${randomUUID()}.tmp`;
  try {
    await writeFile(temporary, data, { mode: 0o600, flag: "wx" });
    await rename(temporary, path);
  } finally {
    await rm(temporary, { force: true });
  }
}
