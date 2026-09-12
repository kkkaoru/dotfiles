// This TypeScript file is executed with Bun.
import { Type, type TSchema } from "typebox";

export const tmuxExecSchema = Type.Object({
  command: Type.String({
    description: "Long-running shell command to start in detached tmux",
    minLength: 1,
  }),
  estimatedDurationSeconds: Type.Optional(
    Type.Integer({
      description: "Expected duration; exceeding it triggers a check-in, not termination",
      maximum: 604_800,
      minimum: 1,
    }),
  ),
  timeoutSeconds: Type.Optional(
    Type.Integer({
      description:
        "Hard runtime limit; terminate the command process group on expiry (requires GNU timeout)",
      maximum: 604_800,
      minimum: 1,
    }),
  ),
}) satisfies TSchema;
