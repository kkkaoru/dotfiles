/** Runs with bun. Argument parsing for dsh-workspace, kept free of I/O. */

import type { ParseResult } from "./types.ts";

const HELP_TEXT = `usage: dsh-workspace [directory] [--title <name>]

Registers a DeepSeek Harness workspace for <directory> (default: the current
directory) and prints its workspace id. Running it again for the same
directory is a no-op that prints the existing id.`;

const TITLE_FLAG = "-t";
const TITLE_LONG_FLAG = "--title";
const TITLE_ASSIGNMENT = "--title=";
const OPTION_SEPARATOR = "--";
const HELP_FLAG = "-h";
const HELP_LONG_FLAG = "--help";
const SINGLE_DASH = "-";
const ONE_ARGUMENT = 1;
const TWO_ARGUMENTS = 2;
const SINGLE_DIRECTORY_MESSAGE = "one directory at a time";

interface ScanState {
  readonly directory: string | undefined;
  readonly title: string | undefined;
}

type ScanOutcome = ScanState | Exclude<ParseResult, { readonly kind: "options" }>;

type TerminalResult = Exclude<ParseResult, { readonly kind: "options" }>;

type ArgumentKind = "help" | "title" | "title-assignment" | "separator" | "unknown" | "positional";

interface Application {
  readonly state: ScanState;
  readonly consumed: number;
  readonly finished: TerminalResult | undefined;
}

const EMPTY_STATE: ScanState = { directory: undefined, title: undefined };

function usageError(message: string): TerminalResult {
  return { kind: "usage-error", message };
}

function classify(argument: string): ArgumentKind {
  if (argument === HELP_FLAG || argument === HELP_LONG_FLAG) {
    return "help";
  }
  if (argument === TITLE_FLAG || argument === TITLE_LONG_FLAG) {
    return "title";
  }
  if (argument.startsWith(TITLE_ASSIGNMENT)) {
    return "title-assignment";
  }
  if (argument === OPTION_SEPARATOR) {
    return "separator";
  }
  return argument.startsWith("-") && argument !== SINGLE_DASH ? "unknown" : "positional";
}

function keep(state: ScanState, consumed: number): Application {
  return { state, consumed, finished: undefined };
}

function stop(state: ScanState, message: string): Application {
  return { state, consumed: ONE_ARGUMENT, finished: usageError(message) };
}

function takeTitle(state: ScanState, next: string | undefined, argument: string): Application {
  return next === undefined
    ? stop(state, `${argument} needs a value`)
    : keep({ ...state, title: next }, TWO_ARGUMENTS);
}

function takeDirectory(state: ScanState, argument: string): Application {
  return state.directory === undefined
    ? keep({ ...state, directory: argument }, ONE_ARGUMENT)
    : stop(state, SINGLE_DIRECTORY_MESSAGE);
}

function applyKind(
  kind: ArgumentKind,
  state: ScanState,
  argument: string,
  next: string | undefined,
): Application {
  if (kind === "help") {
    return { state, consumed: ONE_ARGUMENT, finished: { kind: "help" } };
  }
  if (kind === "separator") {
    return keep(state, ONE_ARGUMENT);
  }
  if (kind === "unknown") {
    return stop(state, `unknown option: ${argument}`);
  }
  if (kind === "title") {
    return takeTitle(state, next, argument);
  }
  return kind === "title-assignment"
    ? keep({ ...state, title: argument.slice(TITLE_ASSIGNMENT.length) }, ONE_ARGUMENT)
    : takeDirectory(state, argument);
}

/**
 * Apply one scanned argument to the fold carry.
 * @param carry - state before this argument.
 * @param argv - the full argument list, read for a flag's value.
 * @param index - position of this argument.
 * @returns the next carry; a terminal result makes later arguments no-ops.
 */
function scan(argv: readonly string[], index: number, state: ScanState): ScanOutcome {
  const argument = argv[index];
  if (argument === undefined) {
    return state;
  }
  const applied = applyKind(classify(argument), state, argument, argv[index + ONE_ARGUMENT]);
  if (applied.finished !== undefined) {
    return applied.finished;
  }
  return scan(argv, index + applied.consumed, applied.state);
}

/** Last path segment, without node:path, so argument parsing stays pure. */
export function lastSegment(path: string): string {
  const segments = path.split("/").filter((segment) => segment !== "");
  return segments.at(-1) ?? path;
}

/** Usage text printed by `--help`. */
export function helpText(): string {
  return HELP_TEXT;
}

/**
 * Parse the command line into options, a help request, or a usage error.
 * @param argv - arguments after the program name.
 * @param cwd - directory used when no positional directory is given.
 * @returns the parsed outcome.
 */
export function parseArguments(argv: readonly string[], cwd: string): ParseResult {
  const outcome = scan(argv, 0, EMPTY_STATE);
  if (!("directory" in outcome)) {
    return outcome;
  }
  const directory = outcome.directory ?? cwd;
  return {
    kind: "options",
    options: { directory, title: outcome.title ?? lastSegment(directory) },
  };
}
