// This TypeScript file is executed with Bun.
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import process from "node:process";
import { setTimeout } from "node:timers/promises";
import { getAgentDir, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { bindPlamo } from "./src/bind.ts";
import { createRuntime, type Runtime } from "./src/runtime.ts";
import { createServer, type ServerDeps } from "./src/server.ts";
import { readPlamoSettings } from "./src/settings.ts";

function liveDeps(report: (message: string) => void): ServerDeps {
  return {
    exists: existsSync,
    fetch: async (url, init) => globalThis.fetch(url, init),
    home: homedir(),
    now: () => Date.now(),
    platform: process.platform,
    report,
    sleep: async (ms) => {
      await setTimeout(ms);
    },
    spawn: (command, args) => spawn(command, [...args], { stdio: "ignore", detached: true }),
  };
}

export default function plamoTranslate(pi: ExtensionAPI): void {
  const settings = readPlamoSettings(() =>
    readFileSync(path.join(getAgentDir(), "settings.json"), "utf8"),
  );
  const holder: { runtime: Runtime | null } = { runtime: null };
  const runtime = createRuntime(
    settings,
    createServer(
      liveDeps((message) => {
        holder.runtime?.ui.notify(message, "warning");
      }),
      settings.port,
    ),
  );
  holder.runtime = runtime;
  bindPlamo(
    {
      onSessionStart: (handler) => {
        pi.on("session_start", (_event, ctx) => {
          handler({ ui: ctx.ui });
        });
      },
      onInput: (handler) => {
        pi.on("input", (event, ctx) => handler(event, ctx));
      },
      onBeforeAgent: (handler) => {
        pi.on("before_agent_start", (event) => {
          handler(event.systemPromptOptions.sections);
        });
      },
      onMessageEnd: (handler) => {
        pi.on("message_end", (event) => {
          handler(event.message);
        });
      },
      onMarkdown: (transformer, messageTypes) => {
        pi.registerMarkdownTransformer(transformer, { messageTypes });
      },
    },
    runtime,
  );
}
