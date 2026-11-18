// This TypeScript file is executed with Bun.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { capImages, imageCapsOf } from "./src/budget.ts";

// Pi describes a model's image caps but does not enforce them, so this trims each request.
// It keeps the newest images of the request and of each message.
export default function imageBudget(pi: ExtensionAPI): void {
  const warned = { done: false };
  pi.on("context", (event, ctx) => {
    const caps = imageCapsOf(ctx.model?.inputLimits?.images);
    if (caps === undefined) {
      return;
    }
    const dropped = capImages(event.messages, caps.keep, caps.perMessage);
    if (dropped > 0 && !warned.done) {
      warned.done = true;
      ctx.ui.notify(
        `image-budget: ${String(dropped)} older images are left out of this request; the newest ones are sent.`,
        "info",
      );
    }
    return { messages: event.messages };
  });
}
