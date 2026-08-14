import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it, vi } from "vitest";
import {
  defaultAuthPaths,
  readAuthRecords,
  walkAuthPaths,
  walkClineProviderSettings,
} from "../src/auth.ts";
import { stringValue } from "../src/utils.ts";

describe("auth source traversal", () => {
  it("builds Cline and pi auth paths", () => {
    expect(defaultAuthPaths("/home/user")).toStrictEqual([
      "/home/user/.cline/data/settings/providers.json",
      "/home/user/.pi/agent/auth.json",
    ]);
  });

  it("returns the first extracted value", () => {
    const readFile = vi
      .fn<(filePath: string) => string>()
      .mockReturnValueOnce("not-json")
      .mockReturnValueOnce(JSON.stringify({ token: "second" }));
    const result = walkAuthPaths(
      {
        authPaths: ["/first", "/second"],
        fileExists: () => true,
        readFile,
      },
      (parsed) => stringValue(parsed["token"]),
    );
    expect(result).toBe("second");
  });

  it("skips missing auth files", () => {
    const readFile = vi.fn<(filePath: string) => string>();
    expect(
      walkAuthPaths({ authPaths: ["/missing"], fileExists: () => false, readFile }, () => "unused"),
    ).toBeUndefined();
    expect(readFile).not.toHaveBeenCalled();
  });

  it("uses default filesystem readers", () => {
    const home = mkdtempSync(path.join(tmpdir(), "clinepass-auth-"));
    const clineDirectory = path.join(home, ".cline", "data", "settings");
    mkdirSync(clineDirectory, { recursive: true });
    writeFileSync(path.join(clineDirectory, "providers.json"), JSON.stringify({ source: "cline" }));
    try {
      expect(readAuthRecords({ homeDir: () => home })).toStrictEqual([{ source: "cline" }]);
    } finally {
      rmSync(home, { force: true, recursive: true });
    }
  });

  it("ignores non-record auth JSON", () => {
    expect(
      readAuthRecords({
        authPaths: ["/array"],
        fileExists: () => true,
        readFile: () => "[]",
      }),
    ).toStrictEqual([]);
  });

  it("walks both Cline provider settings", () => {
    const parsed = {
      providers: {
        "cline-pass": { settings: {} },
        cline: { settings: { selected: "value" } },
      },
    };
    expect(walkClineProviderSettings(parsed, (settings) => stringValue(settings["selected"]))).toBe(
      "value",
    );
    expect(walkClineProviderSettings({}, () => "unused")).toBeUndefined();
    expect(
      walkClineProviderSettings({ providers: { cline: "invalid" } }, () => "unused"),
    ).toBeUndefined();
  });
});
