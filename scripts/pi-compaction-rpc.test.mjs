import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { EventEmitter, once } from "node:events";
import { existsSync } from "node:fs";
import { mkdtemp, writeFile, rm, mkdir } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import test from "node:test";

const scripts = dirname(fileURLToPath(import.meta.url));
const repo = join(scripts, "..");
const COMPACT_CUSTOMIZATION_FLAGS = [
  "--extension",
  join(scripts, "fixtures/pi-compaction-provider.mjs"),
  "--extension",
  join(repo, "tools/pi-effort-manager"),
  "--extension",
  join(repo, "tools/pi-my-cursor-provider"),
  "--extension",
  join(repo, "tools/pi-loop-extension"),
  "--extension",
  join(repo, "tools/pi-tmux-timeout-extension"),
  "--extension",
  join(repo, "tools/pi-agmsg-extension"),
];

test("compact customization packages exist on the shipped path", () => {
  assert.equal(existsSync(join(repo, "tools/pi-effort-manager/index.ts")), true);
  assert.equal(existsSync(join(repo, "tools/pi-my-cursor-provider/src/compaction.ts")), true);
  assert.equal(existsSync(join(repo, "tools/pi-loop-extension/index.ts")), true);
  assert.equal(existsSync(join(repo, "tools/pi-tmux-timeout-extension/index.ts")), true);
  assert.equal(existsSync(join(repo, "tools/pi-agmsg-extension/index.ts")), true);
});

test("real fullscreen terminal handles compact and queued input", { timeout: 45000 }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "pi-compaction-terminal-"));
  const changes = new EventEmitter();
  const summaryGate = Promise.withResolvers();
  const state = { output: "", requests: [] };
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    state.requests.push(JSON.parse(Buffer.concat(chunks).toString()));
    const number = state.requests.length;
    changes.emit("change");
    if (number === 3) await summaryGate.promise;
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write(
      `data: ${JSON.stringify({ id: "terminal-test", object: "chat.completion.chunk", model: "test-model", choices: [{ index: 0, delta: { role: "assistant", content: `TerminalReply${number}` }, finish_reason: null }] })}\n\n`,
    );
    response.write(
      `data: ${JSON.stringify({ id: "terminal-test", object: "chat.completion.chunk", model: "test-model", choices: [{ index: 0, delta: {}, finish_reason: "stop" }] })}\n\n`,
    );
    response.end("data: [DONE]\n\n");
  });
  await mkdir(join(directory, "agent"));
  await writeFile(
    join(directory, "agent/settings.json"),
    JSON.stringify({ compaction: { enabled: false, keepRecentTokens: 1, reserveTokens: 16384 } }),
  );
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const child = spawn(
    "/usr/bin/expect",
    [
      join(scripts, "fixtures/pi-terminal.exp"),
      join(scripts, "pi"),
      "--offline",
      "--no-session",
      "--no-extensions",
      "--no-skills",
      "--no-prompt-templates",
      "--no-context-files",
      "--no-tools",
      ...COMPACT_CUSTOMIZATION_FLAGS,
      "--provider",
      "compaction-test",
      "--model",
      "test-model",
      "--tui-mode",
      "fullscreen",
    ],
    {
      cwd: directory,
      detached: true,
      env: {
        ...process.env,
        TERM: "xterm-256color",
        PI_CODING_AGENT_DIR: join(directory, "agent"),
        PI_TEST_API_URL: `http://127.0.0.1:${server.address().port}/v1`,
      },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  child.stdout.on("data", (chunk) => {
    state.output += chunk;
    changes.emit("change");
  });
  child.stderr.on("data", (chunk) => {
    state.output += chunk;
    changes.emit("change");
  });
  t.after(async () => {
    summaryGate.resolve();
    if (child.exitCode === null) {
      const exited = once(child, "exit");
      process.kill(-child.pid, "SIGTERM");
      await exited;
    }
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await rm(directory, { recursive: true, force: true });
  });
  function wait(predicate) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        changes.off("change", check);
        reject(
          new Error(
            `Terminal timeout (${state.requests.length} requests): ${state.output.slice(-1500)}`,
          ),
        );
      }, 10000);
      function check() {
        if (predicate()) {
          clearTimeout(timeout);
          changes.off("change", check);
          resolve();
        }
      }
      changes.on("change", check);
      check();
    });
  }
  await wait(() => state.output.includes("test-model"));
  await new Promise((resolve) => setTimeout(resolve, 1000));
  child.stdin.write("Seed history for compaction. ".repeat(30) + "\r");
  await wait(() => state.output.includes("TerminalReply1"));
  child.stdin.write("Keep this recent second user turn.\r");
  await wait(() => state.output.includes("TerminalReply2"));
  child.stdin.write("/compact\r");
  await wait(() => state.output.includes("Compacting context"));
  await wait(() => state.requests.length === 3);
  child.stdin.write("QUEUED_INPUT_DURING_COMPACTION\r");
  await wait(() => state.output.includes("QUEUED_INPUT_DURING_COMPACTION"));
  summaryGate.resolve();
  await wait(() => state.output.includes("TerminalReply4"));
  assert.match(JSON.stringify(state.requests[3].messages), /QUEUED_INPUT_DURING_COMPACTION/);
  child.stdin.write("INPUT_AFTER_COMPACTION\r");
  await wait(() => state.output.includes("TerminalReply5"));
  assert.match(JSON.stringify(state.requests[4].messages), /INPUT_AFTER_COMPACTION/);
});

