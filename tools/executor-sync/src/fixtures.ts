// Runs with Bun. Synthetic test data only; never copy local Executor credentials here.
import type { Envelope, Paths, Snapshot } from "./model";

export const PATHS: Paths = {
  home: "/Users/alice",
  repo: "/Users/alice/work/dotfiles",
  commands: { bun: "/opt/homebrew/bin/bun" },
};
export function snapshot(): Snapshot {
  return {
    format: 1,
    executorVersion: "1.6.8",
    schema: {
      integration: ["slug"],
      connection: [],
      oauth_client: [],
      tool_policy: [],
    },
    tables: {
      integration: [{ slug: "demo" }],
      connection: [],
      oauth_client: [],
      tool_policy: [],
    },
    secrets: { "file:fake-token": "SYNTHETIC-TEST-TOKEN" },
  };
}
export function envelope(): Envelope {
  return {
    group: "personal",
    device: "11111111-1111-4111-8111-111111111111",
    revision: "22222222-2222-4222-8222-222222222222",
    createdAt: "2026-01-01T00:00:00.000Z",
    snapshot: snapshot(),
  };
}
