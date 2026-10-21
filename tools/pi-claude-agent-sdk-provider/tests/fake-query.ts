// This file runs with Bun.
import type { Options, SDKMessage, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { AsyncQueue } from "../src/async-queue.ts";
import type { SessionQuery, StartQuery } from "../src/session.ts";

export interface FakeQuery {
  readonly options: Options;
  readonly inputs: SDKUserMessage[];
  readonly effortLevels: unknown[];
  closed: boolean;
  readonly emit: (...events: SDKMessage[]) => void;
  readonly throwError: (error: Error) => void;
  readonly finish: () => void;
  readonly callTool: (
    name: string,
    args: Record<string, unknown>,
    toolUseId: string,
  ) => Promise<unknown>;
}

export type Script = (fake: FakeQuery, input: SDKUserMessage) => Promise<void> | void;

/** JSON round trip lets tests write SDK wire fixtures without restating every optional field. */
export function sdk(value: object): SDKMessage {
  const json = JSON.stringify(value);
  const parsed: SDKMessage = JSON.parse(json);
  return parsed;
}

export function textTurn(text: string): SDKMessage[] {
  return [
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "message_start",
        message: { usage: { input_tokens: 5, cache_read_input_tokens: 7 } },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: { type: "content_block_stop", index: 0 },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "message_delta",
        delta: { stop_reason: "end_turn" },
        usage: {
          input_tokens: 5,
          output_tokens: 3,
          cache_read_input_tokens: 7,
          cache_creation_input_tokens: 1,
        },
      },
    }),
    sdk({ type: "stream_event", parent_tool_use_id: null, event: { type: "message_stop" } }),
    sdk({ type: "result", subtype: "success", is_error: false, result: text }),
  ];
}

export function toolTurn(id: string, name: string, input: Record<string, unknown>): SDKMessage[] {
  return [
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: { type: "message_start", message: { usage: { input_tokens: 4 } } },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_start",
        index: 0,
        content_block: { type: "tool_use", id, name: `mcp__pi__${name}`, input: {} },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_delta",
        index: 0,
        delta: { type: "input_json_delta", partial_json: JSON.stringify(input) },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: { type: "content_block_stop", index: 0 },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "message_delta",
        delta: { stop_reason: "tool_use" },
        usage: { output_tokens: 9 },
      },
    }),
    sdk({ type: "stream_event", parent_tool_use_id: null, event: { type: "message_stop" } }),
  ];
}

async function connectPiTools(options: Options): Promise<Client> {
  const server = options.mcpServers?.["pi"];
  if (server?.type !== "sdk" || !("instance" in server)) {
    throw new Error("pi MCP server missing");
  }
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await server.instance.connect(serverTransport);
  const client = new Client({ name: "fake-claude-code", version: "0.0.0" });
  await client.connect(clientTransport);
  return client;
}

function createFake(options: Options, events: AsyncQueue<Error | SDKMessage>): FakeQuery {
  const client = connectPiTools(options);
  return {
    options,
    inputs: [],
    effortLevels: [],
    closed: false,
    emit: (...items) => {
      for (const item of items) {
        events.push(item);
      }
    },
    throwError: (error) => {
      events.push(error);
    },
    finish: () => {
      events.end();
    },
    callTool: async (name, args, toolUseId) => {
      const result = await (
        await client
      ).callTool({
        name,
        arguments: args,
        _meta: { "claudecode/toolUseId": toolUseId },
      });
      return result;
    },
  };
}

async function* iterate(events: AsyncQueue<Error | SDKMessage>): AsyncGenerator<SDKMessage> {
  for await (const item of events) {
    if (item instanceof Error) {
      throw item;
    }
    yield item;
  }
}

export function fakeStartQuery(script: Script): { start: StartQuery; fakes: FakeQuery[] } {
  const fakes: FakeQuery[] = [];
  const start: StartQuery = ({ prompt, options }) => {
    const events = new AsyncQueue<Error | SDKMessage>();
    const fake = createFake(options, events);
    fakes.push(fake);
    const consume = async (): Promise<void> => {
      for await (const input of prompt) {
        fake.inputs.push(input);
        await script(fake, input);
      }
    };
    consume().catch(() => null);
    const query: SessionQuery = {
      [Symbol.asyncIterator]: () => iterate(events),
      applyFlagSettings: async (settings) => {
        fake.effortLevels.push(settings.effortLevel);
      },
      close: () => {
        fake.closed = true;
        events.end();
      },
    };
    return query;
  };
  return { start, fakes };
}
