// This TypeScript file is executed with Bun.
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import process from "node:process";
import { setTimeout } from "node:timers/promises";
import { createServer, type ServerDeps } from "../../pi-plamo-translate/src/server.ts";
import { readPlamoSettings } from "../../pi-plamo-translate/src/settings.ts";
import { OFF } from "../../pi-plamo-translate/src/policy.ts";
import { TranslatedTimeline } from "./translate.ts";

function liveDeps(): ServerDeps {
  return {
    exists: existsSync,
    fetch: async (url, init) => globalThis.fetch(url, init),
    home: homedir(),
    now: () => Date.now(),
    platform: process.platform,
    // The server's own failures are reported by plamo-translate; the tree keeps its English.
    report: () => {
      // Nothing to add here.
    },
    sleep: async (ms) => {
      await setTimeout(ms);
    },
    spawn: (command, args) => spawn(command, [...args], { stdio: "ignore", detached: true }),
  };
}

/** The reader's language and server come from pi-plamo-translate; "off" leaves the tree English. */
export function plamoDisplay(read: () => string): TranslatedTimeline | undefined {
  const settings = readPlamoSettings(read);
  if (settings.replyLanguage === OFF) {
    return undefined;
  }
  return new TranslatedTimeline(settings.replyLanguage, createServer(liveDeps(), settings.port));
}

/** The layout pi uses, as its getAgentDir(): the environment variable overrides ~/.pi/agent. */
function settingsPath(): string {
  const directory = process.env["PI_CODING_AGENT_DIR"] ?? path.join(homedir(), ".pi", "agent");
  return path.join(directory, "settings.json");
}

export function plamoDisplayFromSettings(): TranslatedTimeline | undefined {
  return plamoDisplay(() => readFileSync(settingsPath(), "utf8"));
}
