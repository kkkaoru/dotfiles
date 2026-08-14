import process from "node:process";
import { PROVIDER_NAME } from "./env.ts";
import { classifyClinePassError } from "./errors.ts";
import { isRecord, stringValue } from "./utils.ts";

interface MessageEndEvent {
  message: unknown;
}

interface ErrorHandlerContext {
  hasUI: boolean;
  model?: { provider?: string };
  ui: { notify: (message: string, type: "info" | "warning" | "error") => void };
}

export function handleClinePassError(event: MessageEndEvent, context: ErrorHandlerContext): void {
  if (!isRecord(event.message)) {
    return;
  }

  const stopReason = stringValue(event.message["stopReason"]);
  const errorMessage = stringValue(event.message["errorMessage"]);
  if (stopReason !== "error" || errorMessage === undefined) {
    return;
  }

  const provider = stringValue(event.message["provider"]) ?? context.model?.provider;
  if (provider !== PROVIDER_NAME) {
    return;
  }

  const friendlyMessage = classifyClinePassError(errorMessage).message;
  if (context.hasUI) {
    context.ui.notify(friendlyMessage, "error");
    return;
  }
  process.stderr.write(`[clinepass] ${friendlyMessage}\n`);
}
