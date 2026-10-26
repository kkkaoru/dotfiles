/** Runs with bun. Reads the durable workspace registry document. */

import type { WorkspaceRecord } from "./types.ts";

const TABLES_FIELD = "tables";
const WORKSPACES_FIELD = "workspaces";
const PATH_FIELD = "path";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function readWorkspaces(registryText: string): Record<string, unknown> | undefined {
  const document = parseJson(registryText);
  if (!isRecord(document)) {
    return undefined;
  }
  const tables = document[TABLES_FIELD];
  if (!isRecord(tables)) {
    return undefined;
  }
  const workspaces = tables[WORKSPACES_FIELD];
  return isRecord(workspaces) ? workspaces : undefined;
}

function matchingIds(workspaces: Record<string, unknown>, path: string): string[] {
  return Object.entries(workspaces)
    .filter(([, entry]) => isRecord(entry) && entry[PATH_FIELD] === path)
    .map(([workspaceId]) => workspaceId);
}

/**
 * Find the workspace already registered for a directory.
 * @param registryText - the `storages/workspace.json` contents.
 * @param path - canonical directory path to look for.
 * @returns the record, or undefined when the document is unreadable or holds no match.
 */
export function findWorkspace(registryText: string, path: string): WorkspaceRecord | undefined {
  const workspaces = readWorkspaces(registryText);
  if (workspaces === undefined) {
    return undefined;
  }
  const workspaceId = matchingIds(workspaces, path).at(0);
  return workspaceId === undefined ? undefined : { workspaceId, path };
}
