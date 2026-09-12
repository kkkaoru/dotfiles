// Runs with Bun. All subprocesses and filesystem operations are mocked.
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { beforeEach, expect, it, vi } from "vitest";
import { atomicWrite, command, readOptional, requiredCommand } from "./io";
import { MAX_BYTES } from "./model";

const mocks = vi.hoisted(() => ({
  spawn: vi.fn(),
  mkdir: vi.fn(),
  read: vi.fn(),
  rename: vi.fn(),
  rm: vi.fn(),
  write: vi.fn(),
}));
vi.mock("node:child_process", () => ({ spawn: mocks.spawn }));
vi.mock("node:fs/promises", () => ({
  mkdir: mocks.mkdir,
  readFile: mocks.read,
  rename: mocks.rename,
  rm: mocks.rm,
  writeFile: mocks.write,
}));
function child() {
  const process = Object.assign(new EventEmitter(), {
    stdout: new PassThrough(),
    stderr: new PassThrough(),
    stdin: new PassThrough(),
    stdio: [null, null, null, new PassThrough()],
    kill: vi.fn(),
  });
  mocks.spawn.mockReturnValue(process);
  return process;
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.mkdir.mockResolvedValue(undefined);
  mocks.write.mockResolvedValue(undefined);
  mocks.rename.mockResolvedValue(undefined);
  mocks.rm.mockResolvedValue(undefined);
});
it("pipes keys through an extra FD and captures bounded stdout", async () => {
  const process = child();
  const pending = command({
    executable: "/mock/age",
    args: ["-d"],
    input: "CIPHER",
    identity: "TEST-IDENTITY",
  });
  process.stdout.write("decoded");
  process.stderr.write("SECRET-ERROR-MUST-NOT-BE-RETURNED");
  process.emit("close", 0, null);
  const result = await pending;
  expect(result.code).toBe(0);
  expect(result.output.toString()).toBe("decoded");
  expect(process.stdio[3]?.read().toString()).toBe("TEST-IDENTITY");
});
it("rejects signals and kills oversized subprocess output", async () => {
  const process = child();
  const pending = command({
    executable: "test",
    args: [],
    input: "",
    identity: null,
  });
  process.stdout.write(Buffer.alloc(MAX_BYTES + 1));
  process.emit("close", null, "SIGTERM");
  await expect(pending).rejects.toThrow("Subprocess interrupted");
  expect(process.kill).toHaveBeenCalledTimes(1);
});
it("does not reveal spawn errors", async () => {
  const process = child();
  const pending = command({
    executable: "test",
    args: [],
    input: "",
    identity: null,
  });
  process.emit("error", new Error("secret value"));
  await expect(pending).rejects.toThrow("Subprocess could not start");
});
it("handles null subprocess exit codes", async () => {
  const process = child();
  const pending = command({
    executable: "test",
    args: [],
    input: "",
    identity: null,
  });
  process.emit("close", null, null);
  expect((await pending).code).toBe(1);
});
it("requires successful subprocesses", async () => {
  const process = child();
  const pending = requiredCommand({ executable: "test", args: [], input: "" });
  process.emit("close", 1, null);
  await expect(pending).rejects.toThrow("Subprocess failed");
});
it("returns successful required output", async () => {
  const process = child();
  const pending = requiredCommand({ executable: "test", args: [], input: "" });
  process.stdout.write("ok");
  process.emit("close", 0, null);
  expect((await pending).toString()).toBe("ok");
});
it("reads existing files", async () => {
  mocks.read.mockResolvedValue("text");
  expect(await readOptional("/mock/file")).toBe("text");
});
it("treats only ENOENT as absence", async () => {
  mocks.read.mockRejectedValue({ code: "ENOENT" });
  expect(await readOptional("/mock/file")).toBeNull();
});
it("does not swallow read permission failures", async () => {
  mocks.read.mockRejectedValue({ code: "EACCES" });
  await expect(readOptional("/mock/file")).rejects.toThrow(
    "Local file read failed",
  );
});
it("does not swallow unknown read failures", async () => {
  mocks.read.mockRejectedValue(null);
  await expect(readOptional("/mock/file")).rejects.toThrow(
    "Local file read failed",
  );
});
it("atomically replaces files with private permissions", async () => {
  await atomicWrite("/mock/file", "TEST-DATA");
  expect(mocks.write.mock.calls[0]?.[2]).toStrictEqual({
    mode: 384,
    flag: "wx",
  });
  expect(mocks.rename.mock.calls[0]?.[1]).toBe("/mock/file");
  expect(mocks.rm).toHaveBeenCalledTimes(1);
});
it("cleans temporary files on failed replacement", async () => {
  mocks.rename.mockRejectedValue(new Error("failed"));
  await expect(atomicWrite("/mock/file", "TEST")).rejects.toThrow("failed");
  expect(mocks.rm).toHaveBeenCalledTimes(1);
});
