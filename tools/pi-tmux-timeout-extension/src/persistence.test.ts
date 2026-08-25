// This TypeScript file is executed with Bun.
import fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, expect, it } from "vitest";
import {
  LAUNCH_METADATA_FILENAME,
  deliveryMarkerPath,
  markCompletionDelivered,
  nextTmuxLaunchId,
  persistTmuxLaunch,
  recoverSessionTmuxLaunches,
  recoverTmuxLaunches,
  serializeLaunchMetadata,
} from "./persistence.ts";
import type { TmuxLaunch } from "./tmux.ts";

const temporaryDirectories: string[] = [];

function temporaryDirectory(): string {
  const directory: string = fs.mkdtempSync(path.join(tmpdir(), "pi-tmux-persistence-test-"));
  temporaryDirectories.push(directory);
  return directory;
}

function launch(rootDirectory: string, id: number): TmuxLaunch {
  const sessionName = `pi-tmux-4321-${String(id)}`;
  const directory: string = path.join(rootDirectory, sessionName);
  fs.mkdirSync(directory);
  return {
    command: "tmux wrapper",
    completionChannel: `${sessionName}-complete`,
    logPath: path.join(directory, "output.log"),
    sessionName,
    statusPath: path.join(directory, "exit-status"),
    submittedAt: new Date(2026, 7, 26, 2, 5).toISOString(),
    taskCommand: `job ${String(id)}`,
  };
}

afterEach(() => {
  temporaryDirectories
    .splice(0)
    .map((directory: string): void => fs.rmSync(directory, { force: true, recursive: true }));
});

function createInvalidMetadataFixtures(rootDirectory: string): void {
  const missingFields: TmuxLaunch = launch(rootDirectory, 12);
  fs.writeFileSync(path.join(path.dirname(missingFields.logPath), LAUNCH_METADATA_FILENAME), "{}");
  const mismatched: TmuxLaunch = launch(rootDirectory, 13);
  fs.writeFileSync(
    path.join(path.dirname(mismatched.logPath), LAUNCH_METADATA_FILENAME),
    serializeLaunchMetadata({ ...mismatched, completionChannel: "wrong" }),
  );
}

it("recovers metadata and legacy jobs for the current Pi pid only", () => {
  const rootDirectory: string = temporaryDirectory();
  const persisted: TmuxLaunch = launch(rootDirectory, 7);
  fs.writeFileSync(
    path.join(path.dirname(persisted.logPath), LAUNCH_METADATA_FILENAME),
    serializeLaunchMetadata(persisted),
  );
  fs.writeFileSync(persisted.logPath, "output");

  const delivered: TmuxLaunch = launch(rootDirectory, 8);
  fs.writeFileSync(
    path.join(path.dirname(delivered.logPath), LAUNCH_METADATA_FILENAME),
    serializeLaunchMetadata(delivered),
  );
  fs.writeFileSync(deliveryMarkerPath(delivered), "already delivered\n");

  const invalid: TmuxLaunch = launch(rootDirectory, 9);
  fs.writeFileSync(path.join(path.dirname(invalid.logPath), LAUNCH_METADATA_FILENAME), "{");

  const legacy: TmuxLaunch = launch(rootDirectory, 10);
  fs.writeFileSync(legacy.logPath, "legacy output");
  launch(rootDirectory, 11);
  createInvalidMetadataFixtures(rootDirectory);
  fs.mkdirSync(path.join(rootDirectory, "pi-tmux-9999-1"));

  const recovered = recoverTmuxLaunches({ pid: 4321, rootDirectory });
  expect(recovered).toHaveLength(2);
  expect(nextTmuxLaunchId({ pid: 4321, rootDirectory })).toBe(14);
  expect(recovered).toEqual(
    expect.arrayContaining([
      { ...persisted, command: "" },
      expect.objectContaining({
        command: "",
        sessionName: "pi-tmux-4321-10",
        taskCommand: "recovered tmux job pi-tmux-4321-10",
      }),
    ]),
  );
});

it("ignores an artifact concurrently removed during recovery", () => {
  expect(
    recoverTmuxLaunches({
      operations: {
        exists: (filePath: string): boolean => filePath.endsWith(LAUNCH_METADATA_FILENAME),
        readDirectory: (): readonly string[] => ["pi-tmux-4321-1"],
        readFile: (): string => {
          throw new Error("removed");
        },
        statBirthtime: (): number => 0,
        writeFile: (): void => undefined,
      },
      pid: 4321,
      rootDirectory: "/tmp",
    }),
  ).toEqual([]);
});

it("persists and recovers undelivered launches from a resumed Pi session", () => {
  const rootDirectory: string = temporaryDirectory();
  const persisted: TmuxLaunch = launch(rootDirectory, 21);
  const entries: unknown[] = [];
  persistTmuxLaunch((customType, data): void => {
    entries.push({ customType, data, type: "custom" });
  }, persisted);
  entries.push(null, { type: "message" }, entries[0], {
    customType: "pi-tmux-launch-v1",
    type: "custom",
  });

  expect(recoverSessionTmuxLaunches(entries)).toEqual([{ ...persisted, command: "" }]);
  markCompletionDelivered(persisted);
  expect(recoverSessionTmuxLaunches(entries)).toEqual([]);
  persistTmuxLaunch(undefined, persisted);
});

it("marks successful delivery and tolerates an unavailable recovery root", () => {
  const rootDirectory: string = temporaryDirectory();
  const persisted: TmuxLaunch = launch(rootDirectory, 1);
  markCompletionDelivered(persisted);
  expect(fs.readFileSync(deliveryMarkerPath(persisted), "utf8")).toMatch(/^\d{4}-\d{2}-\d{2}T/u);
  const missingRoot: string = path.join(rootDirectory, "missing");
  expect(recoverTmuxLaunches({ pid: 4321, rootDirectory: missingRoot })).toEqual([]);
  expect(nextTmuxLaunchId({ pid: 4321, rootDirectory: missingRoot })).toBe(1);
});
