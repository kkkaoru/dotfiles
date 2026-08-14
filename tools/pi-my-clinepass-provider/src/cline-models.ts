import { spawn } from "node:child_process";
import process from "node:process";
import { createInterface } from "node:readline";
import { clearTimeout, setTimeout } from "node:timers";
import { isRecord, stringValue } from "./utils.ts";

const ACP_PROTOCOL_VERSION = 1;
const CLINE_COMMAND = "cline";
const DEFAULT_TIMEOUT_MS = 10_000;
const INITIALIZE_REQUEST_ID = 1;
const NEW_SESSION_REQUEST_ID = 2;

export interface ClineCatalogModel {
  description?: string;
  modelId: string;
  name: string;
}

export interface DiscoverClineModelsOptions {
  command?: string;
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs?: number;
}

interface JsonRpcResponse {
  id: number;
  result?: unknown;
  error?: unknown;
}

interface ResponseWaitOptions {
  failure: Promise<never>;
  id: number;
  iterator: AsyncIterator<string>;
  readStderr: () => string;
}

interface AcpRuntime {
  close: () => void;
  send: (id: number, method: string, params: Record<string, unknown>) => void;
  waitFor: (id: number) => Promise<JsonRpcResponse>;
}

function request(id: number, method: string, params: Record<string, unknown>): string {
  return `${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`;
}

function parseResponse(line: string): JsonRpcResponse | undefined {
  try {
    const value: unknown = JSON.parse(line);
    if (!isRecord(value) || typeof value["id"] !== "number") {
      return undefined;
    }
    return { id: value["id"], result: value["result"], error: value["error"] };
  } catch {
    return undefined;
  }
}

async function waitForResponse(options: ResponseWaitOptions): Promise<JsonRpcResponse> {
  const next = await Promise.race([options.iterator.next(), options.failure]);
  if (next.done === true) {
    throw new Error(
      `cline ACP exited before response ${options.id}: ${options.readStderr().trim()}`,
    );
  }
  const response = parseResponse(next.value);
  return response?.id === options.id ? response : waitForResponse(options);
}

function parseCatalog(result: unknown): ClineCatalogModel[] {
  if (!isRecord(result)) {
    return [];
  }
  const models = isRecord(result["models"]) ? result["models"] : undefined;
  const available = models?.["availableModels"];
  if (!Array.isArray(available)) {
    return [];
  }

  return available.flatMap((entry): ClineCatalogModel[] => {
    if (!isRecord(entry)) {
      return [];
    }
    const modelId = stringValue(entry["modelId"]);
    const name = stringValue(entry["name"]);
    if (modelId === undefined || name === undefined) {
      return [];
    }
    const description = stringValue(entry["description"]);
    return [{ modelId, name, ...(description === undefined ? {} : { description }) }];
  });
}

function formatRpcError(error: unknown): string {
  if (!isRecord(error)) {
    return "unknown ACP error";
  }
  return stringValue(error["message"]) ?? "unknown ACP error";
}

function startAcpRuntime(options: DiscoverClineModelsOptions, cwd: string): AcpRuntime {
  const child = spawn(options.command ?? CLINE_COMMAND, ["--acp"], {
    cwd,
    env: { ...process.env, ...options.env, CLINE_PROVIDER: "cline-pass" },
    stdio: ["pipe", "pipe", "pipe"],
  });
  const lines = createInterface({ input: child.stdout, crlfDelay: Number.POSITIVE_INFINITY });
  const iterator = lines[Symbol.asyncIterator]();
  const failure = new Promise<never>((_resolve, reject) => {
    child.once("error", reject);
  });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk: string) => {
    stderr = `${stderr}${chunk}`.slice(-4096);
  });
  const timer = setTimeout(() => {
    child.kill();
  }, options.timeoutMs ?? DEFAULT_TIMEOUT_MS);
  timer.unref?.();

  return {
    close: () => {
      clearTimeout(timer);
      lines.close();
      child.stdin.end();
      if (child.exitCode === null) {
        child.kill();
      }
    },
    send: (id, method, params) => {
      child.stdin.write(request(id, method, params));
    },
    waitFor: async (id) => waitForResponse({ failure, id, iterator, readStderr: () => stderr }),
  };
}

export async function discoverClinePassModels(
  options: DiscoverClineModelsOptions = {},
): Promise<ClineCatalogModel[]> {
  const cwd = options.cwd ?? process.cwd();
  const runtime = startAcpRuntime(options, cwd);
  try {
    runtime.send(INITIALIZE_REQUEST_ID, "initialize", {
      protocolVersion: ACP_PROTOCOL_VERSION,
      clientCapabilities: {},
      clientInfo: { name: "pi-my-clinepass-provider", version: "0.1.0" },
    });
    const initialized = await runtime.waitFor(INITIALIZE_REQUEST_ID);
    if (initialized.error !== undefined) {
      throw new Error(`cline ACP initialize failed: ${formatRpcError(initialized.error)}`);
    }

    runtime.send(NEW_SESSION_REQUEST_ID, "session/new", { cwd, mcpServers: [] });
    const session = await runtime.waitFor(NEW_SESSION_REQUEST_ID);
    if (session.error !== undefined) {
      throw new Error(`cline ACP model discovery failed: ${formatRpcError(session.error)}`);
    }

    const catalog = parseCatalog(session.result).filter((model) =>
      model.modelId.startsWith("cline-pass/"),
    );
    if (catalog.length === 0) {
      throw new Error("cline ACP returned no ClinePass models");
    }
    return catalog;
  } finally {
    runtime.close();
  }
}
