import { createAgentPlatform, type CursorAgentPlatform } from "@cursor/sdk";
import { beforeEach, describe, expect, test, vi, type Mock } from "vitest";
import {
  cursorPlatformTestApi,
  ensureCursorPlatform,
  warmCursorWorkspace,
} from "../src/platform.ts";
import { getProcessCursorStore } from "../src/store.ts";

vi.mock("@cursor/sdk", async (importOriginal) => {
  const original = await importOriginal<typeof import("@cursor/sdk")>();
  return {
    ...original,
    createAgentPlatform: vi.fn(),
  };
});

const createAgentPlatformMock = vi.mocked(createAgentPlatform);

interface FakePlatform {
  readonly prewarm: Mock;
  prewarmLocalWorkspace: (...args: unknown[]) => Promise<() => Promise<void>>;
}

function fakePlatform(release: () => Promise<void> = () => Promise.resolve()): FakePlatform {
  const prewarm = vi.fn().mockResolvedValue(release);
  return {
    prewarm,
    prewarmLocalWorkspace: (...args: unknown[]) => prewarm(...args),
  };
}

beforeEach(() => {
  cursorPlatformTestApi.setEnabled(true);
  cursorPlatformTestApi.reset();
  createAgentPlatformMock.mockReset();
});

describe("ensureCursorPlatform", () => {
  test("creates a platform, prewarms it, and caches the result", async () => {
    const platform = fakePlatform();
    createAgentPlatformMock.mockResolvedValue(platform as unknown as CursorAgentPlatform);

    const first = await ensureCursorPlatform("key", "/some/cwd");
    expect(first).toBe(platform as unknown as CursorAgentPlatform);
    expect(createAgentPlatformMock).toHaveBeenCalledTimes(1);
    expect(createAgentPlatformMock).toHaveBeenCalledWith({
      localStore: getProcessCursorStore(),
    });
    expect(platform.prewarm).toHaveBeenCalledWith(
      expect.objectContaining({
        model: { id: "auto" },
        tools: ["mcp", "webSearch", "semSearch", "shell"],
        apiKey: "key",
        local: expect.objectContaining({
          cwd: "/some/cwd",
          settingSources: [],
          store: getProcessCursorStore(),
        }),
      }),
    );

    const second = await ensureCursorPlatform("other-key", "/other/cwd");
    expect(second).toBe(platform as unknown as CursorAgentPlatform);
    expect(createAgentPlatformMock).toHaveBeenCalledTimes(1);
  });

  test("creates a platform without an apiKey", async () => {
    const platform = fakePlatform();
    createAgentPlatformMock.mockResolvedValue(platform as unknown as CursorAgentPlatform);

    const first = await ensureCursorPlatform(undefined, "/some/cwd");
    expect(first).toBe(platform as unknown as CursorAgentPlatform);
    expect(platform.prewarm).toHaveBeenCalledWith(
      expect.objectContaining({
        model: { id: "auto" },
        tools: ["mcp", "webSearch", "semSearch", "shell"],
        local: expect.objectContaining({
          cwd: "/some/cwd",
          settingSources: [],
          store: getProcessCursorStore(),
        }),
      }),
    );
    const call = platform.prewarm.mock.calls[0]?.[0];
    expect(call).not.toHaveProperty("apiKey");
  });

  test("returns undefined and warns when prewarmLocalWorkspace fails", async () => {
    const warn = vi.spyOn(console, "warn").mockReturnValue(undefined);
    const platform = fakePlatform();
    platform.prewarm.mockRejectedValue(new Error("no prewarm"));
    createAgentPlatformMock.mockResolvedValue(platform as unknown as CursorAgentPlatform);

    const result = await ensureCursorPlatform("key", "/cwd");

    expect(result).toBeUndefined();
    expect(warn).toHaveBeenCalledWith("Failed to prewarm Cursor workspace:", expect.any(Error));
    warn.mockRestore();
  });

  test("returns undefined and warns when createAgentPlatform fails", async () => {
    const warn = vi.spyOn(console, "warn").mockReturnValue(undefined);
    createAgentPlatformMock.mockRejectedValue(new Error("no platform"));

    const platform = await ensureCursorPlatform("key", "/cwd");

    expect(platform).toBeUndefined();
    expect(warn).toHaveBeenCalledWith("Failed to prewarm Cursor workspace:", expect.any(Error));
    warn.mockRestore();
  });

  test("returns undefined and does not call createAgentPlatform when disabled", async () => {
    cursorPlatformTestApi.setEnabled(false);
    cursorPlatformTestApi.reset();

    const platform = await ensureCursorPlatform("key", "/cwd");

    expect(platform).toBeUndefined();
    expect(createAgentPlatformMock).not.toHaveBeenCalled();
  });

  test("reset clears the cached platform and calls the release", async () => {
    const release = vi.fn().mockResolvedValue(undefined);
    const platform = fakePlatform(release);
    createAgentPlatformMock.mockResolvedValue(platform as unknown as CursorAgentPlatform);

    await ensureCursorPlatform("key", "/cwd");
    expect(release).not.toHaveBeenCalled();

    cursorPlatformTestApi.reset();
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(release).toHaveBeenCalled();
  });
});

describe("warmCursorWorkspace", () => {
  test("starts prewarming and ignores the result", async () => {
    createAgentPlatformMock.mockResolvedValue(fakePlatform() as unknown as CursorAgentPlatform);

    await warmCursorWorkspace("key", "/cwd");

    expect(createAgentPlatformMock).toHaveBeenCalledTimes(1);
  });
});
