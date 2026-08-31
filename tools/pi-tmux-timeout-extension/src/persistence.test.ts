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

const SESSION_NAMESPACE = "a".repeat(32);
const temporaryDirectories: string[] = [];

function temporaryDirectory(): string {
  const directory: string = fs.mkdtempSync(path.join(tmpdir(), "pi-tmux-persistence-test-"));
  temporaryDirectories.push(directory);
  return directory;
}

function launch(rootDirectory: string, id: number): TmuxLaunch {
  const sessionName = `pi-tmux-${SESSION_NAMESPACE}-${String(id)}`;
  const directory: string = path.join(rootDirectory, sessionName);
  fs.mkdirSync(directory);
  return {
    command: "tmux wrapper",
    completionChannel: `${sessionName}-complete`,
    estimatedCompletionAt: new Date(2026, 7, 26, 2, 10).toISOString(),
    logPath: path.join(directory, "output.log"),
    sessionName,
    socketName: `pi-tmux-${SESSION_NAMESPACE}`,
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
  const invalidEstimate: TmuxLaunch = launch(rootDirectory, 14);
  fs.writeFileSync(
    path.join(path.dirname(invalidEstimate.logPath), LAUNCH_METADATA_FILENAME),
    JSON.stringify({ ...invalidEstimate, estimatedCompletionAt: 42 }),
  );
}

it("recovers metadata and legacy jobs for the current Pi session namespace only", () => {
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
  fs.mkdirSync(path.join(rootDirectory, `pi-tmux-${"b".repeat(32)}-1`));

  const recovered = recoverTmuxLaunches({ rootDirectory, sessionNamespace: SESSION_NAMESPACE });
  expect(recovered).toHaveLength(2);
  expect(nextTmuxLaunchId({ rootDirectory, sessionNamespace: SESSION_NAMESPACE })).toBe(15);
  expect(recovered).toEqual(
    expect.arrayContaining([
      { ...persisted, command: "" },
      expect.objectContaining({
        command: "",
        sessionName: `pi-tmux-${SESSION_NAMESPACE}-10`,
        taskCommand: `recovered tmux job pi-tmux-${SESSION_NAMESPACE}-10`,
      }),
    ]),
  );
});

it("ignores an artifact concurrently removed during recovery", () => {
  expect(
    recoverTmuxLaunches({
      operations: {
        exists: (filePath: string): boolean => filePath.endsWith(LAUNCH_METADATA_FILENAME),
        readDirectory: (): readonly string[] => [`pi-tmux-${SESSION_NAMESPACE}-1`],
        readFile: (): string => {
          throw new Error("removed");
        },
        statBirthtime: (): number => 0,
        writeFile: (): void => undefined,
      },
      rootDirectory: "/tmp",
      sessionNamespace: SESSION_NAMESPACE,
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
  const removedArtifact: TmuxLaunch = launch(rootDirectory, 22);
  persistTmuxLaunch((customType, data): void => {
    entries.push({ customType, data, type: "custom" });
  }, removedArtifact);
  fs.rmSync(path.dirname(removedArtifact.statusPath), { recursive: true });
  entries.push(null, { type: "message" }, entries[0], {
    customType: "pi-tmux-launch-v2",
    type: "custom",
  });

  expect(recoverSessionTmuxLaunches(entries, SESSION_NAMESPACE)).toEqual([
    { ...persisted, command: "" },
  ]);
  expect(recoverSessionTmuxLaunches(entries, "b".repeat(32))).toEqual([]);
  markCompletionDelivered(persisted);
  expect(recoverSessionTmuxLaunches(entries, SESSION_NAMESPACE)).toEqual([]);
  persistTmuxLaunch(undefined, persisted);
});

it("marks successful delivery and tolerates an unavailable recovery root", () => {
  const rootDirectory: string = temporaryDirectory();
  const persisted: TmuxLaunch = launch(rootDirectory, 1);
  markCompletionDelivered(persisted);
  expect(fs.readFileSync(deliveryMarkerPath(persisted), "utf8")).toMatch(/^\d{4}-\d{2}-\d{2}T/u);
  const removedArtifact: TmuxLaunch = launch(rootDirectory, 2);
  fs.rmSync(path.dirname(removedArtifact.statusPath), { recursive: true });
  expect((): void => markCompletionDelivered(removedArtifact)).not.toThrow();
  const missingRoot: string = path.join(rootDirectory, "missing");
  expect(
    recoverTmuxLaunches({ rootDirectory: missingRoot, sessionNamespace: SESSION_NAMESPACE }),
  ).toEqual([]);
  expect(
    nextTmuxLaunchId({ rootDirectory: missingRoot, sessionNamespace: SESSION_NAMESPACE }),
  ).toBe(1);
});
