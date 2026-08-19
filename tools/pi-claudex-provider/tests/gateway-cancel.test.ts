import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { DirectStreamInput } from "../src/direct-stream.ts";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GatewayConnection } from "../src/gateway.ts";
import {
  parseClientMessage,
  type ServerMessage,
  type StreamRequestMessage,
} from "../src/protocol.ts";

const streamMock = vi.hoisted(() => vi.fn<(input: DirectStreamInput) => Promise<void>>());
vi.mock("../src/direct-stream.ts", () => ({ streamDirectModel: streamMock }));

const TOKEN = "01234567890123456789012345678901";
const MODEL = {
  provider: "ollama-cloud",
  id: "glm-5.2",
  name: "GLM",
  api: "openai-completions",
  reasoning: true,
  input: ["text"],
  contextWindow: 100,
  maxTokens: 10,
};

function request(id = "request"): StreamRequestMessage {
  const parsed = parseClientMessage(
    JSON.stringify({
      version: 1,
      type: "request",
      id,
      token: TOKEN,
      origin: "claudex",
      provider: MODEL.provider,
      modelId: MODEL.id,
      system: null,
      messages: [],
      tools: [],
      options: {},
    }),
  );
  if (parsed.type !== "request") {
    throw new Error("invalid fixture");
  }
  return parsed;
}

function registry() {
  return {
    getAvailable: () => [MODEL],
  } as unknown as ExtensionContext["modelRegistry"];
}

async function settle() {
  await new Promise<void>((resolve) => {
    setTimeout(resolve, 0);
  });
}

beforeEach(() => {
  streamMock.mockReset();
  streamMock.mockResolvedValue(undefined);
});

describe("gateway cancel", () => {
  it("emits a terminal error immediately on cancel while the provider hangs", async () => {
    streamMock.mockImplementation(async () => new Promise<void>(() => {}));
    const messages: ServerMessage[] = [];
    const gateway = new GatewayConnection(registry(), {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle(request("hang"));
    await settle();
    gateway.handle({ version: 1, type: "cancel", id: "hang", token: TOKEN });
    await settle();
    expect(messages).toStrictEqual([
      {
        version: 1,
        type: "error",
        id: "hang",
        reason: "error",
        error: { errorMessage: "Cancelled by Claudex" },
      },
    ]);
  });
});
