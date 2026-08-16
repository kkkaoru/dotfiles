import { randomUUID } from "node:crypto";
import { mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { JsonlLocalAgentStore, type LocalAgentStore } from "@cursor/sdk";

const STORE_CACHE_DIR = ".cache";
const STORE_PACKAGE_DIR = "pi-my-cursor-provider";
const STORE_AGENTS_DIR = "agents";
const PROCESS_AGENT_PREFIX = "pi-cursor";

interface ProcessStoreState {
  current: { readonly root: string; readonly store: LocalAgentStore } | undefined;
}

const processStoreState: ProcessStoreState = { current: undefined };

export function cursorStoreRoot(pid = process.pid): string {
  return join(homedir(), STORE_CACHE_DIR, STORE_PACKAGE_DIR, STORE_AGENTS_DIR, String(pid));
}

export function cursorProcessAgentId(pid = process.pid, requestId: string = randomUUID()): string {
  return `${PROCESS_AGENT_PREFIX}-${pid}-${requestId}`;
}

export function createProcessCursorStore(pid = process.pid): LocalAgentStore {
  const root = cursorStoreRoot(pid);
  mkdirSync(root, { recursive: true });
  const store = new JsonlLocalAgentStore(root);
  processStoreState.current = { root, store };
  return store;
}

export function getProcessCursorStore(): LocalAgentStore {
  return processStoreState.current?.store ?? createProcessCursorStore();
}

export const cursorStoreTestApi = {
  processStoreRoot: (): string | undefined => processStoreState.current?.root,
  reset(): void {
    processStoreState.current = undefined;
  },
};
