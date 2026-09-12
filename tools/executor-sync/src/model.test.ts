// Runs with Bun; pure validation tests do not access files or services.
/** biome-ignore-all lint/suspicious/noTemplateCurlyInString: Tests assert literal portable path tokens. */
import { describe, expect, it } from "vitest";
import { envelope, PATHS, snapshot } from "./fixtures";
import {
  check,
  configSchema,
  fingerprint,
  MAX_BYTES,
  parseEnvelope,
  parseSnapshot,
  portableConfig,
  stable,
  tenantFor,
} from "./model";

describe("portable settings contract", () => {
  it("accepts a validated envelope", () => {
    expect(parseEnvelope(JSON.stringify(envelope())).group).toBe("personal");
  });
  it("rejects invalid JSON", () => {
    expect(() => parseEnvelope("{")).toThrow();
  });
  it("rejects another sync group", () => {
    expect(() =>
      parseEnvelope(JSON.stringify({ ...envelope(), group: "other" })),
    ).toThrow("Invalid encrypted envelope");
  });
  it("rejects an unsupported version", () => {
    expect(() =>
      parseSnapshot({ ...snapshot(), executorVersion: "2" }),
    ).toThrow("Invalid snapshot");
  });
  it("rejects additional tables", () => {
    expect(() =>
      parseSnapshot({
        ...snapshot(),
        tables: { ...snapshot().tables, oauth_session: [] },
      }),
    ).toThrow("Unexpected snapshot tables");
  });
  it("rejects missing schema", () => {
    expect(() => parseSnapshot({ ...snapshot(), schema: {} })).toThrow(
      "Unexpected snapshot schema",
    );
  });
  it("rejects oversized ciphertext plaintext", () => {
    expect(() => parseEnvelope(" ".repeat(MAX_BYTES + 1))).toThrow(
      "Snapshot exceeds size limit",
    );
  });
  it("sorts recursive object keys without reordering arrays", () => {
    expect(stable({ z: [null, 1, "x"], a: { y: true, a: false } })).toBe(
      '{"a":{"a":false,"y":true},"z":[null,1,"x"]}',
    );
  });
  it("rejects undefined", () => {
    expect(() => stable(undefined)).toThrow("Unsupported snapshot value");
  });
  it("reports only the chosen safe error", () => {
    expect(() => check(false, "safe error")).toThrow("safe error");
  });
  it("hash changes when a token changes", () => {
    expect(
      fingerprint(snapshot()) ===
        fingerprint({
          ...snapshot(),
          secrets: { changed: "OTHER-TEST-TOKEN" },
        }),
    ).toBe(false);
  });
  it("hash ignores property insertion order", () => {
    expect(
      fingerprint(snapshot()) ===
        fingerprint({
          ...snapshot(),
          tables: {
            tool_policy: [],
            oauth_client: [],
            connection: [],
            integration: [{ slug: "demo" }],
          },
        }),
    ).toBe(true);
  });
  it("normalizes commands, home and checkout", () => {
    expect(
      portableConfig(
        '{"command":"/opt/homebrew/bin/bun","args":["/Users/alice/work/dotfiles/tool.ts","/Users/alice/.agents"],"cwd":"/Users/alice/work/dotfiles"}',
        PATHS,
        "export",
      ),
    ).toBe(
      '{"args":["${DOTFILES}/tool.ts","${HOME}/.agents"],"command":"${BIN:bun}","cwd":"${DOTFILES}"}',
    );
  });
  it("normalizes a bare home and repo", () => {
    expect(
      portableConfig(
        '["/Users/alice","/Users/alice/work/dotfiles"]',
        PATHS,
        "export",
      ),
    ).toBe('["${HOME}","${DOTFILES}"]');
  });
  it("expands on a different machine", () => {
    expect(
      portableConfig(
        '{"command":"${BIN:bun}","cwd":"${DOTFILES}","args":["${HOME}/.agents"]}',
        {
          home: "/Users/bob",
          repo: "/Users/bob/other/dotfiles",
          commands: { bun: "/usr/local/bin/bun" },
        },
        "import",
      ),
    ).toBe(
      '{"args":["/Users/bob/.agents"],"command":"/usr/local/bin/bun","cwd":"/Users/bob/other/dotfiles"}',
    );
  });
  it("preserves remote URLs and typed values", () => {
    expect(
      portableConfig(
        '{"endpoint":"https://example.com/mcp","enabled":true,"port":1,"value":null}',
        PATHS,
        "export",
      ),
    ).toBe(
      '{"enabled":true,"endpoint":"https://example.com/mcp","port":1,"value":null}',
    );
  });
  it("preserves imported literals", () => {
    expect(
      portableConfig('["https://example.com",null,false]', PATHS, "import"),
    ).toBe('["https://example.com",null,false]');
  });
  it("rejects unknown absolute paths", () => {
    expect(() => portableConfig('"/private/tool"', PATHS, "export")).toThrow(
      "Unmapped absolute path",
    );
  });
  it("rejects embedded absolute arguments", () => {
    expect(() =>
      portableConfig('"--path=/Users/other/key"', PATHS, "export"),
    ).toThrow("Unmapped absolute path");
  });
  it("rejects missing executables", () => {
    expect(() => portableConfig('"${BIN:missing}"', PATHS, "import")).toThrow(
      "Missing local executable mapping",
    );
  });
  it("rejects malformed executable references", () => {
    expect(() => portableConfig('"${BIN:bunX"', PATHS, "import")).toThrow();
  });
  it("rejects unexpanded remote paths", () => {
    expect(() => portableConfig('"/Users/other/key"', PATHS, "import")).toThrow(
      "Unmapped portable path",
    );
  });
  it("rejects unexpanded placeholders", () => {
    expect(() => portableConfig('"${UNKNOWN}"', PATHS, "import")).toThrow(
      "Unmapped portable path",
    );
  });
  it("matches the existing Executor tenant", () => {
    expect(tenantFor("/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles")).toBe(
      "dotfiles-00f6b3f1",
    );
  });
  it("allows unconfigured disabled state", () => {
    expect(
      configSchema.safeParse({
        format: 1,
        enabled: false,
        accountId: "",
        bucket: "executor-config-sync",
        group: "personal",
        intervalSeconds: 30,
        ageRecipient: "",
      }).success,
    ).toBe(true);
  });
  it("rejects enabling incomplete configuration", () => {
    expect(
      configSchema.safeParse({
        format: 1,
        enabled: true,
        accountId: "",
        bucket: "executor-config-sync",
        group: "personal",
        intervalSeconds: 30,
        ageRecipient: "",
      }).success,
    ).toBe(false);
  });
  it("rejects credentials in public configuration", () => {
    expect(
      configSchema.safeParse({
        format: 1,
        enabled: false,
        accountId: "",
        bucket: "executor-config-sync",
        group: "personal",
        intervalSeconds: 30,
        ageRecipient: "",
        secretAccessKey: "TEST",
      }).success,
    ).toBe(false);
  });
});
