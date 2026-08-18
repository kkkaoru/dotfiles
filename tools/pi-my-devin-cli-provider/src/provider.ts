// This file runs with Bun.
import process from "node:process";
import type { SessionUpdate } from "@agentclientprotocol/sdk";
import type {
  Api,
  AssistantMessageEventStream,
  Context,
  Model,
  SimpleStreamOptions,
} from "@earendil-works/pi-ai";
import { buildContinuationPrompt, buildDevinTranscript } from "./context.ts";
import { createDevinOutput, type DevinOutput } from "./stream-output.ts";
import { resolveDevinSessionId, runDevinJob } from "./runtime.ts";

interface ActiveState {
  count: number;
}

interface RunRequest {
  context: Context;
  model: Model<Api>;
  output: DevinOutput;
  sessionId: string;
  signal: AbortSignal | undefined;
}

const activeState: ActiveState = { count: 0 };

function handleUpdate(update: SessionUpdate, output: DevinOutput): void {
  if (update.sessionUpdate === "agent_message_chunk" && update.content.type === "text") {
    output.appendText(update.content.text);
  }
  if (update.sessionUpdate === "agent_thought_chunk" && update.content.type === "text") {
    output.appendThinking(update.content.text);
  }
}

async function waitUntilIdle(): Promise<void> {
  if (activeState.count === 0) return;
  await new Promise<void>((resolve) => setImmediate(resolve));
  return waitUntilIdle();
}

async function runAcpRequest(request: RunRequest): Promise<void> {
  activeState.count += 1;
  try {
    await runDevinJob({
      continuationPrompt: buildContinuationPrompt(request.context),
      cwd: process.cwd(),
      initialPrompt: buildDevinTranscript(request.context),
      modelId: request.model.id,
      sessionId: request.sessionId,
      signal: request.signal,
      onUpdate: (update) => handleUpdate(update, request.output),
    });
    request.output.finish();
  } finally {
    activeState.count -= 1;
  }
}

export { createDevinSessionId, resolveDevinSessionId, selectPermission } from "./runtime.ts";

export function streamDevin(
  ...parameters: [model: Model<Api>, context: Context, options?: SimpleStreamOptions]
): AssistantMessageEventStream {
  const [model, context, options] = parameters;
  const output: DevinOutput = createDevinOutput(model);
  const sessionId: string = resolveDevinSessionId(options?.sessionId);
  void runAcpRequest({ model, context, output, sessionId, signal: options?.signal }).catch(
    (error: unknown) => {
      output.fail(error, options?.signal?.aborted === true);
    },
  );
  return output.stream;
}

export const devinProviderTestApi = {
  activeCount: (): number => activeState.count,
  waitForIdle: (): Promise<void> => waitUntilIdle(),
};
