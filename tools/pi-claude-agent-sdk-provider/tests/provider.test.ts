// This file runs with Bun.
import { afterEach, expect, it, vi } from "vitest";
import type {
  Api,
  AssistantMessage,
  AssistantMessageEventStream,
  Context,
  Model,
  Tool,
} from "@earendil-works/pi-ai";
import {
  closeAllSessions,
  defaultRuntime,
  streamClaudeSdk,
  type ProviderRuntime,
} from "../src/provider.ts";
import { fakeStartQuery, sdk, textTurn, toolTurn } from "./fake-query.ts";

const model: Model<Api> = {
  id: "claude-haiku-5-5",
  name: "Claude Haiku 5.5",
  api: "claude-agent-sdk",
  provider: "claude-sdk",
  baseUrl: "https://api.anthropic.com",
  reasoning: true,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1_000_000,
  maxTokens: 128_000,
};

const bashTool: Tool = {
  name: "bash",
  description: "Run a command",
  parameters: { type: "object", properties: { command: { type: "string" } } },
};

afterEach(() => {
  closeAllSessions("test finished");
  vi.useRealTimers();
});

function runtime(start: ProviderRuntime["startQuery"], printMode = false): ProviderRuntime {
  return { cwd: () => "/work", printMode, startQuery: start };
}

async function result(stream: AssistantMessageEventStream): Promise<AssistantMessage> {
  for await (const event of stream) {
    if (event.type === "done") {
      return event.message;
    }
    if (event.type === "error") {
      return event.error;
    }
  }
  throw new Error("stream ended without a terminal event");
}

it("streams a text reply and passes plan-safe SDK options", async () => {
  const { start, fakes } = fakeStartQuery((fake) => {
    fake.emit(...textTurn("hello"));
  });
  const hook = vi.fn();
  const context: Context = {
    systemPrompt: "sys",
    messages: [{ role: "user", content: "hi", timestamp: 1 }],
    tools: [bashTool],
  };
  const message = await result(
    streamClaudeSdk(
      model,
      context,
      { sessionId: "text", reasoning: "minimal", onProviderStreamEvent: hook },
      runtime(start),
    ),
  );
  expect(message.stopReason).toBe("stop");
  expect(message.content).toStrictEqual([{ type: "text", text: "hello" }]);
  expect(message.usage.output).toBe(3);
  expect(hook).toHaveBeenCalledTimes(7);
  expect(fakes[0]?.options).toMatchObject({
    model: "claude-haiku-5-5",
    tools: [],
    settingSources: [],
    systemPrompt: "sys",
    allowedTools: ["mcp__pi__bash"],
    effort: "low",
    thinking: { type: "adaptive" },
    cwd: "/work",
  });
  expect(fakes[0]?.inputs[0]?.message.content).toStrictEqual([{ type: "text", text: "hi" }]);
});

it("hands tool calls to pi and returns pi's results to the waiting SDK call", async () => {
  const toolResults: unknown[] = [];
  const { start, fakes } = fakeStartQuery(async (fake) => {
    fake.emit(...toolTurn("toolu_1", "bash", { command: "ls" }));
    toolResults.push(await fake.callTool("bash", { command: "ls" }, "toolu_1"));
    fake.emit(...textTurn("done"));
  });
  const first: Context = {
    systemPrompt: "sys",
    messages: [{ role: "user", content: "list", timestamp: 1 }],
    tools: [bashTool],
  };
  const call = await result(streamClaudeSdk(model, first, { sessionId: "tools" }, runtime(start)));
  expect(call.stopReason).toBe("toolUse");
  expect(call.content).toStrictEqual([
    { type: "toolCall", id: "toolu_1", name: "bash", arguments: { command: "ls" } },
  ]);
  const second: Context = {
    ...first,
    messages: [
      ...first.messages,
      call,
      {
        role: "toolResult",
        toolCallId: "toolu_1",
        toolName: "bash",
        content: [{ type: "text", text: "a.txt" }],
        isError: false,
        timestamp: 3,
      },
    ],
  };
  const reply = await result(
    streamClaudeSdk(model, second, { sessionId: "tools" }, runtime(start)),
  );
  expect(reply.content).toStrictEqual([{ type: "text", text: "done" }]);
  expect(toolResults).toStrictEqual([
    { content: [{ type: "text", text: "a.txt" }], isError: false },
  ]);
  expect(fakes).toHaveLength(1);
  expect(fakes[0]?.options.thinking).toStrictEqual({ type: "disabled" });
});

it("sends follow-up user input into the same SDK session", async () => {
  const { start, fakes } = fakeStartQuery((fake, input) => {
    fake.emit(...textTurn(`echo ${JSON.stringify(input.message.content)}`));
  });
  const first: Context = {
    systemPrompt: "sys",
    messages: [{ role: "user", content: "one", timestamp: 1 }],
  };
  const reply = await result(
    streamClaudeSdk(model, first, { sessionId: "follow" }, runtime(start)),
  );
  const second: Context = {
    ...first,
    messages: [...first.messages, reply, { role: "user", content: "two", timestamp: 5 }],
  };
  await result(streamClaudeSdk(model, second, { sessionId: "follow" }, runtime(start)));
  expect(fakes).toHaveLength(1);
  expect(fakes[0]?.inputs.map((input) => input.message.content)).toStrictEqual([
    [{ type: "text", text: "one" }],
    [{ type: "text", text: "two" }],
  ]);
});

