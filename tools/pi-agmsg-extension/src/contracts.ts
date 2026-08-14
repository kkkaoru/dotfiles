import type { ExecOptions, ExecResult } from "@earendil-works/pi-coding-agent";

export interface AgmsgActionInput {
  readonly action: "history" | "inbox" | "send" | "team" | "whoami";
  readonly limit?: number;
  readonly message?: string;
  readonly team?: string;
  readonly to?: string;
}

export interface ActiveIdentity {
  readonly agent: string;
  readonly teams: readonly string[];
}

export interface IdentityPair {
  readonly agent: string;
  readonly team: string;
}

export type IdentityLookup =
  | ({ readonly kind: "single" } & ActiveIdentity)
  | {
      readonly agents: readonly string[];
      readonly kind: "multiple";
      readonly teams: readonly string[];
    }
  | { readonly availableTeams: readonly string[]; readonly kind: "not-joined" }
  | {
      readonly agents: readonly string[];
      readonly availableTeams: readonly string[];
      readonly kind: "suggestion";
      readonly teams: readonly string[];
    };

export interface TeamMember {
  readonly name: string;
  readonly project?: string;
  readonly types: readonly string[];
}

export interface DialogUi {
  readonly confirm: (title: string, message: string) => Promise<boolean>;
  readonly editor: (title: string, prefill?: string) => Promise<string | undefined>;
  readonly notify: (message: string, type?: "error" | "info" | "warning") => void;
  readonly select: (title: string, options: string[]) => Promise<string | undefined>;
  readonly setStatus: (key: string, text: string | undefined) => void;
}

export interface SessionReader {
  readonly getBranch: () => readonly unknown[];
}

export interface RuntimeContext {
  readonly cwd: string;
  readonly hasUI: boolean;
  readonly sessionManager: SessionReader;
  readonly signal: AbortSignal | undefined;
  readonly ui: DialogUi;
}

export interface DisplayMessage {
  readonly content: string;
  readonly customType: string;
  readonly display: boolean;
}

export interface DeliveryOptions {
  readonly deliverAs?: "followUp" | "nextTurn" | "steer";
  readonly triggerTurn?: boolean;
}

export interface MessageSink {
  readonly sendMessage: (message: DisplayMessage, options?: DeliveryOptions) => void;
}

export interface IdentityStore {
  readonly load: (context: RuntimeContext) => ActiveIdentity | undefined;
  readonly save: (identity: ActiveIdentity | undefined) => void;
}

export interface RepeatScheduler {
  readonly repeat: (task: () => void, intervalMs: number) => () => void;
}

export interface ExecHost {
  readonly exec: (command: string, args: string[], options?: ExecOptions) => Promise<ExecResult>;
}

export interface InboxRequest {
  readonly agent: string;
  readonly quiet: boolean;
  readonly signal?: AbortSignal | undefined;
  readonly team: string;
}

export interface SendRequest {
  readonly from: string;
  readonly message: string;
  readonly signal?: AbortSignal | undefined;
  readonly team: string;
  readonly to: string;
}

export interface HistoryRequest {
  readonly agent: string;
  readonly limit: number;
  readonly signal?: AbortSignal | undefined;
  readonly team: string;
}

export interface JoinRequest {
  readonly agent: string;
  readonly project: string;
  readonly signal?: AbortSignal | undefined;
  readonly team: string;
}

export interface LeaveRequest {
  readonly agent: string;
  readonly signal?: AbortSignal | undefined;
  readonly team: string;
}

export interface AgmsgService {
  readonly history: (request: HistoryRequest) => Promise<string>;
  readonly identities: (project: string, signal?: AbortSignal) => Promise<readonly IdentityPair[]>;
  readonly inbox: (request: InboxRequest) => Promise<string>;
  readonly join: (request: JoinRequest) => Promise<string>;
  readonly leave: (request: LeaveRequest) => Promise<string>;
  readonly listTeams: (signal?: AbortSignal) => Promise<readonly string[]>;
  readonly members: (team: string, signal?: AbortSignal) => Promise<readonly TeamMember[]>;
  readonly send: (request: SendRequest) => Promise<string>;
  readonly team: (team: string, signal?: AbortSignal) => Promise<string>;
  readonly version: (signal?: AbortSignal) => Promise<string>;
  readonly whoami: (project: string, signal?: AbortSignal) => Promise<IdentityLookup>;
}

export interface IdentityCommandRequest {
  readonly command: string;
  readonly context: RuntimeContext;
  readonly identity: ActiveIdentity;
  readonly rest: string;
}

export interface IdentityResolution {
  readonly identity: ActiveIdentity;
  readonly notice?: string;
}
