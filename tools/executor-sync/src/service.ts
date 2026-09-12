// Runs with Bun. Only the explicitly configured per-user Executor service is restarted.

import { rm } from "node:fs/promises";
import { userInfo } from "node:os";
import { z } from "zod";
import type { Adapter } from "./adapter";
import { atomicWrite, command, readOptional, requiredCommand } from "./io";
import { check, type Snapshot } from "./model";
import type { Storage } from "./storage";

export interface ServiceOptions {
  home: string;
  repo: string;
  dataDir: string;
  stateDir: string;
  adapter: Adapter;
  storage: Pick<Storage, "envelope" | "encrypt">;
}
const manifestSchema = z.object({
  scopeDir: z.string(),
  connection: z.object({
    origin: z.string(),
    auth: z.object({ kind: z.literal("bearer"), token: z.string().min(1) }),
  }),
});
const queueSchema = z.array(
  z.object({
    owner: z.enum(["org", "user"]),
    integration: z.string(),
    name: z.string(),
  }),
);
export class Service {
  readonly options: ServiceOptions;
  constructor(options: ServiceOptions) {
    this.options = options;
  }
  async version(): Promise<void> {
    const output: Buffer = await requiredCommand({
      executable: `${this.options.repo}/scripts/executor`,
      args: ["--version"],
      input: "",
    });
    check(
      output.toString().trim() === "executor v1.6.8",
      "Unsupported Executor version; sync disabled",
    );
  }
  async recover(): Promise<void> {
    const pending: string | null = await readOptional(
      `${this.options.stateDir}/restart-needed`,
    );
    if (pending === null) return;
    const domain: string = `gui/${userInfo().uid}`;
    const running = await command({
      executable: "/bin/launchctl",
      args: ["print", `${domain}/sh.executor.daemon`],
      input: "",
      identity: null,
    });
    if (running.code !== 0)
      await requiredCommand({
        executable: "/bin/launchctl",
        args: [
          "bootstrap",
          domain,
          `${this.options.home}/Library/LaunchAgents/sh.executor.daemon.plist`,
        ],
        input: "",
      });
    await rm(`${this.options.stateDir}/restart-needed`, { force: true });
  }
  async import(snapshot: Snapshot): Promise<void> {
    await this.version();
    const queue = queueSchema.safeParse(
      (snapshot.tables.connection ?? []).map((row) => ({
        owner: row.owner,
        integration: row.integration,
        name: row.name,
      })),
    );
    check(queue.success, "Invalid connection rebuild queue");
    const currentManifest: string | null = await readOptional(
      `${this.options.dataDir}/server-control/server.json`,
    );
    check(currentManifest, "Executor must be running before settings import");
    const current = manifestSchema.safeParse(JSON.parse(currentManifest));
    check(
      current.success && current.data.scopeDir === this.options.repo,
      "Executor server scope mismatch; no service stopped",
    );
    const plist: string = `${this.options.home}/Library/LaunchAgents/sh.executor.daemon.plist`;
    const service: string = `gui/${userInfo().uid}`;
    await atomicWrite(`${this.options.stateDir}/restart-needed`, "1");
    try {
      await requiredCommand({
        executable: "/bin/launchctl",
        args: ["bootout", service, plist],
        input: "",
      });
      // Allow a graceful daemon shutdown; the ownership lock remains the final gate.
      await new Promise((resolve) => setTimeout(resolve, 500));
      await this.options.adapter.importStopped(snapshot, async (previous) => {
        const envelope = this.options.storage.envelope(previous);
        await atomicWrite(
          `${this.options.stateDir}/backups/${envelope.revision}.age`,
          await this.options.storage.encrypt(envelope),
        );
      });
      await atomicWrite(
        `${this.options.stateDir}/rebuild.json`,
        JSON.stringify(queue.data),
      );
    } finally {
      await this.recover();
    }
  }
  async rebuild(): Promise<void> {
    const text: string | null = await readOptional(
      `${this.options.stateDir}/rebuild.json`,
    );
    if (text === null) return;
    const parsed = queueSchema.safeParse(JSON.parse(text));
    check(parsed.success, "Invalid local rebuild queue");
    if (parsed.data.length === 0) return;
    const manifestText: string | null = await readOptional(
      `${this.options.dataDir}/server-control/server.json`,
    );
    check(manifestText, "Executor not ready for tool indexing");
    const manifest = manifestSchema.safeParse(JSON.parse(manifestText));
    check(
      manifest.success && manifest.data.scopeDir === this.options.repo,
      "Executor server scope mismatch",
    );
    const origin: URL = new URL(manifest.data.connection.origin);
    check(
      origin.protocol === "http:" &&
        ["localhost", "127.0.0.1"].includes(origin.hostname) &&
        !origin.username &&
        !origin.password &&
        origin.pathname === "/",
      "Refusing non-local management endpoint",
    );
    const remaining = await Promise.all(
      parsed.data.slice(0, 3).map(async (item) => {
        try {
          const path: string = `/api/connections/${encodeURIComponent(item.owner)}/${encodeURIComponent(item.integration)}/${encodeURIComponent(item.name)}/refresh`;
          const response: Response = await fetch(`${origin.origin}${path}`, {
            method: "POST",
            headers: {
              Authorization: `Bearer ${manifest.data.connection.auth.token}`,
              "Content-Type": "application/json",
            },
            body: "{}",
            redirect: "error",
            signal: AbortSignal.timeout(20_000),
          });
          if (!response.ok) return item;
          // Do not log provider responses (which can contain sensitive diagnostics).
          const body: unknown = await response.json();
          const result = z.array(z.unknown()).safeParse(body);
          return result.success && result.data.length > 0 ? null : item;
        } catch {
          return item;
        }
      }),
    );
    await atomicWrite(
      `${this.options.stateDir}/rebuild.json`,
      JSON.stringify([
        ...parsed.data.slice(3),
        ...remaining.filter((item) => item !== null),
      ]),
    );
  }
}

export async function checkAge(executable: string): Promise<void> {
  const result = await command({
    executable,
    args: ["--version"],
    input: "",
    identity: null,
  });
  check(result.code === 0, "age is not installed");
}
