import { timingSafeEqual } from "node:crypto";
import { chmod, lstat, unlink } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { GatewayConfig } from "./config.ts";
import { errorMessage, GatewayError } from "./errors.ts";
import { GatewayConnection, type GatewayWriter } from "./gateway.ts";
import { parseClientMessage, serverMessage, type ServerMessage } from "./protocol.ts";

const MAX_LINE_CHARACTERS = 8 * 1024 * 1024;

type ModelRegistry = ExtensionContext["modelRegistry"];

export interface GatewayServer {
  close: () => Promise<void>;
}

function observe(promise: Promise<unknown>): void {
  promise.catch(() => null);
}

function tokensEqual(left: string, right: string): boolean {
  const leftBytes = Buffer.from(left);
  const rightBytes = Buffer.from(right);
  return leftBytes.length === rightBytes.length && timingSafeEqual(leftBytes, rightBytes);
}

async function writeLine(socket: Socket, line: string): Promise<void> {
  if (socket.destroyed) {
    throw new Error("Pi gateway socket is closed");
  }
  await new Promise<void>((resolve, reject) => {
    socket.write(line, (error) => {
      if (error) {
        reject(error);
      } else {
        resolve();
      }
    });
  });
}

function hasErrorCode(error: unknown, code: string): boolean {
  return typeof error === "object" && error !== null && "code" in error && error.code === code;
}

async function removeStaleSocket(socketPath: string): Promise<void> {
  try {
    const stats = await lstat(socketPath);
    if (!stats.isSocket()) {
      throw new Error("Pi gateway socket path already exists and is not a socket");
    }
    await unlink(socketPath);
  } catch (error) {
    if (!hasErrorCode(error, "ENOENT")) {
      throw error;
    }
  }
}

async function removeOwnedSocket(socketPath: string): Promise<void> {
  try {
    await unlink(socketPath);
  } catch (error) {
    if (!hasErrorCode(error, "ENOENT")) {
      throw error;
    }
  }
}

async function listen(server: Server, socketPath: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, () => {
      server.off("error", reject);
      resolve();
    });
  });
}

async function closeServer(server: Server): Promise<void> {
  if (!server.listening) {
    return;
  }
  await new Promise<void>((resolve) => {
    server.close(() => {
      resolve();
    });
  });
}

class SocketWriter implements GatewayWriter {
  private readonly socket: Socket;
  private pending = Promise.resolve();

  constructor(socket: Socket) {
    this.socket = socket;
  }

  async write(message: ServerMessage): Promise<void> {
    const line = `${JSON.stringify(message)}\n`;
    const operation = this.pending.then(async () => {
      await writeLine(this.socket, line);
    });
    this.pending = operation.catch(() => {
      this.socket.destroy();
    });
    await operation;
  }
}

async function handleLine(
  line: string,
  token: string,
  writer: GatewayWriter,
  gateway: GatewayConnection,
  authenticated: boolean,
): Promise<boolean> {
  const message = parseClientMessage(line);
  if (!tokensEqual(message.token, token)) {
    throw new GatewayError("Pi gateway authentication failed", undefined, true);
  }
  if (!authenticated) {
    if (message.type !== "hello") {
      throw new GatewayError("Pi gateway expected hello as the first message", undefined, true);
    }
    await writer.write(serverMessage("ready"));
    return true;
  }
  if (message.type === "hello") {
    throw new GatewayError("Pi gateway hello may only be sent once");
  }
  gateway.handle(message);
  return true;
}

interface InputState {
  authenticated: boolean;
}

interface LineContext {
  token: string;
  writer: GatewayWriter;
  gateway: GatewayConnection;
  socket: Socket;
  state: InputState;
}

async function queueLine(queue: Promise<void>, line: string, context: LineContext): Promise<void> {
  await queue
    .then(async () => {
      context.state.authenticated = await handleLine(
        line,
        context.token,
        context.writer,
        context.gateway,
        context.state.authenticated,
      );
    })
    .catch(async (error: unknown) => {
      const requestId = error instanceof GatewayError ? error.requestId : undefined;
      await context.writer.write(
        serverMessage("protocol_error", {
          ...(requestId === undefined ? {} : { id: requestId }),
          message: errorMessage(error),
        }),
      );
      if (error instanceof GatewayError && error.fatal) {
        context.socket.destroy();
      }
    });
}

function bindSocket(
  socket: Socket,
  token: string,
  registry: ModelRegistry,
  onClose: () => void,
): void {
  const writer = new SocketWriter(socket);
  const gateway = new GatewayConnection(registry, writer);
  const state: InputState = { authenticated: false };
  const lineContext: LineContext = { token, writer, gateway, socket, state };
  let buffer = "";
  let inputQueue = Promise.resolve();
  socket.setEncoding("utf8");
  socket.on("data", (chunk: string) => {
    buffer += chunk;
    if (buffer.length > MAX_LINE_CHARACTERS) {
      const rejection = writer
        .write(serverMessage("protocol_error", { message: "Pi gateway input line is too large" }))
        .finally(() => {
          socket.destroy();
        });
      observe(rejection);
      return;
    }
    const lines = buffer.split("\n");
    buffer = lines.splice(-1, 1).join("");
    for (const line of lines) {
      const normalized = line.endsWith("\r") ? line.slice(0, -1) : line;
      if (normalized.length > 0) {
        inputQueue = queueLine(inputQueue, normalized, lineContext);
      }
    }
  });
  socket.on("close", () => {
    gateway.close();
    onClose();
  });
  socket.on("error", () => gateway.close());
}

export async function startGatewayServer(
  config: GatewayConfig,
  registry: ModelRegistry,
): Promise<GatewayServer> {
  await removeStaleSocket(config.socketPath);
  const activeSockets = new Set<Socket>();
  const server = createServer((socket) => {
    activeSockets.add(socket);
    bindSocket(socket, config.token, registry, () => {
      activeSockets.delete(socket);
    });
  });
  await listen(server, config.socketPath);
  await chmod(config.socketPath, 0o600);
  return {
    close: async (): Promise<void> => {
      for (const socket of activeSockets) {
        socket.destroy();
      }
      activeSockets.clear();
      await closeServer(server);
      await removeOwnedSocket(config.socketPath);
    },
  };
}
