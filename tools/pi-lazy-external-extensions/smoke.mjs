// Runs with Bun. Offline lifecycle fixtures; never starts a model or real provider session.
import assert from "node:assert/strict";
import {mkdtemp, mkdir, writeFile, rm} from "node:fs/promises";
import {tmpdir, homedir} from "node:os";
import {join} from "node:path";
import {setTimeout as delay} from "node:timers/promises";

const directory = await mkdtemp(join(tmpdir(), "pi-deferred-provider-smoke-"));
const previous = process.env.PI_CODING_AGENT_DIR;
const originalFetch = globalThis.fetch;
process.env.PI_CODING_AGENT_DIR = directory;
globalThis.fetch = () => {throw new Error("Offline smoke: network disabled");};
try {
  const fixture = join(directory, "npm/node_modules/fixture-provider");
  await mkdir(fixture, {recursive: true});
  await writeFile(join(fixture, "index.ts"), 'export default function (pi) { pi.registerProvider("fixture", {}); }');
  const {deferExternalExtension} = await import("./loader.ts");
  const events = new Map();
  const providers = [];
  const host = {on: (name, handler) => {events.set(name, handler);}, registerProvider: (name) => {providers.push(name);}};
  deferExternalExtension(host, "fixture-provider");
  await delay(300);
  assert.deepEqual(providers, [], "Discovery must not run an unowned loader timer");
  assert.ok(events.has("session_start"));
  await events.get("session_start")();
  await events.get("session_start")();
  assert.deepEqual(providers, ["fixture"], "Provider registers once across session switches");

  const hostRoot = join(process.env.BUN_INSTALL || join(homedir(), ".bun"), "install/global/node_modules");
  const {DefaultResourceLoader, SettingsManager} = await import(join(hostRoot, "@earendil-works/pi-coding-agent/dist/index.js"));
  const loader = new DefaultResourceLoader({cwd: directory, agentDir: directory,
    settingsManager: SettingsManager.inMemory({packages: [], extensions: []}),
    additionalExtensionPaths: [join(homedir(), ".pi/agent/npm/node_modules/pi-web-access/index.ts")],
    noSkills: true, noThemes: true, noPromptTemplates: true, noContextFiles: true});
  await loader.reload();
  const result = loader.getExtensions();
  assert.deepEqual(result.errors, []);
  assert.equal(result.extensions.length, 1);
  assert.ok(result.extensions[0].handlers.get("session_start")?.length > 0,
    "Native loading must register initial session hooks before dispatch");
  process.stdout.write("PASS: discovery is inert, session loading is awaited/idempotent, Web Access native lifecycle hooks are ready; no network/model calls.\n");
} finally {
  globalThis.fetch = originalFetch;
  if (previous === undefined) delete process.env.PI_CODING_AGENT_DIR;
  else process.env.PI_CODING_AGENT_DIR = previous;
  await rm(directory, {recursive: true, force: true});
}
