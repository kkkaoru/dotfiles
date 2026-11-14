// This TypeScript file is executed with Bun.
import type { InputEvent, MarkdownTransformContext } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import { bindPlamo, type PlamoHooks } from "./bind.ts";
import { createRuntime, type UiSink } from "./runtime.ts";
import type { Server } from "./server.ts";

interface Stored {
  session: ((ctx: { readonly ui: UiSink }) => void) | undefined;
  input: ((event: InputEvent, ctx: { readonly ui: UiSink }) => Promise<unknown>) | undefined;
  rule: ((sections: Record<string, string>) => void) | undefined;
  reply: ((message: { readonly role: string }) => void) | undefined;
  markdown: ((markdown: string, context: MarkdownTransformContext) => string) | undefined;
}

function capture(): { readonly hooks: PlamoHooks; readonly stored: Stored } {
  const stored: Stored = {
    session: undefined,
    input: undefined,
    rule: undefined,
    reply: undefined,
    markdown: undefined,
  };
  return {
    stored,
    hooks: {
      onSessionStart: (handler) => {
        stored.session = handler;
      },
      onInput: (handler) => {
        stored.input = handler;
      },
      onBeforeAgent: (handler) => {
        stored.rule = handler;
      },
      onMessageEnd: (handler) => {
        stored.reply = handler;
      },
      onMarkdown: (transformer) => {
        stored.markdown = transformer;
      },
    },
  };
}

test("the hooks translate prompts and leave a stream in English", async () => {
  const captured = capture();
  const sink: UiSink = {
    notify: () => undefined,
    setStatus: () => undefined,
    setWidget: () => undefined,
  };
  bindPlamo(
    captured.hooks,
    createRuntime({ replyLanguage: "ja", promptLanguage: "en", port: 18555 }, {
      complete: async () => "Add a tab",
      ensure: async () => true,
    } satisfies Server),
  );
  if (captured.stored.session === undefined || captured.stored.input === undefined) {
    throw new Error("hooks were not registered");
  }
  captured.stored.session({ ui: sink });
  expect(
    await captured.stored.input(
      { type: "input", text: "タブを追加して", source: "interactive" },
      { ui: sink },
    ),
  ).toStrictEqual({ action: "transform", text: "Add a tab" });
  const sections: Record<string, string> = {};
  captured.stored.rule?.(sections);
  expect(sections["plamo-translate"]).toBe(
    "The user writes in Japanese. Their messages are machine-translated into English before you read them, and your replies are machine-translated into `ja` for display only. Read and write English. Only inline code and fenced code blocks are shown exactly as written. Put anything the user must read or copy verbatim in one of them: commands, file paths, identifiers, option and flag names, literal values, error messages and quoted UI text. Do not wrap ordinary prose.",
  );
  captured.stored.reply?.({ role: "user" });
  expect(
    captured.stored.markdown?.("Add a tab", {
      messageType: "assistant",
      isStreaming: true,
      availableWidth: 20,
    }),
  ).toBe("Add a tab");
});
