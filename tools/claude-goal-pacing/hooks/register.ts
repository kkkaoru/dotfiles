// This TypeScript file is executed by the Claude Code mod engine.
import type { EngineInterface, EventResult, Register } from 'claude-code'

// Main-thread tool calls since the last stop: zero means the turn did no work.
interface Pacing {
  toolCalls: number
}

const IDLE_NOTICE: string =
  'goal-pacing: nothing was done since the last goal check, so the turn ends here. Send a message to continue and check the goal again.'

const pacing: Pacing = { toolCalls: 0 }

// A continuation that used no tool changed nothing the next check could see, so checking again
// would only repeat the last answer.
const idleContinuation = (stopHookActive: boolean): boolean => stopHookActive && pacing.toolCalls === 0

function endIdleTurn(engine: EngineInterface): EventResult<'classic.Stop'> {
  engine.ui.toast(IDLE_NOTICE)
  return {}
}

// The idle check is read before the count is reset for the next turn.
function pacedStop(
  engine: EngineInterface,
  stopHookActive: boolean,
  check: () => Promise<EventResult<'classic.Stop'>>,
): EventResult<'classic.Stop'> | Promise<EventResult<'classic.Stop'>> {
  const idle = idleContinuation(stopHookActive)
  pacing.toolCalls = 0
  return idle ? endIdleTurn(engine) : check()
}

function countCall<Result>(agentId: string | undefined, result: Result): Result {
  if (agentId === undefined) {
    pacing.toolCalls += 1
  }
  return result
}

// Skipping `next` on an idle continuation also skips the goal's own check, which is the slow part.
export const register: Register = on => {
  on('tool.call', (_engine, event, next) => countCall(event.agentId, next(event)))
  on('classic.Stop', (engine, event, next) => pacedStop(engine, event.stop_hook_active, () => next(event)))
}
