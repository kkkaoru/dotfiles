// This TypeScript file is executed with Bun.
import { Buffer } from "node:buffer";
import { Agent, request } from "node:http";
import { URL } from "node:url";
import type { FetchLike, RequestInitLike, ResponseLike } from "./server.ts";

/** Keep-alive agents, one per socket: a reused connection is what makes a socket call cheap. */
const agents = new Map<string, Agent>();

function agentFor(socketPath: string, maxSockets: number): Agent {
  const existing = agents.get(socketPath);
  if (existing !== undefined) {
    return existing;
  }
  const agent = new Agent({ keepAlive: true, maxSockets });
  agents.set(socketPath, agent);
  return agent;
}

async function overSocket(
  url: string,
  socketPath: string,
  init: RequestInitLike | undefined,
  maxSockets: number,
): Promise<ResponseLike> {
  return new Promise<ResponseLike>((resolve, reject) => {
    const target = new URL(url);
    const outgoing = request(
      {
        agent: agentFor(socketPath, maxSockets),
        headers: init?.headers,
        method: init?.method ?? "GET",
        path: `${target.pathname}${target.search}`,
        socketPath,
      },
      (incoming) => {
        const chunks: Buffer[] = [];
        incoming.on("data", (chunk: Buffer) => {
          chunks.push(chunk);
        });
        incoming.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          // A received response always carries a status; Number() keeps the type branch-free.
          const status = Number(incoming.statusCode);
          resolve({
            ok: status >= 200 && status < 300,
            status,
            json: async () => JSON.parse(text) as unknown,
          });
        });
        incoming.on("error", reject);
      },
    );
    outgoing.on("error", reject);
    outgoing.end(init?.body);
  });
}

/**
 * The one fetch the mod uses: a request that names a socket goes over it, everything else over TCP.
 * Both speak the same HTTP, so the router and the fallback servers need no second protocol.
 */
export function plamoFetch(maxSockets: number): FetchLike {
  return async (url, init) => {
    const socketPath = init?.socketPath;
    if (socketPath === undefined) {
      const plain: RequestInit = { method: init?.method ?? "GET" };
      if (init?.body !== undefined) {
        plain.body = init.body;
      }
      if (init?.headers !== undefined) {
        plain.headers = init.headers;
      }
      return globalThis.fetch(url, plain);
    }
    return overSocket(url, socketPath, init, maxSockets);
  };
}