test(
  "hanging session_before_compact cannot keep fullscreen input locked",
  { timeout: 30000 },
  async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "pi-compaction-hang-"));
    const changes = new EventEmitter();
    const state = { output: "", requests: [] };
    const server = createServer(async (request, response) => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      state.requests.push(JSON.parse(Buffer.concat(chunks).toString()));
      changes.emit("change");
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(
        `data: ${JSON.stringify({ id: "terminal-test", object: "chat.completion.chunk", model: "test-model", choices: [{ index: 0, delta: { role: "assistant", content: `HangReply${state.requests.length}` }, finish_reason: null }] })}\n\n`,
      );
      response.write(
        `data: ${JSON.stringify({ id: "terminal-test", object: "chat.completion.chunk", model: "test-model", choices: [{ index: 0, delta: {}, finish_reason: "stop" }] })}\n\n`,
      );
      response.end("data: [DONE]\n\n");
    });
    await mkdir(join(directory, "agent"));
    await writeFile(
      join(directory, "agent/settings.json"),
      JSON.stringify({ compaction: { enabled: false, keepRecentTokens: 1, reserveTokens: 16384 } }),
    );
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const child = spawn(
      "/usr/bin/expect",
      [
        join(scripts, "fixtures/pi-terminal.exp"),
        join(scripts, "pi"),
        "--offline",
        "--no-session",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-context-files",
        "--no-tools",
        "--extension",
        join(scripts, "fixtures/pi-compaction-provider.mjs"),
        "--extension",
        join(scripts, "fixtures/pi-hanging-before-compact.mjs"),
        "--provider",
        "compaction-test",
        "--model",
        "test-model",
        "--tui-mode",
        "fullscreen",
      ],
      {
        cwd: directory,
        detached: true,
        env: {
          ...process.env,
          TERM: "xterm-256color",
          PI_CODING_AGENT_DIR: join(directory, "agent"),
          PI_TEST_API_URL: `http://127.0.0.1:${server.address().port}/v1`,
        },
        stdio: ["pipe", "pipe", "pipe"],
      },
    );
    child.stdout.on("data", (chunk) => {
      state.output += chunk;
      changes.emit("change");
    });
    child.stderr.on("data", (chunk) => {
      state.output += chunk;
      changes.emit("change");
    });
    t.after(async () => {
      if (child.exitCode === null) {
        const exited = once(child, "exit");
        process.kill(-child.pid, "SIGTERM");
        await exited;
      }
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
      await rm(directory, { recursive: true, force: true });
    });
    function wait(predicate) {
      return new Promise((resolve, reject) => {
        const timeout = setTimeout(() => {
          changes.off("change", check);
          reject(
            new Error(
              `Terminal timeout (${state.requests.length} requests): ${state.output.slice(-1500)}`,
            ),
          );
        }, 10000);
        function check() {
          if (predicate()) {
            clearTimeout(timeout);
            changes.off("change", check);
            resolve();
          }
        }
        changes.on("change", check);
        check();
      });
    }
    await wait(() => state.output.includes("test-model"));
    await new Promise((resolve) => setTimeout(resolve, 1000));
    child.stdin.write("Seed history for compaction. ".repeat(30) + "\r");
    await wait(() => state.output.includes("HangReply1"));
    child.stdin.write("Keep this recent second user turn.\r");
    await wait(() => state.output.includes("HangReply2"));
    child.stdin.write("/compact\r");
    await wait(() => state.output.includes("Compacting context"));
    child.stdin.write("\x1b");
    await wait(() => state.output.includes("Compaction cancelled"));
    child.stdin.write("INPUT_AFTER_CANCELLED_COMPACTION\r");
    await wait(() => state.output.includes("HangReply3"));
    assert.match(
      JSON.stringify(state.requests[2].messages),
      /INPUT_AFTER_CANCELLED_COMPACTION/,
    );
  },
);

