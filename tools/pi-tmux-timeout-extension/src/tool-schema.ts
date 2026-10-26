// This TypeScript file is executed with Bun.
import { Type, type TSchema } from "typebox";

export const tmuxCheckInSchema = Type.Object({
  sessionName: Type.String({
    description: "Exact tmux session name from the overdue check-in",
    minLength: 1,
  }),
  nextCheckSeconds: Type.Integer({
    description:
      "Seconds until the next overdue check-in; reschedules the job's estimate without stopping it",
    maximum: 604_800,
    minimum: 60,
  }),
}) satisfies TSchema;

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
