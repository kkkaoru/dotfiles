// Runs with Bun.
export interface CreateCommand {
  readonly kind: "create";
  readonly objective: string;
  readonly tokenBudget: number | null;
}
export type GoalCommand =
  | CreateCommand
  | { readonly kind: "status" | "pause" | "resume" | "clear" }
  | { readonly kind: "edit"; readonly objective: string | null }
  | { readonly kind: "budget"; readonly tokens: number | null };

const CONTROL_COMMANDS: ReadonlyMap<string, GoalCommand> = new Map([
  ["", { kind: "status" }],
  ["status", { kind: "status" }],
  ["pause", { kind: "pause" }],
  ["resume", { kind: "resume" }],
  ["clear", { kind: "clear" }],
  ["edit", { kind: "edit", objective: null }],
]);
const BUDGET_PATTERN: RegExp = /^budget\s+(\S+)$/;
const CREATE_PATTERN: RegExp = /^--tokens\s+(\S+)\s+([\s\S]+)$/;
const EDIT_PATTERN: RegExp = /^edit\s+([\s\S]+)$/;
const POSITIVE_INTEGER: RegExp = /^[1-9]\d*$/;
export const GOAL_USAGE: string =
  "/goal [objective|--tokens N objective|status|pause|resume|clear|edit [objective]|budget N|budget none]";

export function parseBudget(text: string): number {
  const tokens: number = Number(text);
  if (!POSITIVE_INTEGER.test(text) || !Number.isSafeInteger(tokens)) {
    throw new Error("Token budget must be a positive safe integer.");
  }
  return tokens;
}

export function parseGoalCommand(args: string): GoalCommand {
  const text: string = args.trim();
  const control: GoalCommand | undefined = CONTROL_COMMANDS.get(text);
  if (control !== undefined) return control;
  const budget: RegExpExecArray | null = BUDGET_PATTERN.exec(text);
  if (budget?.[1] !== undefined) {
    return {
      kind: "budget",
      tokens: budget[1] === "none" ? null : parseBudget(budget[1]),
    };
  }
  const edit: RegExpExecArray | null = EDIT_PATTERN.exec(text);
  if (edit?.[1] !== undefined) return { kind: "edit", objective: edit[1] };
  const create: RegExpExecArray | null = CREATE_PATTERN.exec(text);
  if (create?.[1] !== undefined && create[2] !== undefined) {
    return {
      kind: "create",
      objective: create[2],
      tokenBudget: parseBudget(create[1]),
    };
  }
  if (text.startsWith("--") || text === "budget") throw new Error(GOAL_USAGE);
  return { kind: "create", objective: text, tokenBudget: null };
}
