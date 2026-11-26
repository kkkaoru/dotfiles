import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionAPI, ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import { afterEach, describe, expect, it, vi } from "vitest";

import piMyAntigravity from "../index.ts";
import { DEFAULT_ENDPOINT, DEFAULT_MODEL, PROVIDER_ID, PROVIDER_NAME } from "../src/constants.ts";

interface Registered {
  name?: string;
  baseUrl?: string;
  api?: string;
  apiKey?: string;
  streamSimple?: unknown;
  models?: ProviderModelConfig[];
  refreshModels?: () => Promise<ProviderModelConfig[]>;
}

interface Captured {
  id: string;
  config: Registered;
}

function capture(): Captured {
  let captured: Captured | undefined;
  const pi = {
    registerProvider: (id: string, config: Registered) => {
      captured = { id, config };
    },
  } as unknown as ExtensionAPI;
  piMyAntigravity(pi);
  if (!captured) {
    throw new Error("provider was not registered");
  }
  return captured;
}

function refresh(captured: Captured): Promise<ProviderModelConfig[]> {
  const refreshModels = captured.config.refreshModels;
  if (!refreshModels) {
    throw new Error("refreshModels is missing");
  }
  return refreshModels();
}

const directories: string[] = [];

function script(body: string): string {
  const directory = mkdtempSync(join(tmpdir(), "my-antigravity-"));
  directories.push(directory);
  const path = join(directory, "gemini-api");
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
  return path;
}

function ids(models: ProviderModelConfig[]): string[] {
  return models.map((model) => model.id);
}

afterEach(() => {
  vi.unstubAllEnvs();
  for (const directory of directories.splice(0)) {
    rmSync(directory, { force: true, recursive: true });
  }
});

describe("piMyGeminiApi", () => {
  it("registers the provider under the existing id", () => {
    const captured = capture();
    expect(captured.id).toBe(PROVIDER_ID);
    expect(captured.config.name).toBe(PROVIDER_NAME);
    expect(captured.config.api).toBe("my-gemini-api");
    expect(captured.config.baseUrl).toBe(DEFAULT_ENDPOINT);
    expect(captured.config.streamSimple).toBeTypeOf("function");
    expect(ids(captured.config.models ?? [])).toEqual([DEFAULT_MODEL]);
  });

  it("passes GEMINI_API_KEY through when it is set", () => {
    vi.stubEnv("GEMINI_API_KEY", "key");
    expect(capture().config.apiKey).toBe("key");
  });

  it("omits the key when the CLI owns it", () => {
    vi.stubEnv("GEMINI_API_KEY", undefined);
    expect(capture().config).not.toHaveProperty("apiKey");
  });

  it("falls back to the static catalog when the CLI cannot be run", async () => {
    vi.stubEnv("GEMINI_API_CLI", "/nonexistent/gemini-api");
    expect(ids(await refresh(capture()))).toEqual([DEFAULT_MODEL]);
  });

  it("merges the catalog the CLI reports", async () => {
    vi.stubEnv("GEMINI_API_CLI", script(`echo '{"models":[{"id":"gemini-extra"}]}'`));
    expect(ids(await refresh(capture()))).toEqual([DEFAULT_MODEL, "gemini-extra"]);
  });

  it("keeps the static catalog when the CLI reports nothing", async () => {
    vi.stubEnv("GEMINI_API_CLI", script("echo '{}'"));
    expect(ids(await refresh(capture()))).toEqual([DEFAULT_MODEL]);
  });
});

describe("catalog without a CLI", () => {
  it("returns the static catalog when no binary can be found", async () => {
    const empty = mkdtempSync(join(tmpdir(), "my-antigravity-home-"));
    directories.push(empty);
    vi.stubEnv("GEMINI_API_CLI", "");
    vi.stubEnv("PATH", "");
    vi.stubEnv("HOME", empty);
    expect(ids(await refresh(capture()))).toEqual([DEFAULT_MODEL]);
  });
});
