// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { tmuxExecSchema } from "./tool-schema.ts";

it("exposes separate optional estimate and hard timeout with bounded positive seconds", () => {
  expect(tmuxExecSchema.required).toStrictEqual(["command"]);
  expect(tmuxExecSchema.properties.estimatedDurationSeconds).toMatchObject({
    type: "integer",
    minimum: 1,
    maximum: 604_800,
  });
  expect(tmuxExecSchema.properties.timeoutSeconds).toMatchObject({
    type: "integer",
    minimum: 1,
    maximum: 604_800,
  });
  expect(tmuxExecSchema.properties.estimatedDurationSeconds).toMatchObject({
    description: expect.stringMatching(/not termination/u),
  });
  expect(tmuxExecSchema.properties.timeoutSeconds).toMatchObject({
    description: expect.stringMatching(/terminate the command process group/u),
  });
});
