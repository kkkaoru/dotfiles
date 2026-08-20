// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.

/** Provider id used by the omlx entry in .pi/agent/models.json. */
export const OMLX_PROVIDER_ID = "omlx";

/** Action to take against the managed omlx server. */
export type OmlxAction = "none" | "start" | "stop";

export interface ModelSwitchInput {
  readonly nextProvider: string;
  readonly previousProvider: string | undefined;
}

/**
 * Decide what to do with the omlx server when the active model/provider changes.
 * - Switching into omlx (from any other provider, or from no model at all) starts it.
 * - Switching away from omlx (to any other provider) nudges the idle-stop check.
 * - Anything else (switching between two non-omlx providers, or staying on omlx) is a no-op.
 */
export function decideModelSwitchAction(input: ModelSwitchInput): OmlxAction {
  const enteringOmlx: boolean = input.nextProvider === OMLX_PROVIDER_ID;
  if (enteringOmlx) {
    return "start";
  }
  const leavingOmlx: boolean = input.previousProvider === OMLX_PROVIDER_ID;
  return leavingOmlx ? "stop" : "none";
}

/** Decide what to do with the omlx server when the session/extension runtime is shutting down. */
export function decideShutdownAction(currentProvider: string | undefined): OmlxAction {
  return currentProvider === OMLX_PROVIDER_ID ? "stop" : "none";
}
