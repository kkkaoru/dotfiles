// This file runs with Bun.
import { expect, test } from "vitest";
import { buildCursorMessage } from "./context.ts";

test("replays native system sections without treating them as tool results", () => {
  expect(
    buildCursorMessage({
      messages: [
        { role: "system", content: "Base", sections: { policy: "Old" }, timestamp: 1 },
        { role: "user", content: "hello", timestamp: 2 },
        { role: "system", content: "Update", sections: { policy: "New" }, timestamp: 3 },
      ],
    }),
  ).toStrictEqual({
    text: "SYSTEM INSTRUCTIONS:\nBase\n\nUpdate\n\nNew\n\nUSER:\nhello\n\nContinue from the transcript above. Follow the latest user request.",
  });
});
