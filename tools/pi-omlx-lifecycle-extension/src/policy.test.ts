import { expect, it } from "vitest";
import {
  BONSAI_PROVIDER_ID,
  decideModelSwitchAction,
  decideShutdownAction,
  OMLX_PROVIDER_ID,
  usesLocalOmlx,
} from "./policy.ts";

it("exposes the omlx provider id used by .pi/agent/models.json", () => {
  expect(OMLX_PROVIDER_ID).toBe("omlx");
});

it("starts omlx when switching from another provider into omlx", () => {
  expect(decideModelSwitchAction({ nextProvider: "omlx", previousProvider: "anthropic" })).toBe(
    "start",
  );
});

it("starts omlx when switching into omlx from no prior model", () => {
  expect(decideModelSwitchAction({ nextProvider: "omlx", previousProvider: undefined })).toBe(
    "start",
  );
});

it("stops omlx when switching away from omlx to another provider", () => {
  expect(decideModelSwitchAction({ nextProvider: "anthropic", previousProvider: "omlx" })).toBe(
    "stop",
  );
});

it("does nothing when switching between two non-omlx providers", () => {
  expect(decideModelSwitchAction({ nextProvider: "anthropic", previousProvider: "cursor" })).toBe(
    "none",
  );
});

it("re-runs the (idempotent) start check when staying on omlx", () => {
  expect(decideModelSwitchAction({ nextProvider: "omlx", previousProvider: "omlx" })).toBe("start");
});

it("does nothing on shutdown when the current provider is not omlx", () => {
  expect(decideShutdownAction("anthropic")).toBe("none");
});

it("does nothing on shutdown when there is no current provider", () => {
  expect(decideShutdownAction(undefined)).toBe("none");
});

it("stops omlx on shutdown when the current provider is omlx", () => {
  expect(decideShutdownAction("omlx")).toBe("stop");
});

it("treats local bonsai as the same oMLX process", () => {
  expect(OMLX_PROVIDER_ID).toBe("omlx");
  expect(BONSAI_PROVIDER_ID).toBe("bonsai");
  expect(usesLocalOmlx("bonsai")).toBe(true);
  expect(usesLocalOmlx("bonsai-mac")).toBe(false);
  expect(decideModelSwitchAction({ nextProvider: "bonsai", previousProvider: "anthropic" })).toBe(
    "start",
  );
  expect(decideModelSwitchAction({ nextProvider: "bonsai", previousProvider: "omlx" })).toBe(
    "start",
  );
  expect(decideModelSwitchAction({ nextProvider: "strata", previousProvider: "bonsai" })).toBe(
    "stop",
  );
  expect(decideShutdownAction("bonsai")).toBe("stop");
});
