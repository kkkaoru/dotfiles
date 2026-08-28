// This TypeScript file is executed with Bun.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import {
  isRecord,
  readManagerSettings,
  readReserveTokens,
  resolveSettingsTarget,
  writeManagerBoolean,
} from "./settings.ts";

const directories: string[] = [];

function temporaryDirectory(): string {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "pi-effort-manager-test-"));
  directories.push(directory);
  return directory;
}

afterEach(() => {
  directories.splice(0).map((directory): void => fs.rmSync(directory, { recursive: true }));
});

it("recognizes plain record settings values", () => {
  expect(isRecord({ setting: true })).toBe(true);
  expect(isRecord([])).toBe(false);
  expect(isRecord(null)).toBe(false);
});

it("reads defaults and merged compaction reserve settings", () => {
  const directory = temporaryDirectory();
  const globalPath = path.join(directory, "global.json");
  const projectPath = path.join(directory, "project.json");
  fs.writeFileSync(
    globalPath,
    JSON.stringify({
      compaction: { reserveTokens: 100 },
      "pi-effort-manager": {
        compactionEffort: "max",
        compactionResetEffort: "low",
        compactionResetInterval: 5,
        dynamicDefault: true,
        endEffort: "xhigh",
        fastMode: true,
        progressTextOnCompaction: true,
        progressTextOnEffortChange: false,
        rampStartRatio: 0.7,
        startEffort: "low",
      },
    }),
  );
  fs.writeFileSync(
    projectPath,
    JSON.stringify({
      compaction: { reserveTokens: 25 },
      "pi-effort-manager": {
        compactionResetEffort: "xhigh",
        dynamicDefault: false,
        progressTextOnCompaction: false,
        startEffort: "medium",
      },
    }),
  );

  expect(readManagerSettings(globalPath, projectPath)).toStrictEqual({
    compactionEffort: "max",
    compactionResetEffort: "xhigh",
    compactionResetInterval: 5,
    dynamicDefault: false,
    endEffort: "xhigh",
    fastMode: true,
    progressTextOnCompaction: false,
    progressTextOnEffortChange: false,
    rampStartRatio: 0.7,
    startEffort: "medium",
  });
  expect(readReserveTokens(globalPath, projectPath)).toBe(25);
  expect(readReserveTokens(globalPath)).toBe(100);
  expect(readManagerSettings(path.join(directory, "missing.json"))).toStrictEqual({
    compactionEffort: undefined,
    compactionResetEffort: "xhigh",
    compactionResetInterval: 1,
    dynamicDefault: false,
    endEffort: undefined,
    fastMode: false,
    progressTextOnCompaction: false,
    progressTextOnEffortChange: false,
    rampStartRatio: 0.6,
    startEffort: "medium",
  });
});

it("writes manager booleans atomically through a settings symlink", () => {
  const directory = temporaryDirectory();
  const target = path.join(directory, "settings-target.json");
  const link = path.join(directory, "settings.json");
  fs.writeFileSync(target, `${JSON.stringify({ theme: "dark" })}\n`);
  fs.symlinkSync(target, link);

  writeManagerBoolean(link, "fastMode", true);
  writeManagerBoolean(link, "dynamicDefault", true);

  expect(JSON.parse(fs.readFileSync(target, "utf8"))).toStrictEqual({
    theme: "dark",
    "pi-effort-manager": { dynamicDefault: true, fastMode: true },
  });
  expect(fs.lstatSync(link).isSymbolicLink()).toBe(true);

  const missing = path.join(directory, "nested", "settings.json");
  writeManagerBoolean(missing, "fastMode", true);
  expect(JSON.parse(fs.readFileSync(missing, "utf8"))).toStrictEqual({
    "pi-effort-manager": { fastMode: true },
  });
});

it("rejects malformed settings and falls back for invalid manager values", () => {
  const directory = temporaryDirectory();
  const malformed = path.join(directory, "malformed.json");
  const invalid = path.join(directory, "invalid.json");
  const array = path.join(directory, "array.json");
  fs.writeFileSync(malformed, "{");
  fs.writeFileSync(array, "[]");
  fs.writeFileSync(
    invalid,
    JSON.stringify({ compaction: { reserveTokens: -1 }, "pi-effort-manager": "invalid" }),
  );

  expect(readManagerSettings(malformed)).toStrictEqual({
    compactionResetEffort: "xhigh",
    compactionResetInterval: 1,
    dynamicDefault: false,
    fastMode: false,
    progressTextOnCompaction: false,
    progressTextOnEffortChange: false,
    rampStartRatio: 0.6,
    startEffort: "medium",
  });
  expect(readManagerSettings(array)).toStrictEqual({
    compactionEffort: undefined,
    compactionResetEffort: "xhigh",
    compactionResetInterval: 1,
    dynamicDefault: false,
    endEffort: undefined,
    fastMode: false,
    progressTextOnCompaction: false,
    progressTextOnEffortChange: false,
    rampStartRatio: 0.6,
    startEffort: "medium",
  });
  expect(readManagerSettings(invalid)).toStrictEqual({
    compactionEffort: undefined,
    compactionResetEffort: "xhigh",
    compactionResetInterval: 1,
    dynamicDefault: false,
    endEffort: undefined,
    fastMode: false,
    progressTextOnCompaction: false,
    progressTextOnEffortChange: false,
    rampStartRatio: 0.6,
    startEffort: "medium",
  });
  expect(readReserveTokens(invalid)).toBe(16_384);

  const blockedParent = path.join(directory, "blocked");
  fs.writeFileSync(blockedParent, "file");
  expect((): void =>
    writeManagerBoolean(path.join(blockedParent, "settings.json"), "fastMode", true),
  ).toThrow();

  expect((): void => {
    resolveSettingsTarget(malformed, (): never => {
      throw new Error("inspection failed");
    });
  }).toThrow("inspection failed");

  const remove = vi.fn<typeof fs.rmSync>();
  expect((): void =>
    writeManagerBoolean(
      invalid,
      "fastMode",
      true,
      (): never => {
        throw new Error("write failed");
      },
      remove,
    ),
  ).toThrow("write failed");
  expect(remove).toHaveBeenCalledOnce();
});
