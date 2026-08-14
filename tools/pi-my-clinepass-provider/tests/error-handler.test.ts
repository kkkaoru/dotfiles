import process from "node:process";
import { describe, expect, it, vi } from "vitest";
import { PROVIDER_NAME } from "../src/env.ts";
import { handleClinePassError } from "../src/error-handler.ts";
import { CLINEPASS_ERROR_MESSAGES } from "../src/errors.ts";

interface Notification {
  msg: string;
  type: string;
}

function runError(errorMessage: string, provider?: string): Notification[] {
  const notifications: Notification[] = [];
  handleClinePassError(
    {
      message: {
        errorMessage,
        ...(provider === undefined ? {} : { provider }),
        stopReason: "error",
      },
    },
    {
      hasUI: true,
      model: { provider: PROVIDER_NAME },
      ui: { notify: (msg, type) => notifications.push({ msg, type }) },
    },
  );
  return notifications;
}

describe("ClinePass error handling", () => {
  it("classifies authentication, subscription, rate-limit, and unknown errors", () => {
    expect(runError("401 Unauthorized", PROVIDER_NAME)).toStrictEqual([
      { msg: CLINEPASS_ERROR_MESSAGES.auth_expired, type: "error" },
    ]);
    expect(runError("403 Forbidden")).toStrictEqual([
      { msg: CLINEPASS_ERROR_MESSAGES.not_subscribed, type: "error" },
    ]);
    expect(runError("429 Too Many Requests", PROVIDER_NAME)).toStrictEqual([
      { msg: CLINEPASS_ERROR_MESSAGES.rate_limited, type: "error" },
    ]);
    expect(runError("Internal server error", PROVIDER_NAME)).toStrictEqual([
      { msg: CLINEPASS_ERROR_MESSAGES.unknown, type: "error" },
    ]);
  });

  it("ignores unrelated or successful messages", () => {
    expect(runError("403 Forbidden", "openai")).toStrictEqual([]);
    const notifications: Notification[] = [];
    handleClinePassError(
      { message: { provider: PROVIDER_NAME, stopReason: "stop" } },
      {
        hasUI: true,
        ui: { notify: (msg, type) => notifications.push({ msg, type }) },
      },
    );
    handleClinePassError(
      { message: "invalid" },
      { hasUI: true, ui: { notify: (msg, type) => notifications.push({ msg, type }) } },
    );
    expect(notifications).toStrictEqual([]);
  });

  it("writes errors to stderr without a UI", () => {
    const errorSpy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    handleClinePassError(
      {
        message: {
          errorMessage: "403 Forbidden",
          provider: PROVIDER_NAME,
          stopReason: "error",
        },
      },
      { hasUI: false, ui: { notify: () => undefined } },
    );
    expect(errorSpy).toHaveBeenCalledWith(
      `[clinepass] ${CLINEPASS_ERROR_MESSAGES.not_subscribed}\n`,
    );
  });
});
