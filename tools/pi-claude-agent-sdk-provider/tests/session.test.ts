// This file runs with Bun.
import { expect, it } from "vitest";
import { childEnv, ClaudeSession, sessionOptions } from "../src/session.ts";
import { PendingToolCalls } from "../src/tool-bridge.ts";
import { fakeStartQuery } from "./fake-query.ts";

it("drops credentials that would move the child off the Claude plan", () => {
  expect(
    childEnv({
      ANTHROPIC_API_KEY: "key",
      ANTHROPIC_AUTH_TOKEN: "token",
      ANTHROPIC_BASE_URL: "http://127.0.0.1:8318",
      HOME: "/home/me",
    }),
  ).toStrictEqual({ HOME: "/home/me" });
});

it("omits effort when thinking is off", () => {
  const options = sessionOptions(
    { cwd: "/w", effort: undefined, model: "m", systemPrompt: "s", thinking: false, tools: [] },
    new PendingToolCalls(),
  );
  expect(options.effort).toBe(undefined);
  expect(options.strictMcpConfig).toBe(true);
  expect(options.persistSession).toBe(false);
  expect(options.includePartialMessages).toBe(true);
});

it("pumps SDK failures into the event queue and closes once", async () => {
  const { start, fakes } = fakeStartQuery(() => undefined);
  const session = new ClaudeSession(
    { cwd: "/w", effort: "high", model: "m", systemPrompt: "s", thinking: true, tools: [] },
    start,
  );
  const waiting = session.pending.expect("toolu_1");
  fakes[0]?.throwError(new Error("child crashed"));
  const event = await session.events.next();
  expect(event.value?.type).toBe("pump_failure");
  await session.setEffort("max");
  expect(fakes[0]?.effortLevels).toStrictEqual(["max"]);
  expect(session.effort).toBe("max");
  session.close("done");
  session.close("again");
  expect(session.closed).toBe(true);
  expect(fakes[0]?.closed).toBe(true);
  await expect(waiting).rejects.toThrow("done");
});

it("ends the event queue when the SDK stream finishes", async () => {
  const { start, fakes } = fakeStartQuery(() => undefined);
  const session = new ClaudeSession(
    { cwd: "/w", effort: undefined, model: "m", systemPrompt: "s", thinking: false, tools: [] },
    start,
  );
  fakes[0]?.finish();
  expect(await session.events.next()).toStrictEqual({ done: true, value: undefined });
});
