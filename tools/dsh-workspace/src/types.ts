/** Runs with bun. Shared types for the dsh-workspace command. */

export interface Options {
  readonly directory: string;
  readonly title: string;
}

export type ParseResult =
  | { readonly kind: "help" }
  | { readonly kind: "options"; readonly options: Options }
  | { readonly kind: "usage-error"; readonly message: string };

export interface WorkspaceRecord {
  readonly workspaceId: string;
  readonly path: string;
}

export type DirectoryKind = "directory" | "file" | "missing";

/** One HTTP exchange, reduced to what the workspace call needs. */
export interface FetchRequest {
  readonly url: string;
  readonly method: "GET" | "POST";
  readonly body?: string;
  readonly cookie?: string;
}

export interface FetchResponse {
  readonly status: number;
  readonly body: string;
  readonly setCookie?: string;
}

export interface StartHostOptions {
  readonly port: number;
  readonly logPath: string;
}

/** A `dsh web` process whose URL line is read from its log. */
export interface SpawnedHost {
  readonly running: () => boolean;
  readonly stop: () => void;
}

/** Every external effect the command performs, injected so tests stay offline. */
export interface Effects {
  readonly harnessHome: () => string;
  readonly canonicalDirectory: (directory: string) => string;
  readonly directoryKind: (directory: string) => DirectoryKind;
  readonly temporaryFile: (prefix: string) => string;
  readonly readText: (path: string) => string | undefined;
  readonly removeFile: (path: string) => void;
  readonly freePort: () => Promise<number>;
  readonly startHost: (options: StartHostOptions) => SpawnedHost;
  readonly fetchResponse: (request: FetchRequest) => Promise<FetchResponse>;
  readonly sleep: (milliseconds: number) => Promise<void>;
}

export type CreateWorkspaceResult =
  | { readonly kind: "created"; readonly workspace: WorkspaceRecord }
  | { readonly kind: "exists"; readonly workspace: WorkspaceRecord }
  | { readonly kind: "error"; readonly message: string };

export interface RunInput {
  readonly argv: readonly string[];
  readonly cwd: string;
  readonly effects: Effects;
}

export interface RunResult {
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
}
