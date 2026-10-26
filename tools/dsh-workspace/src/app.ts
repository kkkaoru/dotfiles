/** Runs with bun. Orchestrates one dsh-workspace invocation. */

import { helpText, parseArguments } from "./args.ts";
import { createWorkspace } from "./host.ts";
import { findWorkspace } from "./registry.ts";
import type {
  CreateWorkspaceResult,
  Effects,
  Options,
  RunInput,
  RunResult,
  WorkspaceRecord,
} from "./types.ts";

const REGISTRY_RELATIVE_PATH = "/storages/workspace.json";
const SUCCESS_EXIT_CODE = 0;
const FAILURE_EXIT_CODE = 1;
const USAGE_EXIT_CODE = 2;

interface OutcomeInput {
  readonly outcome: CreateWorkspaceResult;
  readonly directory: string;
}

interface RunContext {
  readonly effects: Effects;
  readonly options: Options;
  readonly directory: string;
}

function failure(message: string, exitCode: number): RunResult {
  return { exitCode, stdout: "", stderr: `dsh-workspace: ${message}` };
}

function report(state: string, workspace: WorkspaceRecord): RunResult {
  return {
    exitCode: SUCCESS_EXIT_CODE,
    stdout: `${state} ${workspace.workspaceId} ${workspace.path}`,
    stderr: "",
  };
}

function renderOutcome(input: OutcomeInput): RunResult {
  const { outcome, directory } = input;
  if (outcome.kind === "error") {
    return failure(outcome.message, FAILURE_EXIT_CODE);
  }
  if (outcome.workspace.path !== directory) {
    return failure(
      `Host registered ${outcome.workspace.path} instead of ${directory}`,
      FAILURE_EXIT_CODE,
    );
  }
  return report(outcome.kind, outcome.workspace);
}

function registryPath(effects: Effects): string {
  return `${effects.harnessHome()}${REGISTRY_RELATIVE_PATH}`;
}

function cachedWorkspace(context: RunContext): WorkspaceRecord | undefined {
  const text = context.effects.readText(registryPath(context.effects)) ?? "";
  return findWorkspace(text, context.directory);
}

async function register(context: RunContext): Promise<RunResult> {
  const outcome = await createWorkspace({
    effects: context.effects,
    directory: context.directory,
    title: context.options.title,
  });
  return renderOutcome({ outcome, directory: context.directory });
}

async function resolve(context: RunContext): Promise<RunResult> {
  if (context.effects.directoryKind(context.directory) !== "directory") {
    return failure(`not a directory: ${context.options.directory}`, FAILURE_EXIT_CODE);
  }
  const known = cachedWorkspace(context);
  return known === undefined ? register(context) : report("exists", known);
}

/**
 * Run the command for one invocation.
 * @param input - argv, cwd, and the effects to perform.
 * @returns the exit code and the streams to write.
 */
export async function run(input: RunInput): Promise<RunResult> {
  const parsed = parseArguments(input.argv, input.cwd);
  if (parsed.kind === "help") {
    return { exitCode: SUCCESS_EXIT_CODE, stdout: helpText(), stderr: "" };
  }
  if (parsed.kind === "usage-error") {
    return failure(parsed.message, USAGE_EXIT_CODE);
  }
  const directory = input.effects.canonicalDirectory(parsed.options.directory);
  return resolve({ effects: input.effects, options: parsed.options, directory });
}
