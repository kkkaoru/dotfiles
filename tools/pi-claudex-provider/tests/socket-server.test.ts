import { spawn } from "node:child_process";
import { once } from "node:events";
import { chmod, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { connect, type Socket } from "node:net";
import { createInterface, type Interface } from "node:readline";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { afterEach, describe, expect, it } from "vitest";
import { isRecord } from "../src/protocol.ts";
import { startGatewayServer } from "../src/socket-server.ts";

const TOKEN = "01234567890123456789012345678901";
const ROOT = `/tmp/pi-claudex-provider-${process.pid}`;

interface Client {
  socket: Socket;
  lines: AsyncIterableIterator<string>;
  reader: Interface;
}

function registry() {
  return {
    getAvailable: () => [
      {
        provider: "ollama-cloud",
        id: "glm-5.2",
        name: "GLM",
        api: "openai-completions",
        reasoning: true,
        input: ["text"],
        contextWindow: 100,
        maxTokens: 10,
      },
      {
        provider: "claudex",
        id: "loop",
        name: "Loop",
        api: "anthropic-messages",
        reasoning: false,
        input: ["text"],
        contextWindow: 100,
        maxTokens: 10,
      },
    ],
  } as unknown as ExtensionContext["modelRegistry"];
}

async function openClient(socketPath: string): Promise<Client> {
  const socket = connect(socketPath);
  await new Promise<void>((resolve, reject) => {
    socket.once("connect", resolve);
    socket.once("error", reject);
  });
  const reader = createInterface({ input: socket, crlfDelay: Number.POSITIVE_INFINITY });
  return { socket, lines: reader[Symbol.asyncIterator](), reader };
}

async function readJson(client: Client): Promise<Record<string, unknown>> {
  const line = await client.lines.next();
  if (line.done === true) {
    throw new Error("socket ended before a line arrived");
  }
  const value: unknown = JSON.parse(line.value);
  if (!isRecord(value)) {
    throw new Error("server line was not an object");
  }
  return value;
}

function send(client: Client, value: unknown): void {
  client.socket.write(`${JSON.stringify(value)}\n`);
}

async function closeClient(client: Client): Promise<void> {
  client.reader.close();
  client.socket.destroy();
}

async function createSocketPath(name: string): Promise<string> {
  await mkdir(ROOT, { recursive: true, mode: 0o700 });
  await chmod(ROOT, 0o700);
  return `${ROOT}/${name}.sock`;
}

async function leaveStaleSocket(path: string): Promise<void> {
  const script = [
    'const net = require("node:net")',
    `net.createServer().listen(${JSON.stringify(path)}, () => process.stdout.write("ready\\n"))`,
    "setInterval(() => {}, 1000)",
  ].join(";");
  const child = spawn(process.execPath, ["-e", script], { stdio: ["ignore", "pipe", "inherit"] });
  await once(child.stdout, "data");
  child.kill("SIGKILL");
  await once(child, "exit");
}

afterEach(async () => {
  await rm(ROOT, { recursive: true, force: true });
});

describe("authenticated Unix socket gateway", () => {
  it("handshakes, lists filtered models, and cleans up its mode-0600 socket", async () => {
    const path = await createSocketPath("happy");
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    expect((await stat(path)).mode.toString(8).slice(-3)).toBe("600");
    const client = await openClient(path);
    client.socket.write(`${JSON.stringify({ version: 1, type: "hello", token: TOKEN })}\r\n\n`);
    expect(await readJson(client)).toStrictEqual({ version: 1, type: "ready" });
    send(client, { version: 1, type: "list_models", id: "models", token: TOKEN });
    expect(await readJson(client)).toStrictEqual({
      version: 1,
      type: "models",
      id: "models",
      models: [
        {
          provider: "ollama-cloud",
          id: "glm-5.2",
          name: "GLM",
          api: "openai-completions",
          reasoning: true,
          input: ["text"],
          contextWindow: 100,
          maxTokens: 10,
        },
      ],
    });
    await closeClient(client);
    await server.close();
    await expect(stat(path)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("fails closed on invalid authentication and non-hello first messages", async () => {
    const path = await createSocketPath("auth");
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    const badToken = await openClient(path);
    send(badToken, { version: 1, type: "hello", token: "short" });
    expect(await readJson(badToken)).toStrictEqual({
      version: 1,
      type: "protocol_error",
      message: "Pi gateway authentication failed",
    });
    await closeClient(badToken);

    const missingHello = await openClient(path);
    send(missingHello, { version: 1, type: "list_models", id: "m", token: TOKEN });
    expect(await readJson(missingHello)).toStrictEqual({
      version: 1,
      type: "protocol_error",
      message: "Pi gateway expected hello as the first message",
    });
    await closeClient(missingHello);
    await server.close();
  });

  it("reports malformed input and supports a second concurrent authenticated client", async () => {
    const path = await createSocketPath("protocol");
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    const client = await openClient(path);
    client.socket.write("not-json\n");
    expect(await readJson(client)).toMatchObject({ version: 1, type: "protocol_error" });
    send(client, { version: 1, type: "hello", token: TOKEN });
    expect(await readJson(client)).toStrictEqual({ version: 1, type: "ready" });
    send(client, {
      version: 1,
      type: "request",
      id: "bad-origin",
      token: TOKEN,
      origin: "pi",
      provider: "ollama-cloud",
      modelId: "glm-5.2",
      messages: [],
      tools: [],
      options: {},
    });
    expect(await readJson(client)).toStrictEqual({
      version: 1,
      type: "protocol_error",
      id: "bad-origin",
      message: "Gateway request origin must be claudex",
    });
    send(client, { version: 1, type: "hello", token: TOKEN });
    expect(await readJson(client)).toStrictEqual({
      version: 1,
      type: "protocol_error",
      message: "Pi gateway hello may only be sent once",
    });

    const second = await openClient(path);
    send(second, { version: 1, type: "hello", token: TOKEN });
    expect(await readJson(second)).toStrictEqual({ version: 1, type: "ready" });
    await closeClient(second);
    await closeClient(client);
    await server.close();
  });

  it("handles a client disconnect while writing a protocol error", async () => {
    const path = await createSocketPath("write-disconnect");
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    const client = await openClient(path);
    client.socket.end("not-json\n");
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 10);
    });
    await closeClient(client);
    await server.close();
    await expect(stat(path)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("rejects oversized unterminated input and closes the connection", async () => {
    const path = await createSocketPath("oversized");
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    const client = await openClient(path);
    client.socket.write("x".repeat(8 * 1024 * 1024 + 1));
    expect(await readJson(client)).toStrictEqual({
      version: 1,
      type: "protocol_error",
      message: "Pi gateway input line is too large",
    });
    await closeClient(client);
    await server.close();
  }, 20_000);

  it("replaces a stale Unix socket left by a crashed process", async () => {
    const path = await createSocketPath("stale");
    await leaveStaleSocket(path);
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    const client = await openClient(path);
    send(client, { version: 1, type: "hello", token: TOKEN });
    expect(await readJson(client)).toStrictEqual({ version: 1, type: "ready" });
    await closeClient(client);
    await server.close();
  });

  it("absorbs queued write failures while the server shuts down", async () => {
    const path = await createSocketPath("queued-close");
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    const client = await openClient(path);
    client.socket.write("not-json\n".repeat(100));
    await server.close();
    await closeClient(client);
    await expect(stat(path)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("refuses to unlink a regular file at the configured path", async () => {
    const path = await createSocketPath("regular");
    await writeFile(path, "keep");
    await expect(
      startGatewayServer({ socketPath: path, token: TOKEN }, registry()),
    ).rejects.toThrow("is not a socket");
    expect(await readFile(path, "utf8")).toBe("keep");
  });

  it("closes idempotently when no client connected", async () => {
    const path = await createSocketPath("close");
    const server = await startGatewayServer({ socketPath: path, token: TOKEN }, registry());
    await server.close();
    await server.close();
    await expect(stat(path)).rejects.toMatchObject({ code: "ENOENT" });
  });
});
