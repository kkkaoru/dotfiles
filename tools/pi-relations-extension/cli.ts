#!/usr/bin/env bun
// This TypeScript file is executed with Bun.
import { openRelations } from "./src/sidecar.ts";
import { ACTIONS, answer, apply, type Params } from "./src/tool.ts";

const USAGE = `usage: cli.ts [--session <file>] add <id> [--label <text>]
       cli.ts [--session <file>] link <from> <relation> <to> [--note <text>]
       cli.ts [--session <file>] unlink <from> <relation> <to>
       cli.ts [--session <file>] show <id>
       cli.ts [--session <file>] list

--session defaults to $PI_SESSION_FILE, so a shell inside a pi session writes that session's graph.`;

interface Args {
  session: string | undefined;
  label: string | undefined;
  note: string | undefined;
  action: string | undefined;
  positional: string[];
}

const FLAGS = { "--session": "session", "--label": "label", "--note": "note" } as const;
type Flag = keyof typeof FLAGS;

function isFlag(value: string): value is Flag {
  return value in FLAGS;
}

function split(argv: readonly string[]): Args {
  const args: Args = {
    session: undefined,
    label: undefined,
    note: undefined,
    action: undefined,
    positional: [],
  };
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (item === undefined) {
      break;
    }
    if (isFlag(item)) {
      const value = argv[index + 1];
      if (value === undefined) {
        throw new Error(`${item} needs a value.`);
      }
      args[FLAGS[item]] = value;
      index += 1;
    } else if (args.action === undefined) {
      args.action = item;
    } else {
      args.positional.push(item);
    }
  }
  return args;
}

function require(value: string | undefined, name: string): string {
  if (value === undefined || value.trim() === "") {
    throw new Error(`${name} is required.\n\n${USAGE}`);
  }
  return value;
}

function paramsOf(args: Args): Params {
  const [first, second, third] = args.positional;
  const action = ACTIONS.find((candidate) => candidate === args.action);
  if (action === undefined) {
    throw new Error(`unknown action ${String(args.action)}\n\n${USAGE}`);
  }
  const label = args.label === undefined ? {} : { label: args.label };
  const note = args.note === undefined ? {} : { note: args.note };
  switch (action) {
    case "add": {
      return { action, id: require(first, "id"), ...label };
    }
    case "link": {
      return {
        action,
        from: require(first, "from"),
        relation: require(second, "relation"),
        to: require(third, "to"),
        ...note,
      };
    }
    case "unlink": {
      return {
        action,
        from: require(first, "from"),
        relation: require(second, "relation"),
        to: require(third, "to"),
      };
    }
    case "show": {
      return { action, id: require(first, "id") };
    }
    case "list": {
      return { action };
    }
    default: {
      throw new Error(`unknown action ${String(action)}`);
    }
  }
}

async function main(): Promise<void> {
  const args = split(process.argv.slice(2));
  if (args.action === undefined || args.action === "--help" || args.action === "-h") {
    console.log(USAGE);
    process.exitCode = args.action === undefined ? 2 : 0;
    return;
  }
  const params = paramsOf(args);
  const opened = await openRelations(args.session ?? process.env.PI_SESSION_FILE, (error) => {
    console.error(`relations: ${String(error)}`);
  });
  try {
    const next = apply(opened.graph, params);
    if (next !== opened.graph) {
      opened.store.save(next);
      await opened.store.flush();
    }
    console.log(answer(next, params));
  } finally {
    await opened.close();
  }
}

await main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
