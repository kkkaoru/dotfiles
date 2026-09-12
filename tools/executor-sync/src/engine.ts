// Runs with Bun. No timers, filesystem, network or credentials in the state machine.
import {
  type Envelope,
  fingerprint,
  type Snapshot,
  type SyncState,
} from "./model";

export interface RemoteValue {
  tag: string;
  envelope: Envelope;
}
export interface SyncPorts {
  exportLocal(): Promise<Snapshot>;
  importLocal(snapshot: Snapshot): Promise<void>;
  readRemote(): Promise<RemoteValue | null>;
  publish(snapshot: Snapshot): Promise<string>;
  saveState(state: SyncState): Promise<void>;
  now(): string;
}
export interface SyncResult {
  action: "uploaded" | "downloaded" | "unchanged";
  state: SyncState;
}

async function complete(
  ports: SyncPorts,
  action: SyncResult["action"],
  state: SyncState,
): Promise<SyncResult> {
  await ports.saveState(state);
  return { action, state };
}
export async function syncOnce(
  ports: SyncPorts,
  state: SyncState,
): Promise<SyncResult> {
  const local: Snapshot = await ports.exportLocal();
  const localHash: string = fingerprint(local);
  // A failed GET is never interpreted as an empty bucket; the caller retries later.
  const remote: RemoteValue | null = await ports.readRemote();
  const changed: boolean =
    state.localHash !== null && localHash !== state.localHash;
  if (changed || remote === null) {
    const remoteTag: string = await ports.publish(local);
    return complete(ports, "uploaded", {
      localHash,
      remoteTag,
      lastSuccess: ports.now(),
    });
  }
  if (remote.tag !== state.remoteTag) {
    if (fingerprint(remote.envelope.snapshot) !== localHash)
      await ports.importLocal(remote.envelope.snapshot);
    // Imports can re-key local credentials and IDs. Establish a fresh local baseline
    // instead of echoing the same remote change back indefinitely.
    const appliedHash: string = fingerprint(await ports.exportLocal());
    return complete(ports, "downloaded", {
      localHash: appliedHash,
      remoteTag: remote.tag,
      lastSuccess: ports.now(),
    });
  }
  return complete(ports, "unchanged", {
    localHash,
    remoteTag: remote.tag,
    lastSuccess: ports.now(),
  });
}