it("applies a changed effort live and restarts on a changed system prompt", async () => {
  const { start, fakes } = fakeStartQuery((fake) => {
    fake.emit(...textTurn("ok"));
  });
  const first: Context = {
    systemPrompt: "sys",
    messages: [{ role: "user", content: "one", timestamp: 1 }],
  };
  const reply = await result(
    streamClaudeSdk(model, first, { sessionId: "live", reasoning: "medium" }, runtime(start)),
  );
  const second: Context = {
    ...first,
    messages: [...first.messages, reply, { role: "user", content: "two", timestamp: 5 }],
  };
  const again = await result(
    streamClaudeSdk(model, second, { sessionId: "live", reasoning: "max" }, runtime(start)),
  );
  expect(fakes[0]?.effortLevels).toStrictEqual(["max"]);
  const third: Context = {
    systemPrompt: "changed",
    messages: [...second.messages, again, { role: "user", content: "three", timestamp: 9 }],
  };
  await result(
    streamClaudeSdk(model, third, { sessionId: "live", reasoning: "max" }, runtime(start)),
  );
  expect(fakes).toHaveLength(2);
  expect(fakes[0]?.closed).toBe(true);
  expect(fakes[1]?.inputs[0]?.message.content).toStrictEqual([
    {
      type: "text",
      text: "USER:\none\n\nASSISTANT:\nok\n\nUSER:\ntwo\n\nASSISTANT:\nok\n\nUSER:\nthree\n\nContinue from the transcript above. Earlier tool calls already ran; follow the latest request.",
    },
  ]);
});

it("closes the session after a print-mode reply", async () => {
  const { start, fakes } = fakeStartQuery((fake) => {
    fake.emit(...textTurn("bye"));
  });
  const context: Context = { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  await result(streamClaudeSdk(model, context, { sessionId: "print" }, runtime(start, true)));
  expect(fakes[0]?.closed).toBe(true);
});

it("closes an idle session after the idle timeout", async () => {
  vi.useFakeTimers();
  const { start, fakes } = fakeStartQuery((fake) => {
    fake.emit(...textTurn("hi"));
  });
  const context: Context = { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  await result(streamClaudeSdk(model, context, { sessionId: "idle" }, runtime(start)));
  expect(fakes[0]?.closed).toBe(false);
  await vi.advanceTimersByTimeAsync(15 * 60 * 1000);
  expect(fakes[0]?.closed).toBe(true);
});

it("reports SDK failures and closes the session", async () => {
  const { start, fakes } = fakeStartQuery((fake) => {
    fake.emit(
      sdk({
        type: "result",
        subtype: "error_during_execution",
        is_error: true,
        errors: ["rate limited", "retry later"],
      }),
    );
  });
  const context: Context = { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  const message = await result(
    streamClaudeSdk(model, context, { sessionId: "fail" }, runtime(start)),
  );
  expect(message.stopReason).toBe("error");
  expect(message.errorMessage).toBe(
    "Claude Agent SDK error_during_execution: rate limited; retry later",
  );
  expect(fakes[0]?.closed).toBe(true);
});

it("reports an aborted turn and closes the SDK session", async () => {
  const controller = new AbortController();
  const { start, fakes } = fakeStartQuery(() => {
    controller.abort();
  });
  const context: Context = { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  const message = await result(
    streamClaudeSdk(
      model,
      context,
      { sessionId: "abort", signal: controller.signal },
      runtime(start),
    ),
  );
  expect(message.stopReason).toBe("aborted");
  expect(fakes[0]?.closed).toBe(true);
});

it("fails immediately for a request aborted before it starts", async () => {
  const controller = new AbortController();
  controller.abort();
  const { start, fakes } = fakeStartQuery(() => undefined);
  const context: Context = { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  const message = await result(
    streamClaudeSdk(model, context, { signal: controller.signal }, runtime(start)),
  );
  expect(message.stopReason).toBe("aborted");
  expect(fakes).toHaveLength(0);
});

it("reports a failure to start the SDK", async () => {
  const context: Context = { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  const message = await result(
    streamClaudeSdk(
      model,
      context,
      { sessionId: "start" },
      runtime(() => {
        throw new Error("claude binary missing");
      }),
    ),
  );
  expect(message.stopReason).toBe("error");
  expect(message.errorMessage).toBe("claude binary missing");
});

it("detects pi print mode from the process arguments", () => {
  const argv = process.argv;
  process.argv = ["node", "pi", "-p", "hi"];
  expect(defaultRuntime().printMode).toBe(true);
  process.argv = ["node", "pi"];
  expect(defaultRuntime().printMode).toBe(false);
  expect(defaultRuntime().cwd()).toBe(process.cwd());
  process.argv = argv;
});

it("keeps the session when pi adds a transient system update with tool results", async () => {
  const toolResults: unknown[] = [];
  const { start, fakes } = fakeStartQuery(async (fake) => {
    fake.emit(...toolTurn("toolu_9", "bash", { command: "ls" }));
    toolResults.push(await fake.callTool("bash", { command: "ls" }, "toolu_9"));
    fake.emit(...textTurn("done"));
  });
  const first: Context = {
    systemPrompt: "sys",
    messages: [{ role: "user", content: "list", timestamp: 1 }],
    tools: [bashTool],
  };
  const call = await result(
    streamClaudeSdk(model, first, { sessionId: "updates" }, runtime(start)),
  );
  const second: Context = {
    ...first,
    messages: [
      ...first.messages,
      call,
      {
        role: "toolResult",
        toolCallId: "toolu_9",
        toolName: "bash",
        content: [{ type: "text", text: "a.txt" }],
        isError: false,
        timestamp: 3,
      },
      { role: "system", content: "Progress note", timestamp: 4 },
    ],
  };
  await result(streamClaudeSdk(model, second, { sessionId: "updates" }, runtime(start)));
  expect(fakes).toHaveLength(1);
  expect(toolResults).toStrictEqual([
    {
      content: [
        { type: "text", text: "a.txt" },
        { type: "text", text: "Progress note" },
      ],
      isError: false,
    },
  ]);
});
