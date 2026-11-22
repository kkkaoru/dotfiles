// This TypeScript file is executed with Bun.
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { vi } from "vitest";
import timelineExtension from "../index.ts";
import type { TranslatedTimeline } from "./translate.ts";

export interface Harness {
  readonly context: ExtensionContext;
  readonly handlers: Map<string, (event: unknown, context: ExtensionContext) => Promise<void>>;
  readonly notify: ReturnType<typeof vi.fn>;
  readonly sendMessage: ReturnType<typeof vi.fn>;
  readonly run: (params: unknown) => Promise<{ content: readonly { text: string }[] }>;
  readonly setStatus: ReturnType<typeof vi.fn>;
  readonly setWidget: ReturnType<typeof vi.fn>;
  readonly show: () => Promise<void>;
  /** The lines the widget would draw at `width`, from its last paint. */
  readonly widgetLines: (width?: number) => string[];
}

/** The extension wired to a fake pi, so a test can call its tool, command and handlers. */
export async function start(
  file?: string,
  display?: TranslatedTimeline,
  fg: (token: string, text: string) => string = (_token, text) => text,
): Promise<Harness> {
  const handlers = new Map<string, (event: unknown, context: ExtensionContext) => Promise<void>>();
  const notify = vi.fn();
  const sendMessage = vi.fn();
  const setStatus = vi.fn();
  const setWidget = vi.fn();
  // Registered by the extension below, so a holder keeps them out of an uninitialised variable.
  const registered: {
    command?: (args: string, context: ExtensionContext) => Promise<void>;
    tool?: (
      id: string,
      params: unknown,
      signal: undefined,
      update: undefined,
      ctx: ExtensionContext,
    ) => Promise<unknown>;
  } = {};
  timelineExtension(
    {
      on: (name: string, handler: (event: unknown, context: ExtensionContext) => Promise<void>) => {
        handlers.set(name, handler);
      },
      registerCommand: (
        _name: string,
        definition: { handler: NonNullable<typeof registered.command> },
      ) => {
        registered.command = definition.handler;
      },
      registerTool: (definition: { execute: NonNullable<typeof registered.tool> }) => {
        registered.tool = definition.execute;
      },
      sendMessage: (...args: unknown[]): void => {
        sendMessage(...args);
      },
    } as unknown as ExtensionAPI,
    { display },
  );

  const context = {
    sessionManager: { getSessionFile: () => file },
    ui: { notify, setStatus, setWidget, theme: { fg } },
  } as unknown as ExtensionContext;
  await handlers.get("session_start")?.({}, context);
  return {
    context,
    handlers,
    notify,
    sendMessage,
    setStatus,
    setWidget,
    widgetLines(width = 200) {
      const last = setWidget.mock.calls.at(-1) as [string, unknown] | undefined;
      const content = last?.[1];
      if (typeof content !== "function") {
        return [];
      }
      const component = (
        content as (tui?: unknown, theme?: unknown) => { render: (width: number) => string[] }
      )();
      return component.render(width);
    },
    async run(params: unknown) {
      const result = (await registered.tool?.("call", params, undefined, undefined, context)) as {
        content: readonly { text: string }[];
      };
      return result;
    },
    async show() {
      await registered.command?.("", context);
    },
  };
}
