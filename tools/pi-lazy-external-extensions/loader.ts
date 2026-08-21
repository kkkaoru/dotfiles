// This file runs with Bun.

import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

interface JitiModule {
  import(path: string, options: { default: true }): Promise<unknown>;
}

interface JitiFactory {
  createJiti(
    url: string,
    options: { alias: Record<string, string>; moduleCache: boolean },
  ): JitiModule;
}

interface SettingsFile {
  defaultProvider?: unknown;
}

const DEFAULT_DELAY_MS = 250;
const AGENT_DIR = process.env.PI_CODING_AGENT_DIR?.trim() || join(homedir(), ".pi", "agent");
const BUN_INSTALL = process.env.BUN_INSTALL?.trim() || join(homedir(), ".bun");
const HOST_NODE_MODULES = join(BUN_INSTALL, "install", "global", "node_modules");
const DEFAULT_SETTINGS_PATH = join(AGENT_DIR, "settings.json");

let jiti: JitiModule | undefined;

function readDefaultProvider(path: string): string | undefined {
  try {
    const settings = JSON.parse(readFileSync(path, "utf8")) as SettingsFile;
    return typeof settings.defaultProvider === "string" ? settings.defaultProvider : undefined;
  } catch {
    return undefined;
  }
}

function configuredProvider(): string | undefined {
  const projectProvider = readDefaultProvider(join(process.cwd(), ".pi", "settings.json"));
  return projectProvider ?? readDefaultProvider(DEFAULT_SETTINGS_PATH);
}

function selectedValue(flag: string): string | undefined {
  const args = process.argv.slice(2);
  for (let index = 0; index < args.length; index += 1) {
    const value = args[index];
    if (value === flag) return args[index + 1];
    if (value?.startsWith(`${flag}=`)) return value.slice(flag.length + 1);
  }
  return undefined;
}

function providerWasSelected(provider: string): boolean {
  const explicitProvider = selectedValue("--provider") ?? process.env.PI_PROVIDER?.trim();
  if ((explicitProvider || configuredProvider()) === provider) return true;

  const model = selectedValue("--model") ?? process.env.PI_MODEL?.trim();
  return model?.split("/", 1)[0] === provider;
}

function getJiti(): JitiModule {
  if (jiti) return jiti;

  const codingAgentEntry = join(
    HOST_NODE_MODULES,
    "@earendil-works",
    "pi-coding-agent",
    "dist",
    "index.js",
  );
  const aiCompatEntry = join(HOST_NODE_MODULES, "@earendil-works", "pi-ai", "dist", "compat.js");
  const tuiEntry = join(HOST_NODE_MODULES, "@earendil-works", "pi-tui", "dist", "index.js");
  if (!existsSync(codingAgentEntry) || !existsSync(aiCompatEntry) || !existsSync(tuiEntry)) {
    throw new Error(`Pi host packages are missing under ${HOST_NODE_MODULES}`);
  }

  const hostRequire = createRequire(codingAgentEntry);
  const { createJiti } = hostRequire("jiti") as JitiFactory;
  jiti = createJiti(import.meta.url, {
    alias: {
      "@earendil-works/pi-coding-agent": codingAgentEntry,
      "@earendil-works/pi-ai": aiCompatEntry,
      "@earendil-works/pi-ai/compat": aiCompatEntry,
      "@earendil-works/pi-tui": tuiEntry,
    },
    moduleCache: false,
  });
  return jiti;
}

function externalEntry(packageName: string, entryPoint: string): string {
  return join(AGENT_DIR, "npm", "node_modules", packageName, entryPoint);
}

async function loadExternal(pi: ExtensionAPI, packageName: string, entryPoint: string): Promise<void> {
  const entry = externalEntry(packageName, entryPoint);
  if (!existsSync(entry)) {
    throw new Error(`Extension package is not installed: ${packageName}`);
  }
  const loaded = await getJiti().import(entry, { default: true });
  const factory = typeof loaded === "function" ? loaded : (loaded as { default?: unknown }).default;
  if (typeof factory !== "function") {
    throw new Error(`Extension package has no default factory: ${packageName}`);
  }
  await factory(pi);
}

export function deferExternalExtension(
  pi: ExtensionAPI,
  packageName: string,
  entryPoint = "index.ts",
  eagerProvider?: string,
): void | Promise<void> {
  let loading: Promise<void> | undefined;
  const load = (): Promise<void> => {
    loading ??= loadExternal(pi, packageName, entryPoint).catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      console.error(`[lazy-extension] ${packageName}: ${message}`);
    });
    return loading;
  };

  if (eagerProvider !== undefined && providerWasSelected(eagerProvider)) return load();

  const delay = Number(process.env.PI_LAZY_EXTENSION_DELAY_MS ?? DEFAULT_DELAY_MS);
  const timer = setTimeout(() => {
    void load();
  }, Number.isFinite(delay) && delay >= 0 ? delay : DEFAULT_DELAY_MS);
  timer.unref?.();
}
