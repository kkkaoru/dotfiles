// This file runs with Bun.
import { expect, it } from "vitest";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { Tool, ToolResultMessage } from "@earendil-works/pi-ai";
import {
  createPiMcpServer,
  PendingToolCalls,
  piToolName,
  PI_TOOL_PREFIX,
  sdkToolNames,
  toCallToolResult,
} from "../src/tool-bridge.ts";

const bashTool: Tool = {
  name: "bash",
  description: "Run a command",
  parameters: {
    type: "object",
    properties: { command: { type: "string" } },
    required: ["command"],
  },
};

async function connect(tools: Tool[], pending: PendingToolCalls): Promise<Client> {
  const server = createPiMcpServer(tools, pending);
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await server.instance.connect(serverTransport);
  const client = new Client({ name: "test", version: "0.0.0" });
  await client.connect(clientTransport);
  return client;
}

it("maps pi tool names to and from the SDK MCP namespace", () => {
  expect(PI_TOOL_PREFIX).toBe("mcp__pi__");
  expect(sdkToolNames([bashTool])).toStrictEqual(["mcp__pi__bash"]);
  expect(piToolName("mcp__pi__bash")).toBe("bash");
  expect(piToolName("Bash")).toBe(undefined);
});

it("converts pi tool results, keeping images and the error flag", () => {
  const message: ToolResultMessage = {
    role: "toolResult",
    toolCallId: "toolu_1",
    toolName: "read",
    content: [
      { type: "text", text: "body" },
      { type: "image", data: "AAAA", mimeType: "image/png" },
    ],
    isError: true,
    timestamp: 1,
  };
  expect(toCallToolResult(message, ["system note"])).toStrictEqual({
    content: [
      { type: "text", text: "body" },
      { type: "image", data: "AAAA", mimeType: "image/png" },
      { type: "text", text: "system note" },
    ],
    isError: true,
  });
});

it("resolves a result settled before the handler starts waiting", async () => {
  const pending = new PendingToolCalls();
  pending.settle("toolu_1", { content: [{ type: "text", text: "early" }] });
  expect(await pending.expect("toolu_1")).toStrictEqual({
    content: [{ type: "text", text: "early" }],
  });
});

it("resolves a handler that is already waiting", async () => {
  const pending = new PendingToolCalls();
  const waiting = pending.expect("toolu_2");
  pending.settle("toolu_2", { content: [{ type: "text", text: "late" }] });
  expect(await waiting).toStrictEqual({ content: [{ type: "text", text: "late" }] });
});

it("rejects every waiting handler on shutdown", async () => {
  const pending = new PendingToolCalls();
  const waiting = pending.expect("toolu_3");
  pending.rejectAll(new Error("closed"));
  await expect(waiting).rejects.toThrow("closed");
});

it("lists pi tools with their JSON schema and always-load metadata", async () => {
  const client = await connect([bashTool], new PendingToolCalls());
  const listed = await client.listTools();
  expect(listed.tools).toStrictEqual([
    {
      name: "bash",
      description: "Run a command",
      inputSchema: {
        type: "object",
        properties: { command: { type: "string" } },
        required: ["command"],
      },
      _meta: { "anthropic/alwaysLoad": true },
    },
  ]);
});

it("answers a tool call with the result pi delivers for its tool_use id", async () => {
  const pending = new PendingToolCalls();
  const client = await connect([bashTool], pending);
  const call = client.callTool({
    name: "bash",
    arguments: { command: "echo hi" },
    _meta: { "claudecode/toolUseId": "toolu_4" },
  });
  pending.settle("toolu_4", { content: [{ type: "text", text: "hi" }] });
  expect(await call).toStrictEqual({ content: [{ type: "text", text: "hi" }] });
});

it("reports a tool call that carries no tool_use id", async () => {
  const client = await connect([bashTool], new PendingToolCalls());
  expect(await client.callTool({ name: "bash", arguments: {} })).toStrictEqual({
    content: [{ type: "text", text: "Tool call bash has no tool_use id" }],
    isError: true,
  });
});
