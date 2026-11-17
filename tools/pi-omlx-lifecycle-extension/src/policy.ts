// Runs under bun (see package.json scripts) via pi's TypeScript extension loader.

/** Provider id used by the omlx entry in .pi/agent/models.json. */
export const OMLX_PROVIDER_ID = "omlx";

/** Local Bonsai is a model on the same oMLX server, not its own process. */
export const BONSAI_PROVIDER_ID = "bonsai";

/** True when this provider's requests are served by the local oMLX process. */
export const usesLocalOmlx = (provider: string | undefined): boolean =>
  provider === OMLX_PROVIDER_ID || provider === BONSAI_PROVIDER_ID;

/** Action to take against the managed omlx server. */
export type OmlxAction = "none" | "start" | "stop";

export interface ModelSwitchInput {
  readonly nextProvider: string;
  readonly previousProvider: string | undefined;
}

/**
 * Decide what to do with the omlx server when the active model/provider changes.
 * - Switching into omlx or local bonsai starts it. Moving between those two stays up.
 * - Switching away from either (to any other provider) nudges the idle-stop check.
 * - Switching between two providers that are not served by local oMLX is a no-op.
 */
export function decideModelSwitchAction(input: ModelSwitchInput): OmlxAction {
  if (usesLocalOmlx(input.nextProvider)) {
    return "start";
  }
  return usesLocalOmlx(input.previousProvider) ? "stop" : "none";
}

/** Decide what to do with the omlx server when the session/extension runtime is shutting down. */
export function decideShutdownAction(currentProvider: string | undefined): OmlxAction {
  return usesLocalOmlx(currentProvider) ? "stop" : "none";
}
