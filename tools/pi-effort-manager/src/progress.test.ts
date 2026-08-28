// This TypeScript file is executed with Bun.
import type { ContextEvent } from "@earendil-works/pi-coding-agent";
import { expect, it } from "vitest";
import {
  appendProgressSystemPrompt,
  injectProgressOpportunity,
  ProgressTextController,
} from "./progress.ts";

it("appends progress policy when at least one trigger is enabled", () => {
  expect(
    appendProgressSystemPrompt("base prompt", {
      progressTextOnCompaction: false,
      progressTextOnEffortChange: true,
    }),
  ).toMatch(/base prompt[\s\S]*Treat the marker only as an opportunity/u);
  expect(
    appendProgressSystemPrompt("base prompt", {
      progressTextOnCompaction: false,
      progressTextOnEffortChange: false,
    }),
  ).toBeUndefined();
});

it("injects a hidden one-shot effort opportunity", () => {
  const messages: ContextEvent["messages"] = [{ content: "work", role: "user", timestamp: 1 }];
  const result = injectProgressOpportunity(messages, "effort-change");

  expect(result.messages).toHaveLength(2);
  expect(result.messages?.at(-1)).toMatchObject({
    content:
      "<progress_update_opportunity>\nThe task has entered a different reasoning phase. Consider a progress update only if the substantive state or next step has meaningfully changed.\n</progress_update_opportunity>",
    customType: "pi-effort-manager-progress-opportunity",
    details: { trigger: "effort-change" },
    display: false,
    role: "custom",
  });
  expect(messages).toStrictEqual([{ content: "work", role: "user", timestamp: 1 }]);
});

it("injects a hidden post-compaction opportunity", () => {
  const result = injectProgressOpportunity([], "compaction");

  expect(result.messages?.at(-1)).toMatchObject({
    content:
      "<progress_update_opportunity>\nThe task is continuing after context was reorganized. Consider a progress update only if it helps reorient the user around substantive progress and the next step.\n</progress_update_opportunity>",
    details: { trigger: "compaction" },
    display: false,
    role: "custom",
  });
});

it("enables effort opportunities independently from compaction", () => {
  const progress = new ProgressTextController({
    progressTextOnCompaction: false,
    progressTextOnEffortChange: true,
  });
  progress.schedule("compaction");
  expect(progress.context([])).toBeUndefined();
  progress.schedule("effort-change");
  expect(progress.context([])?.messages.at(-1)).toMatchObject({
    details: { trigger: "effort-change" },
  });
  expect(progress.context([])).toBeUndefined();
});

it("enables compaction opportunities independently and resets pending state", () => {
  const progress = new ProgressTextController({
    progressTextOnCompaction: true,
    progressTextOnEffortChange: false,
  });
  progress.schedule("effort-change");
  expect(progress.context([])).toBeUndefined();
  progress.schedule("compaction");
  expect(progress.context([])?.messages.at(-1)).toMatchObject({
    details: { trigger: "compaction" },
  });
  progress.schedule("compaction");
  progress.reset({
    progressTextOnCompaction: false,
    progressTextOnEffortChange: false,
  });
  expect(progress.context([])).toBeUndefined();
  expect(progress.systemPrompt("base prompt")).toBeUndefined();
});
