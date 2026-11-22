// This TypeScript file is executed with Bun.
import { Buffer } from "node:buffer";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, expect, test } from "vitest";
import { plamoFetch } from "./socket.ts";

const MAX_SOCKETS = 8;
const cleanup: (() => void)[] = [];

afterEach(() => {
  for (const stop of cleanup.splice(0)) {
    stop();
  }
});

interface Harness {
  readonly connections: () => number;
  readonly socketPath: string | undefined;
  readonly url: string;
}

function handler(incoming: IncomingMessage, outgoing: ServerResponse): void {
  if (incoming.url === "/v1/models") {
    outgoing.writeHead(200, { "content-type": "application/json" });
    outgoing.end(JSON.stringify({ data: [] }));
    return;
  }
  if (incoming.url === "/broken") {
    outgoing.writeHead(200, { "content-type": "application/json" });
    outgoing.end("not json");
    return;
  }
  if (incoming.url === "/missing") {
    outgoing.writeHead(404, { "content-type": "application/json" });
    outgoing.end(JSON.stringify({ error: "missing" }));
    return;
  }
  const chunks: Buffer[] = [];
  incoming.on("data", (chunk: Buffer) => {
    chunks.push(chunk);
  });
  incoming.on("end", () => {
    outgoing.writeHead(200, { "content-type": "application/json" });
    outgoing.end(JSON.stringify({ length: Buffer.concat(chunks).length }));
  });
}

function listen(useSocket: boolean): Promise<Harness> {
  let connections = 0;
  const server: Server = createServer(handler);
  server.on("connection", () => {
    connections += 1;
  });
  return new Promise<Harness>((resolve, reject) => {
    server.once("error", reject);
    if (!useSocket) {
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        const port = typeof address === "object" && address !== null ? address.port : 0;
        cleanup.push(() => {
          server.close();
        });
        resolve({
          connections: () => connections,
          socketPath: undefined,
          url: `http://127.0.0.1:${String(port)}`,
        });
      });
      return;
    }
    const directory = mkdtempSync(path.join(tmpdir(), "plamo-socket-"));
    const socketPath = path.join(directory, "plamo.sock");
    server.listen(socketPath, () => {
      cleanup.push(() => {
        server.close();
        rmSync(directory, { force: true, recursive: true });
      });
      resolve({ connections: () => connections, socketPath, url: "http://127.0.0.1:18555" });
    });
  });
}

test("a request naming a socket goes over it", async () => {
  const harness = await listen(true);
  const response = await plamoFetch(MAX_SOCKETS)(`${harness.url}/v1/models`, {
    socketPath: harness.socketPath,
  });
  expect(response.ok).toBe(true);
  expect(response.status).toBe(200);
  expect(await response.json()).toStrictEqual({ data: [] });
});

test("a socket request carries its body and headers", async () => {
  const harness = await listen(true);
  const body = JSON.stringify({ prompt: "hello" });
  const response = await plamoFetch(MAX_SOCKETS)(`${harness.url}/v1/completions`, {
    body,
    headers: { "content-type": "application/json" },
    method: "POST",
    socketPath: harness.socketPath,
  });
  expect(await response.json()).toStrictEqual({ length: body.length });
});

test("keep-alive reuses the socket connection", async () => {
  const harness = await listen(true);
  const fetch = plamoFetch(MAX_SOCKETS);
  await fetch(`${harness.url}/v1/models`, { socketPath: harness.socketPath });
  await fetch(`${harness.url}/v1/models`, { socketPath: harness.socketPath });
  await fetch(`${harness.url}/v1/models`, { socketPath: harness.socketPath });
  expect(harness.connections()).toBe(1);
});

test("a failing status is not ok", async () => {
  const harness = await listen(true);
  const response = await plamoFetch(MAX_SOCKETS)(`${harness.url}/missing`, {
    socketPath: harness.socketPath,
  });
  expect(response.ok).toBe(false);
  expect(response.status).toBe(404);
});

test("a body that is not JSON rejects json()", async () => {
  const harness = await listen(true);
  const response = await plamoFetch(MAX_SOCKETS)(`${harness.url}/broken`, {
    socketPath: harness.socketPath,
  });
  await expect(response.json()).rejects.toThrow(SyntaxError);
});

test("a missing socket rejects the request", async () => {
  const harness = await listen(true);
  await expect(
    plamoFetch(MAX_SOCKETS)(`${harness.url}/v1/models`, {
      socketPath: `${String(harness.socketPath)}-gone`,
    }),
  ).rejects.toMatchObject({ code: "ENOENT" });
});

test("a request without a socket uses TCP", async () => {
  const harness = await listen(false);
  const response = await plamoFetch(MAX_SOCKETS)(`${harness.url}/v1/models`);
  expect(await response.json()).toStrictEqual({ data: [] });
});

test("a TCP request sends its body and headers", async () => {
  const harness = await listen(false);
  const body = JSON.stringify({ prompt: "hello" });
  const response = await plamoFetch(MAX_SOCKETS)(`${harness.url}/v1/completions`, {
    body,
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(await response.json()).toStrictEqual({ length: body.length });
});
