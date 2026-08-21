import { expect, it, vi } from "vitest";
import { OmlxLifecycleRuntime, type OmlxCommandRunner, type OmlxRunResult } from "./runtime.ts";

const CONFIG = {
  ensureCommand: "/home/user/.local/bin/ensure-omlx",
  ensureTimeoutMs: 200_000,
  idleStopCommand: "/home/user/.local/bin/omlx-idle-stop",
  idleStopTimeoutMs: 10_000,
};

function runnerReturning(result: OmlxRunResult): OmlxCommandRunner {
  return { run: vi.fn().mockResolvedValue(result) };
}

it("runs ensure-omlx when switching into omlx and reports success", async () => {
  const runner: OmlxCommandRunner = runnerReturning({ code: 0, stderr: "", stdout: "" });
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onModelSelect({
    nextProvider: "omlx",
    previousProvider: "anthropic",
  });

  expect(outcome).toStrictEqual({ action: "start", ok: true });
  expect(runner.run).toHaveBeenCalledWith(CONFIG.ensureCommand, [], CONFIG.ensureTimeoutMs);
});

it("runs omlx-idle-stop when switching away from omlx and reports success", async () => {
  const runner: OmlxCommandRunner = runnerReturning({ code: 0, stderr: "", stdout: "" });
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onModelSelect({
    nextProvider: "anthropic",
    previousProvider: "omlx",
  });

  expect(outcome).toStrictEqual({ action: "stop", ok: true });
  expect(runner.run).toHaveBeenCalledWith(CONFIG.idleStopCommand, [], CONFIG.idleStopTimeoutMs);
});

it("skips the runner entirely when switching between two non-omlx providers", async () => {
  const runner: OmlxCommandRunner = runnerReturning({ code: 0, stderr: "", stdout: "" });
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onModelSelect({
    nextProvider: "anthropic",
    previousProvider: "cursor",
  });

  expect(outcome).toStrictEqual({ action: "none", ok: true });
  expect(runner.run).not.toHaveBeenCalled();
});

it("reports a non-zero exit code from ensure-omlx as a non-ok outcome with stderr as the reason", async () => {
  const runner: OmlxCommandRunner = runnerReturning({
    code: 1,
    stderr: "ensure-omlx: missing /Applications/oMLX.app\n",
    stdout: "",
  });
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onModelSelect({
    nextProvider: "omlx",
    previousProvider: undefined,
  });

  expect(outcome).toStrictEqual({
    action: "start",
    ok: false,
    reason: "ensure-omlx: missing /Applications/oMLX.app",
  });
});

it("falls back to the exit code when a failing command has no stderr", async () => {
  const runner: OmlxCommandRunner = runnerReturning({ code: 127, stderr: "", stdout: "" });
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onModelSelect({
    nextProvider: "omlx",
    previousProvider: undefined,
  });

  expect(outcome).toStrictEqual({ action: "start", ok: false, reason: "exit code 127" });
});

it("treats a thrown Error (e.g. ENOENT because omlx was never installed) as a non-ok outcome", async () => {
  const runner: OmlxCommandRunner = { run: vi.fn().mockRejectedValue(new Error("ENOENT")) };
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onModelSelect({
    nextProvider: "omlx",
    previousProvider: undefined,
  });

  expect(outcome).toStrictEqual({ action: "start", ok: false, reason: "ENOENT" });
});

it("stringifies a non-Error rejection", async () => {
  const runner: OmlxCommandRunner = { run: vi.fn().mockRejectedValue("boom") };
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onModelSelect({
    nextProvider: "omlx",
    previousProvider: undefined,
  });

  expect(outcome).toStrictEqual({ action: "start", ok: false, reason: "boom" });
});

it("nudges omlx-idle-stop on session shutdown when the current provider is omlx", async () => {
  const runner: OmlxCommandRunner = runnerReturning({ code: 0, stderr: "", stdout: "" });
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onSessionShutdown("omlx");

  expect(outcome).toStrictEqual({ action: "stop", ok: true });
  expect(runner.run).toHaveBeenCalledWith(CONFIG.idleStopCommand, [], CONFIG.idleStopTimeoutMs);
});

it("does nothing on session shutdown when the current provider is not omlx", async () => {
  const runner: OmlxCommandRunner = runnerReturning({ code: 0, stderr: "", stdout: "" });
  const runtime = new OmlxLifecycleRuntime(runner, CONFIG);

  const outcome = await runtime.onSessionShutdown("anthropic");

  expect(outcome).toStrictEqual({ action: "none", ok: true });
  expect(runner.run).not.toHaveBeenCalled();
});
