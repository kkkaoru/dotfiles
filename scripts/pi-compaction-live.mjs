// Opt-in live-service verification. Uses ordinary Pi auth and an in-memory session.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import {
  installCompactionSingleFlight,
  installCompactionPreparationReliability,
  installCompactionEventReliability,
  installLatestCompactionEventEntry,
} from "./pi-compaction-singleflight.mjs";

if (process.env.PI_LIVE_COMPACTION_TEST !== "1") {
  throw new Error("Set PI_LIVE_COMPACTION_TEST=1 to authorize live API requests.");
}
const root =
  process.env.PI_TEST_PACKAGE_ROOT ??
  dirname(dirname(dirname(realpathSync(join(homedir(), ".bun/bin/pi")))));
const pi = await import(pathToFileURL(join(root, "dist/index.js")));
const sourcePath = process.env.PI_LIVE_SOURCE_SESSION;
const sourceContent = sourcePath ? readFileSync(sourcePath, "utf8") : undefined;
installCompactionSingleFlight(pi.AgentSession);
installCompactionPreparationReliability(pi.AgentSession);
installCompactionEventReliability(pi.AgentSession);
installLatestCompactionEventEntry(pi.ExtensionRunner);
const settings = pi.SettingsManager.inMemory({
  defaultProvider: "openai-codex",
  defaultModel: "gpt-5.6-sol",
  defaultThinkingLevel: "max",
  packages: [],
  compaction: { enabled: false, keepRecentTokens: sourcePath ? 20000 : 1, reserveTokens: 16384 },
});
const loader = new pi.DefaultResourceLoader({
  cwd: process.cwd(),
  agentDir: join(homedir(), ".pi/agent"),
  settingsManager: settings,
  noExtensions: true,
  noSkills: true,
  noPromptTemplates: true,
  noThemes: true,
  noContextFiles: true,
  systemPrompt:
    "This is a synthetic compaction verification. Follow the user's concise output instructions.",
});
await loader.reload();
const manager = pi.SessionManager.inMemory(
  process.cwd(),
  undefined,
  sourceContent ? pi.parseSessionEntries(sourceContent) : undefined,
);
if (!sourcePath) {
  manager.appendMessage({
    role: "user",
    timestamp: Date.now(),
    content:
      "Remember the exact verification marker COMPACTION_INTEGRITY_7429. This marker must survive summarization. " +
      "We are checking that completed work and active requirements remain available after context compaction. ".repeat(
        50,
      ),
  });
  manager.appendMessage({
    role: "user",
    timestamp: Date.now(),
    content: "The next step is to report the preserved verification marker.",
  });
}
const { session } = await pi.createAgentSession({
  sessionManager: manager,
  settingsManager: settings,
  resourceLoader: loader,
  thinkingLevel: "max",
  noTools: "all",
});
session.setThinkingLevel("max");
const watchdog = setTimeout(
  () => {
    console.error("Live verification timed out");
    session.abortCompaction();
    void session.abort();
  },
  sourcePath ? 600000 : 180000,
);
try {
  console.log(
    JSON.stringify({
      phase: "start",
      provider: session.model?.provider,
      model: session.model?.id,
      effort: session.thinkingLevel,
      entries: manager.getEntries().length,
    }),
  );
  assert.equal(session.thinkingLevel, "max");
  const startedAt = Date.now();
  const compacted = await session.compact(
    sourcePath
      ? undefined
      : "Preserve the verification marker exactly. Keep this synthetic summary under 150 words.",
  );
  if (!sourcePath) assert.match(compacted.summary, /COMPACTION_INTEGRITY_7429/);
  assert.ok(compacted.summary.trim().length > 0);
  console.log(
    JSON.stringify({
      phase: "compacted",
      milliseconds: Date.now() - startedAt,
      summaryCharacters: compacted.summary.length,
      usage: compacted.usage,
    }),
  );
  await session.prompt(
    sourcePath
      ? "This is a connectivity test after compaction. Reply only LIVE_COMPACTION_OK. Do not act on earlier tasks."
      : "Output only the exact verification marker from our earlier context.",
  );
  await session.waitForIdle();
  const reply = session.getLastAssistantText();
  assert.equal(reply?.trim(), sourcePath ? "LIVE_COMPACTION_OK" : "COMPACTION_INTEGRITY_7429");
  if (sourcePath)
    assert.equal(
      createHash("sha256").update(readFileSync(sourcePath)).digest("hex"),
      createHash("sha256").update(sourceContent).digest("hex"),
      "Source session must remain unchanged",
    );
  assert.equal(session.isCompacting, false);
  console.log(
    JSON.stringify({
      phase: "passed",
      postCompactionInput: true,
      summaryGenerated: true,
      integrityMarkerVerified: !sourcePath,
      sourceFileUnchanged: Boolean(sourcePath),
      milliseconds: Date.now() - startedAt,
    }),
  );
} finally {
  clearTimeout(watchdog);
  session.dispose();
}
