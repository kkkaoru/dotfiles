// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import { CompletionDelivery, type CompletionDeliveryContext } from "./delivery.ts";
import { createTmuxLaunch } from "./tmux.ts";

it.each([
  null,
  "invalid",
  {},
  { toolName: "bash" },
  { toolName: "read" },
  { toolName: "read", isError: true },
  { toolName: "read", isError: false },
  { toolName: "read", isError: false, input: null },
  { toolName: "read", isError: false, input: "invalid" },
  { toolName: "read", isError: false, input: {} },
  { toolName: "read", isError: false, input: { path: 1 } },
  { toolName: "read", isError: false, input: { path: "/unrelated/output.log" } },
])("does not acknowledge an overdue job from an unrelated or malformed result: %j", (event) => {
  const delivery = new CompletionDelivery({ sendUserMessage: vi.fn() });
  delivery.setContext({ isIdle: () => false, ui: { notify: vi.fn(), setStatus: vi.fn() } });
  delivery.overdue([createTmuxLaunch({ command: "review", id: 1, namespace: "a".repeat(32) })]);
  delivery.inspectedLog(event);
  expect(delivery.hasPending()).toBe(true);
});

it("requires a successful log read, not a failed read or status-only inspection", () => {
  const delivery = new CompletionDelivery({ sendUserMessage: vi.fn() });
  const launch = createTmuxLaunch({ command: "review", id: 1, namespace: "a".repeat(32) });
  delivery.setContext({ isIdle: () => false, ui: { notify: vi.fn(), setStatus: vi.fn() } });
  delivery.overdue([launch]);
  delivery.inspectedLog({ toolName: "read", isError: true, input: { path: launch.logPath } });
  expect(delivery.hasPending()).toBe(true);
  delivery.inspectedLog({ toolName: "read", isError: false, input: { path: launch.statusPath } });
  expect(delivery.hasPending()).toBe(true);
  delivery.inspectedLog({
    toolName: "read",
    isError: false,
    input: { path: `@${launch.logPath}` },
  });
  expect(delivery.hasPending()).toBe(false);
  delivery.overdue([launch]);
  expect(delivery.hasPending()).toBe(true);
});

it("still wakes an idle agent after a context notice was ignored or its provider call failed", () => {
  const sendUserMessage = vi.fn();
  const delivery = new CompletionDelivery({ sendUserMessage });
  const context: CompletionDeliveryContext = {
    isIdle: () => false,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  delivery.setContext(context);
  delivery.overdue([createTmuxLaunch({ command: "review", id: 1, namespace: "a".repeat(32) })]);
  expect(delivery.injectOverdue({ messages: [] })?.messages).toHaveLength(1);
  expect(delivery.injectOverdue({ messages: [] })?.messages).toHaveLength(1);
  delivery.agentSettled({ ...context, isIdle: () => true });
  expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith(
    expect.stringMatching(/^tmux overdue check-in: 1 task/u),
    { deliverAs: "followUp" },
  );
  delivery.agentSettled({ ...context, isIdle: () => true });
  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(delivery.hasPending()).toBe(false);
});

it("rotates the twenty-first job into the next request without dropping earlier notices", () => {
  const delivery = new CompletionDelivery({ sendUserMessage: vi.fn() });
  delivery.setContext({ isIdle: () => false, ui: { notify: vi.fn(), setStatus: vi.fn() } });
  delivery.overdue(
    Array.from({ length: 21 }, (_value, id) =>
      createTmuxLaunch({ command: `review ${id}`, id, namespace: "a".repeat(32) }),
    ),
  );
  expect(delivery.injectOverdue({ messages: [] })?.messages).toStrictEqual([
    expect.objectContaining({ content: expect.not.stringMatching(/task: review 20\n/u) }),
  ]);
  expect(delivery.injectOverdue({ messages: [] })?.messages).toStrictEqual([
    expect.objectContaining({ content: expect.stringMatching(/task: review 20\n/u) }),
  ]);
  expect(delivery.hasPending()).toBe(true);
});