function client(directory, endpoint, sessionFile) {
  const child = spawn(
    join(scripts, "pi"),
    [
      "--offline",
      "--mode",
      "rpc",
      "--no-extensions",
      "--no-skills",
      "--no-prompt-templates",
      "--no-context-files",
      "--no-tools",
      "--extension",
      join(scripts, "fixtures/pi-compaction-provider.mjs"),
      "--provider",
      "compaction-test",
      "--model",
      "test-model",
      "--session-dir",
      join(directory, "sessions"),
      ...(sessionFile ? ["--session", sessionFile] : []),
    ],
    {
      cwd: directory,
      env: {
        ...process.env,
        PI_CODING_AGENT_DIR: join(directory, "agent"),
        PI_TEST_API_URL: endpoint,
      },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  const state = { events: [], stderr: "", sequence: 0, waiters: new Set() };
  child.stderr.on("data", (chunk) => {
    state.stderr += chunk;
  });
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    try {
      state.events.push(JSON.parse(line));
    } catch {
      return;
    }
    state.waiters.forEach((notify) => notify());
  });
  function wait(predicate, start) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        cleanup();
        reject(
          new Error(`RPC timeout: ${state.stderr}\n${JSON.stringify(state.events.slice(-5))}`),
        );
      }, 15000);
      function cleanup() {
        clearTimeout(timer);
        state.waiters.delete(notify);
      }
      function notify() {
        const found = state.events.slice(start).find(predicate);
        if (found) {
          cleanup();
          resolve(found);
        }
      }
      state.waiters.add(notify);
      notify();
    });
  }
  return {
    state,
    async request(command) {
      const start = state.events.length;
      const id = String(++state.sequence);
      child.stdin.write(JSON.stringify({ ...command, id }) + "\n");
      const response = await wait((event) => event.type === "response" && event.id === id, start);
      assert.equal(response.success, true, JSON.stringify(response));
      return response.data;
    },
    async prompt(message) {
      const start = state.events.length;
      await this.request({ type: "prompt", message });
      await wait((event) => event.type === "agent_end", start);
    },
    async stop() {
      if (child.exitCode !== null) return;
      const exited = once(child, "exit");
      child.kill("SIGTERM");
      await exited;
      lines.close();
    },
  };
}

test(
  "real RPC process compacts, accepts another message, resumes disk session, and compacts again",
  { timeout: 45000 },
  async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "pi-compaction-rpc-"));
    const server = createServer(async (request, response) => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const payload = JSON.parse(Buffer.concat(chunks).toString());
      assert.ok(Array.isArray(payload.messages));
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(
        `data: ${JSON.stringify({ id: "local", object: "chat.completion.chunk", model: "test-model", choices: [{ index: 0, delta: { role: "assistant", content: "Verified local response and concise checkpoint." }, finish_reason: null }] })}\n\n`,
      );
      response.write(
        `data: ${JSON.stringify({ id: "local", object: "chat.completion.chunk", model: "test-model", choices: [{ index: 0, delta: {}, finish_reason: "stop" }], usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 } })}\n\n`,
      );
      response.end("data: [DONE]\n\n");
    });
    const clients = [];
    t.after(async () => {
      await Promise.all(clients.map((connection) => connection.stop()));
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
      await rm(directory, { recursive: true, force: true });
    });
    await mkdir(join(directory, "agent"));
    await writeFile(
      join(directory, "agent/settings.json"),
      JSON.stringify({
        compaction: { enabled: false, keepRecentTokens: 20, reserveTokens: 16384 },
      }),
    );
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const endpoint = `http://127.0.0.1:${server.address().port}/v1`;
    const first = client(directory, endpoint);
    clients.push(first);
    await first.prompt("Please retain this context. ".repeat(200));
    await first.prompt("Keep this recent turn while compacting older history.");
    await first.request({ type: "compact" });
    await first.prompt("Post-compaction message works.");
    const state = await first.request({ type: "get_state" });
    assert.equal(state.isCompacting, false);
    assert.ok(state.sessionFile);
    await first.stop();
    const resumed = client(directory, endpoint, state.sessionFile);
    clients.push(resumed);
    await resumed.prompt("Resumed session still accepts input. ".repeat(100));
    await resumed.request({ type: "compact" });
    await resumed.prompt("Final input after resumed compaction.");
    const resumedState = await resumed.request({ type: "get_state" });
    assert.equal(resumedState.isCompacting, false);
    assert.equal(resumedState.sessionId, state.sessionId);
    assert.equal(
      resumed.state.events.some((event) => event.type === "extension_error"),
      false,
    );
  },
);
