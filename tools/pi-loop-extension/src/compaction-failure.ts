// This TypeScript file is executed with Bun.
import type { LoopContext, LoopRuntime } from "./runtime.ts";

export function failedAgentRun(event: unknown): boolean {
  if (
    typeof event !== "object" ||
    event === null ||
    !("messages" in event) ||
    !Array.isArray(event.messages)
  ) {
    return false;
  }
  const messages: readonly unknown[] = event.messages;
  const assistant = messages.findLast(
    (message): message is { readonly role: "assistant"; readonly stopReason: unknown } =>
      typeof message === "object" &&
      message !== null &&
      "role" in message &&
      message.role === "assistant" &&
      "stopReason" in message,
  );
  return assistant?.stopReason === "error" || assistant?.stopReason === "aborted";
}

export function pauseAfterAgentFailure(runtime: LoopRuntime, context: LoopContext): void {
  if (!runtime.ownsContinuation()) {
    return;
  }
  runtime.command("pause", context);
  context.ui.notify(
    "Loop paused after an agent error or abort. Resolve the failure before /loop resume.",
    "warning",
  );
}

export function pauseAfterCompactionFailure(runtime: LoopRuntime, context: LoopContext): void {
  if (!runtime.ownsContinuation()) {
    return;
  }
  runtime.command("pause", context);
  context.ui.notify(
    "Loop paused after compaction failed. Recover with /compact or a new session before /loop resume.",
    "warning",
  );
}
