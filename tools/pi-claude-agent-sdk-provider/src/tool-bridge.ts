// This file runs with Bun.
import type { McpSdkServerConfigWithInstance } from "@anthropic-ai/claude-agent-sdk";
import type { Tool, ToolResultMessage } from "@earendil-works/pi-ai";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  type CallToolRequest,
  type CallToolResult,
  type ListToolsResult,
} from "@modelcontextprotocol/sdk/types.js";

interface Deferred {
  readonly promise: Promise<CallToolResult>;
  readonly resolve: (result: CallToolResult) => void;
  readonly reject: (error: Error) => void;
}

type McpTool = ListToolsResult["tools"][number];

export const PI_MCP_SERVER = "pi";
export const PI_TOOL_PREFIX = `mcp__${PI_MCP_SERVER}__`;
// Pi owns tool execution time (approvals, long builds), so the SDK must not cut it short.
const TOOL_CALL_TIMEOUT_MS = 24 * 60 * 60 * 1000;
const TOOL_USE_ID_META_KEY = "claudecode/toolUseId";
const SERVER_VERSION = "0.1.0";

function createDeferred(): Deferred {
  const deferred = Promise.withResolvers<CallToolResult>();
  // A call rejected during shutdown may never be awaited by the SDK.
  deferred.promise.catch(() => null);
  return deferred;
}

/** Tool results that pi will deliver on a later turn, keyed by Anthropic tool_use id. */
export class PendingToolCalls {
  readonly #calls = new Map<string, Deferred>();

  #entry(id: string): Deferred {
    const existing = this.#calls.get(id);
    if (existing !== undefined) {
      return existing;
    }
    const created = createDeferred();
    this.#calls.set(id, created);
    return created;
  }

  async expect(id: string): Promise<CallToolResult> {
    try {
      return await this.#entry(id).promise;
    } finally {
      this.#calls.delete(id);
    }
  }

  /** Claude Code may call parallel tools one after another, so a result can arrive first. */
  settle(id: string, result: CallToolResult): void {
    this.#entry(id).resolve(result);
  }

  rejectAll(error: Error): void {
    for (const call of this.#calls.values()) {
      call.reject(error);
    }
    this.#calls.clear();
  }
}

export function piToolName(sdkToolName: string): string | undefined {
  return sdkToolName.startsWith(PI_TOOL_PREFIX)
    ? sdkToolName.slice(PI_TOOL_PREFIX.length)
    : undefined;
}

export function sdkToolNames(tools: readonly Tool[]): string[] {
  return tools.map((tool) => `${PI_TOOL_PREFIX}${tool.name}`);
}

export function toCallToolResult(
  message: ToolResultMessage,
  notes: readonly string[],
): CallToolResult {
  return {
    content: [
      ...message.content.map((part): CallToolResult["content"][number] =>
        part.type === "text"
          ? { type: "text", text: part.text }
          : { type: "image", data: part.data, mimeType: part.mimeType },
      ),
      ...notes.map((text): CallToolResult["content"][number] => ({ type: "text", text })),
    ],
    isError: message.isError,
  };
}

function toMcpTool(tool: Tool): McpTool {
  const schema: Record<string, unknown> = { ...tool.parameters };
  return {
    name: tool.name,
    description: tool.description,
    inputSchema: { ...schema, type: "object" },
    // Without this the CLI may defer the schema behind tool search, which is disabled here.
    _meta: { "anthropic/alwaysLoad": true },
  };
}

function toolUseId(request: CallToolRequest): string | undefined {
  const { _meta: meta } = request.params;
  const value = meta?.[TOOL_USE_ID_META_KEY];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

async function callTool(
  request: CallToolRequest,
  pending: PendingToolCalls,
): Promise<CallToolResult> {
  const id = toolUseId(request);
  return id === undefined
    ? {
        content: [{ type: "text", text: `Tool call ${request.params.name} has no tool_use id` }],
        isError: true,
      }
    : pending.expect(id);
}

/** Exposes pi's tool declarations; pi itself executes every call. */
export function createPiMcpServer(
  tools: readonly Tool[],
  pending: PendingToolCalls,
): McpSdkServerConfigWithInstance {
  const instance = new McpServer(
    { name: PI_MCP_SERVER, version: SERVER_VERSION },
    { capabilities: { tools: {} } },
  );
  const listed: ListToolsResult = { tools: tools.map((tool) => toMcpTool(tool)) };
  instance.server.setRequestHandler(ListToolsRequestSchema, () => listed);
  instance.server.setRequestHandler(CallToolRequestSchema, async (request) =>
    callTool(request, pending),
  );
  return { type: "sdk", name: PI_MCP_SERVER, instance, timeout: TOOL_CALL_TIMEOUT_MS };
}
