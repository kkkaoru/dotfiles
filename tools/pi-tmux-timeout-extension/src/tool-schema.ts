// This TypeScript file is executed with Bun.
import { Type, type TSchema } from "typebox";

export const tmuxExecSchema = Type.Object({
  command: Type.String({
    description: "Long-running shell command to start in detached tmux",
    minLength: 1,
  }),
  estimatedDurationSeconds: Type.Optional(
    Type.Integer({
      description: "Estimated duration in seconds for expected completion time",
      maximum: 604_800,
      minimum: 1,
    }),
  ),
}) satisfies TSchema;
