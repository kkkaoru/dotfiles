// This TypeScript file is executed with Bun.
import type {
  InputEvent,
  InputEventResult,
  MarkdownMessageType,
  MarkdownTransformContext,
} from "@earendil-works/pi-coding-agent";
import { DISPLAY_TYPES } from "./policy.ts";
import {
  displayOf,
  installRule,
  scheduleReply,
  startSession,
  translateInput,
  type Runtime,
  type UiSink,
} from "./runtime.ts";

export interface PlamoHooks {
  onSessionStart(handler: (ctx: { readonly ui: UiSink }) => void): void;
  onInput(
    handler: (event: InputEvent, ctx: { readonly ui: UiSink }) => Promise<InputEventResult>,
  ): void;
  onBeforeAgent(handler: (sections: Record<string, string>) => void): void;
  onMessageEnd(
    handler: (message: { readonly content?: unknown; readonly role: string }) => void,
  ): void;
  onMarkdown(
    transformer: (markdown: string, context: MarkdownTransformContext) => string,
    messageTypes: readonly MarkdownMessageType[],
  ): void;
}

export function bindPlamo(hooks: PlamoHooks, runtime: Runtime): void {
  hooks.onSessionStart((ctx) => {
    startSession(runtime, ctx.ui);
  });
  hooks.onInput((event, ctx) => translateInput(runtime, event, ctx.ui));
  hooks.onBeforeAgent((sections) => {
    installRule(runtime, sections);
  });
  hooks.onMessageEnd((message) => {
    scheduleReply(runtime, message);
  });
  hooks.onMarkdown((markdown, context) => displayOf(runtime, markdown, context), DISPLAY_TYPES);
}
