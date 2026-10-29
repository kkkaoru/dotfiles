// This TypeScript file is executed by the Claude Code mod engine.
import type {
  EngineInterface,
  ModelEffort,
  PluginOptions,
  PromptOrigin,
  Register,
  RenderElement,
  SessionCompactInput,
  SessionCompactResult,
  SessionCompactTrigger,
  SessionContextUsage,
  SessionMessage,
  HookStream,
  TurnStepChunk,
  TurnStepInput,
  TurnStepResult,
  TurnUsage,
} from 'claude-code'

// The recovery floor and what it remembers between rounds. Replaced whole, once per round, so
// all of it changes in one assignment.
interface Floor {
  readonly steps: number
  readonly successes: number
  readonly lastTurn: string
  readonly lastIndex: number
  readonly stuck: number
  readonly nudged: boolean
}

// The tally of the open tool round. Mutated in place, so a tool call allocates nothing.
interface Round {
  calls: number
  failed: boolean
  repeated: boolean
  verified: boolean
}

// A fixed-size ring of recent keys: constant memory, constant-time membership.
interface Ring {
  readonly items: string[]
  cursor: number
}

// The session effort Claude Code holds, and whether the mod's `/effort` is still queued. One
// immutable record, replaced whole, so both always change together.
interface SessionEffort {
  readonly level: ModelEffort | undefined
  readonly pending: boolean
}

// Where a main step sits in the conversation; a turn's steps are numbered from zero.
type StepPosition = Pick<TurnStepInput, 'turnId' | 'index'>

interface Manager {
  enabled: boolean
  lastSent: ModelEffort | undefined
  original: ModelEffort | undefined
  session: SessionEffort
  sessionId: string | undefined
  stepped: boolean
  generation: number
  contextTokens: number
  prefetchedLimit: number | undefined
  readonly limits: Map<string, number>
  floor: Floor
  round: Round
  readonly recentCalls: Ring
  readonly failedInputs: Ring
}

// A switch read back for a session: applied only while no newer reset owns the state.
interface StoredSwitch {
  readonly generation: number
  readonly sessionId: string
  readonly off: boolean
}

// One main-agent tool call as recovery evidence: what it asked and what it got back.
interface CallRecord {
  readonly tool: string
  readonly input: unknown
  readonly isError: boolean
  readonly text: string | undefined
}

// One call's effect on the round: `held` is a repeat or a malformed call, `fixed` a verified fix.
interface CallOutcome {
  readonly key: string
  readonly failed: boolean
  readonly held: boolean
  readonly fixed: boolean
}

// The levels a main request can be sent with, from the chosen minimum up to the work cap.
interface Policy {
  start: ModelEffort
  operational: readonly ModelEffort[]
  ranks: ReadonlyMap<ModelEffort, number>
  last: number
  rampStep: number
}

interface Runtime {
  policy: Policy
  manager: Manager
}

type DynamicAction = 'on' | 'off' | 'status'

type EffortMode = 'dynamic' | 'static'

type MissingValue = 'unknown' | 'none'

// shortcut: Claude Code exposes no per-model level list; the engine silently
// downgrades a level the model lacks. Read a capability API once one exists.
const LEVELS: readonly ModelEffort[] = ['low', 'medium', 'high', 'xhigh', 'max']
// The minimum is the person's choice (`minEffort` in the config menu); `medium`, one above the
// lowest, is the default because too small a budget can cost more tokens, not fewer (TALE token
// elasticity, arXiv:2412.18547) and all-low steps lose accuracy (ARES).
const DEFAULT_START: ModelEffort = 'medium'
const END: ModelEffort = 'xhigh'
const MINIMUMS: ReadonlySet<string> = new Set(['low', 'medium', 'high', 'xhigh'])
const MIN_EFFORT_OPTION: string = 'minEffort'
// Evidence behind the policy (README "Design and evidence"): effort rises on external failure
// evidence (ARES arXiv:2603.07915; Reflexion arXiv:2303.11366; Huang arXiv:2310.01798) and with
// context length (context rot), from a medium baseline capped at xhigh (overthinking
// arXiv:2502.08235; inverse scaling arXiv:2507.14417). Effort changes keep Claude Code's prompt
// cache (measured on Sonnet 5.5 and Opus 5.5), so the level follows the evidence step by step.
// A main call repeating one of the last few calls with the same outcome made no progress (MAST
// arXiv:2503.13657). A repeat holds the level instead of raising it: polling a long job repeats too.
const REPEAT_WINDOW = 8 satisfies number
// Signatures keep bounded slices, so the window stays small whatever the tools take or return.
const SIGNATURE_INPUT_CHARS = 512 satisfies number
const SIGNATURE_TEXT_CHARS = 256 satisfies number
const LONG_STRING_CHARS: number = SIGNATURE_INPUT_CHARS * 2
// Per-call ids and the model's free-text label of a call (Bash `description`) are not what it does.
const VOLATILE_KEYS: ReadonlySet<string> = new Set(['tool_use_id', 'requestMeta', 'description', 'tool'])
// A malformed call (invalid arguments, an unknown tool) needs a corrected call, not deeper
// reasoning: stronger reasoning raises tool hallucination (Reasoning Trap, arXiv:2510.22977), so it
// holds the floor like a repeat. The texts are Claude Code's own tool errors.
const MALFORMED_CALL = /InputValidationError|No such tool available/u
// A call that failed and then succeeds with the same input (a test or build that now passes) is
// verified recovery: effort drops a level at once, since shorter reasoning once the answer is
// confirmed is both cheaper and more accurate (DEER arXiv:2504.15895; execution feedback,
// Self-Debugging arXiv:2304.05128). No output text is interpreted.
// After this many failing rounds in a row at the work cap, more reasoning on the same path stops
// paying (Mirage of test-time scaling arXiv:2506.04210; expensive failures, SWE-Effi
// arXiv:2509.09853), so the failing result carries one note to change approach. The note is
// external evidence, not intrinsic self-correction (Huang arXiv:2310.01798), and it is appended
// after the result, so the prompt cache is untouched.
const STUCK_ROUNDS = 3 satisfies number
const STUCK_NOTE: string =
  'dynamic-effort: this approach has now failed for several rounds at the highest work effort. More reasoning on the same path is unlikely to help: state what is blocking, then try a different approach or ask the person.'
// A resumed session replays this many of its last tool rounds to rebuild the recovery evidence.
const RESUME_ROUNDS = 8 satisfies number
const RESUME_TURN: string = 'resume'
// Sessions switched off with `/dynamic-effort off`, newest last. A resumed session keeps its id and
// so its switch; a new session's id is not listed, so it starts on. Bounded, so the store does not
// grow with the number of sessions.
const OFF_SESSIONS_KEY: string = 'offSessions'
const MAX_OFF_SESSIONS = 32 satisfies number
const RAMP_START_RATIO = 0.6 satisfies number
const RECOVERY_ROUNDS = 2 satisfies number
const COMPACTION_EFFORT: ModelEffort = 'max'
const COMMAND: string = 'dynamic-effort'
const EFFORT_COMMAND: string = 'effort'
const ACTION_ON: DynamicAction = 'on'
const ACTION_OFF: DynamicAction = 'off'
const ACTION_STATUS: DynamicAction = 'status'
const ACTIONS: readonly DynamicAction[] = [ACTION_ON, ACTION_OFF, ACTION_STATUS]
const ACTION_USAGE: string = ACTIONS.join('|')
const MODE_DYNAMIC: EffortMode = 'dynamic'
const MODE_STATIC: EffortMode = 'static'
const UNKNOWN: MissingValue = 'unknown'
const NONE: MissingValue = 'none'
const PLUGIN: string = 'dynamic-effort'
const EMPTY_ROW: RenderElement = Object.freeze({ type: 'Box', children: [] })
// Hot-path texts are built once per level, not on every step or spinner redraw.
const SPINNER_TAGS: ReadonlyMap<ModelEffort, string> = new Map(LEVELS.map(level => [level, ` · ${MODE_DYNAMIC} ${level}`]))
const STATUS_TEXTS: ReadonlyMap<ModelEffort, string> = new Map(LEVELS.map(level => [level, `effort: ${level} (${MODE_DYNAMIC})`]))
const STATIC_STATUS: string = `effort: ${MODE_STATIC}`
const PRECOMPUTE_SKIP: string = 'dynamic-effort: compaction effort is not applied yet'
// Claude Code's summary keeps errors and fixes but not abandoned approaches; losing what failed
// invites repeating it (step repetition, MAST arXiv:2503.13657), and a compression guideline that
// names what must survive keeps accuracy at a fraction of the tokens (ACON arXiv:2510.00615).
const COMPACTION_GUIDELINE: string =
  'Also list approaches that were tried and abandoned, with the error or reason, so they are not repeated.'
const EFFORT_NAMES: ReadonlySet<string> = new Set(LEVELS)
const ACTION_NAMES: ReadonlySet<string> = new Set(ACTIONS)

// Lookups the hot paths need are built once per minimum, so each is a constant-time hash read.
// Failure pressure stops at `last`, what the levels can express, so recovery is two clean rounds
// per level: holding a raised level after recovery is overthinking (arXiv:2502.08235) and longer
// reasoning can lower accuracy (inverse scaling, arXiv:2507.14417).
function policyFor(start: ModelEffort): Policy {
  const operational = LEVELS.slice(LEVELS.indexOf(start), LEVELS.indexOf(END) + 1)
  const last = operational.length - 1
  return {
    start,
    operational,
    ranks: new Map(operational.map((level, rank) => [level, rank])),
    last,
    rampStep: (1 - RAMP_START_RATIO) / Math.max(1, last),
  }
}

// A value outside the picker's options counts as unset, as the config menu treats it.
const isMinimum = (value: unknown): value is ModelEffort => typeof value === 'string' && MINIMUMS.has(value) && isEffort(value)

function minimumOf(options: PluginOptions): ModelEffort {
  const chosen = options[MIN_EFFORT_OPTION]
  return isMinimum(chosen) ? chosen : DEFAULT_START
}

const emptyRing = (): Ring => ({ items: Array.from({ length: REPEAT_WINDOW }, () => ''), cursor: 0 })

// Adds a key and reports whether it was already among the recent ones.
function pushRing(ring: Ring, key: string): boolean {
  const seen = ring.items.includes(key)
  ring.items[ring.cursor] = key
  ring.cursor = (ring.cursor + 1) % REPEAT_WINDOW
  return seen
}

function clearRing(ring: Ring): void {
  ring.items.fill('')
  ring.cursor = 0
}

const UNKNOWN_SESSION: SessionEffort = { level: undefined, pending: false }

const openRound = (): Round => ({ calls: 0, failed: false, repeated: false, verified: false })

// shortcut: module state restarts on a hot reload (development only); move to
// the engine's `state` with a contract if reload persistence starts to matter.
const fresh = (generation: number, original: ModelEffort | undefined): Manager => ({
  enabled: true,
  lastSent: undefined,
  original,
  session: UNKNOWN_SESSION,
  sessionId: undefined,
  stepped: false,
  generation,
  contextTokens: 0,
  prefetchedLimit: undefined,
  limits: new Map(),
  floor: { steps: 0, successes: 0, lastTurn: '', lastIndex: -1, stuck: 0, nudged: false },
  round: openRound(),
  recentCalls: emptyRing(),
  failedInputs: emptyRing(),
})

// The live state, replaced by reference on a reset or a new minimum: nothing is copied field by field.
const runtime: Runtime = { policy: policyFor(DEFAULT_START), manager: fresh(0, undefined) }

function rampEffort(tokens: number, limit: number): ModelEffort {
  const ratio = Math.max(0, tokens) / Math.max(1, limit)
  return ratio <= RAMP_START_RATIO
    ? runtime.policy.start
    : (runtime.policy.operational[Math.min(runtime.policy.last, 1 + Math.floor((ratio - RAMP_START_RATIO) / runtime.policy.rampStep))] ??
        runtime.policy.start)
}

const nextSteps = (failed: boolean, steps: number, lowered: boolean): number =>
  failed ? Math.min(runtime.policy.last, steps + 1) : Math.max(0, steps - Number(lowered))

// A failure, a repeat or a verified fix starts the count of clean rounds over.
const interruptsRecovery = ({ failed, repeated }: Round, verified: boolean): boolean => failed || repeated || verified

// The recovery floor after a finished tool round: a failure raises it, a repeat holds it, a
// verified fix lowers it at once, two clean rounds lower it. Failing rounds at the cap are counted.
function nextFloor({ floor, round }: Manager, { turnId, index }: StepPosition): Floor {
  const verified = !round.failed && round.verified
  const successes = interruptsRecovery(round, verified) ? 0 : floor.successes + 1
  const recovered = successes >= RECOVERY_ROUNDS
  return {
    lastTurn: turnId,
    lastIndex: index,
    stuck: round.failed && floor.steps >= runtime.policy.last ? floor.stuck + 1 : 0,
    nudged: round.failed && floor.nudged,
    steps: nextSteps(round.failed, floor.steps, recovered || verified),
    successes: recovered ? 0 : successes,
  }
}

// Closes the tool round tallied since the previous main step; drained on every main step so a
// round never outlives the step that follows it. A round without calls has nothing to close or
// clear, so most steps allocate nothing; a repeated step does not fold its round twice.
function observeRound(position: StepPosition): void {
  if (runtime.manager.round.calls === 0) {
    return
  }
  if (position.turnId !== runtime.manager.floor.lastTurn || position.index !== runtime.manager.floor.lastIndex) {
    runtime.manager.floor = nextFloor(runtime.manager, position)
  }
  runtime.manager.round = openRound()
}

const withRecovery = (selected: ModelEffort, steps: number): ModelEffort =>
  runtime.policy.operational[Math.max(runtime.policy.ranks.get(selected) ?? 0, Math.min(steps, runtime.policy.last))] ?? selected

const isAction = (value: string): value is DynamicAction => ACTION_NAMES.has(value)

const isEffort = (value: string): value is ModelEffort => EFFORT_NAMES.has(value)

// The session's auto-compact threshold (the breakdown is a local estimate, no request).
const thresholdOf = ({ breakdown, window }: SessionContextUsage): number => breakdown?.autoCompactThreshold ?? window

// A generation check keeps work started for an earlier reset from writing into a newer session,
// and a step that already ran holds fresher evidence than anything read here.
const isCurrent = (generation: number): boolean => runtime.manager.generation === generation && !runtime.manager.stepped

// Until the first response, the context size is the local estimate, so a resumed session ramps
// from its real size instead of zero. The usage arrives as an argument, after its await.
function prefetchIfCurrent(generation: number, context: SessionContextUsage): void {
  if (isCurrent(generation)) {
    runtime.manager.prefetchedLimit = thresholdOf(context)
    runtime.manager.contextTokens = context.tokens ?? context.breakdown?.totalTokens ?? 0
  }
}

// A session that already has prompts rebuilds its recovery evidence from the transcript's last
// tool rounds. Nothing here compacts: resuming only reads.
// Checked before the read, to skip it for a session already replaced, and again once the
// messages arrive, since a step or reset can land while they load.
async function restoreSession(engine: EngineInterface, generation: number): Promise<void> {
  if ((await engine.session.turns()) > 0 && isCurrent(generation)) {
    replayIfCurrent(generation, await engine.session.messages())
  }
}

// The messages are an argument, so they are awaited before the generation is checked.
const replayIfCurrent = (generation: number, messages: readonly SessionMessage[]): void =>
  isCurrent(generation) && tracksCall(undefined) ? replayRounds(messages) : undefined

function replayRounds(messages: readonly SessionMessage[]): void {
  messages
    .filter(message => message.role === 'assistant' && message.toolUses.length > 0)
    .slice(-RESUME_ROUNDS)
    .forEach(({ toolUses }, round) => {
    toolUses.forEach(({ input, tool, isError, text }) =>
      recordCall({ tool, input, isError: isError === true, text }),
    )
    observeRound({ turnId: RESUME_TURN, index: round })
  })
}

// Starts a session's state over. The context estimate and the transcript are read in the
// background; the on/off switch is awaited, so no step is sent before it applies. An in-process
// `/clear`, `/resume` or fork keeps the person's own effort, which outlives the conversation.
function resetSession(engine: EngineInterface, keepOriginal: boolean): Promise<void> {
  const generation = runtime.manager.generation + 1
  runtime.manager = fresh(generation, keepOriginal ? runtime.manager.original : undefined)
  void engine.session
    .usage({ breakdown: 'summary' })
    .then(({ context }) => prefetchIfCurrent(generation, context))
    .then(() => restoreSession(engine, generation))
    .catch(() => undefined)
  return restoreSwitch(engine, generation)
}

// The store is plain JSON any version of the mod may have written, so anything else counts as none.
const storedSessions = (value: unknown): readonly string[] =>
  Array.isArray(value) ? value.filter((sessionId): sessionId is string => typeof sessionId === 'string') : []

const withoutSession = (sessions: readonly string[], sessionId: string): string[] =>
  sessions.filter(listed => listed !== sessionId)

// The off list after a switch: an off session moves to the end, an on one leaves the list.
const offSessionsAfter = (sessions: readonly string[], sessionId: string, enabled: boolean): readonly string[] =>
  enabled ? withoutSession(sessions, sessionId) : [...withoutSession(sessions, sessionId), sessionId].slice(-MAX_OFF_SESSIONS)

// A store that cannot be read leaves the session on, the default.
const restoreSwitch = (engine: EngineInterface, generation: number): Promise<void> =>
  Promise.all([engine.session.id(), engine.store.get(OFF_SESSIONS_KEY)])
    .then(([sessionId, stored]) => applySwitch(engine, { generation, sessionId, off: storedSessions(stored).includes(sessionId) }))
    .catch(() => undefined)

function applySwitch(engine: EngineInterface, { generation, sessionId, off }: StoredSwitch): void {
  if (runtime.manager.generation !== generation) {
    return
  }
  runtime.manager.sessionId = sessionId
  if (off) {
    setEnabled(engine, false)
  }
}

// shortcut: two sessions switching at the same instant can lose one write (read, then write);
// move to a per-session key if that ever matters. A failed write only loses the switch on resume.
function rememberSwitch(engine: EngineInterface, enabled: boolean): void {
  const sessionId = runtime.manager.sessionId
  if (sessionId === undefined) {
    return
  }
  void engine.store
    .get(OFF_SESSIONS_KEY)
    .then(stored => engine.store.set(OFF_SESSIONS_KEY, offSessionsAfter(storedSessions(stored), sessionId, enabled)))
    .catch(() => undefined)
}

// `Map#set` returns the map, which is always truthy, so the expression yields `limit`.
const rememberLimit = (model: string, limit: number): number => runtime.manager.limits.set(model, limit) && limit

// The threshold depends on the model's window, so it is cached per model. The prefetched value
// serves only while the cache is empty, so the first model takes it and no later model reuses it.
const cachedLimit = (model: string): number | undefined =>
  runtime.manager.limits.get(model) ??
  (runtime.manager.limits.size === 0 && runtime.manager.prefetchedLimit !== undefined
    ? rememberLimit(model, runtime.manager.prefetchedLimit)
    : undefined)

// Main requests are rewritten per step, so the session effort only reaches compaction and
// inheriting subagents. It should be the compaction level while dynamic, the original while off,
// and nothing until an effort is known.
const desiredSessionLevel = (): ModelEffort | undefined =>
  runtime.manager.enabled && runtime.manager.original !== undefined ? COMPACTION_EFFORT : runtime.manager.original

// A level is known, no command is in flight, and the session is not there yet.
const needsSessionEffort = (level: ModelEffort | undefined): level is ModelEffort =>
  level !== undefined && !runtime.manager.session.pending && level !== runtime.manager.session.level

// `engine.command.run` is refused inside a command hook, so the change is queued at a main turn
// end; a person's `/effort` that ran first, or a dropped command, is corrected at the next one.
function syncSessionEffort(engine: EngineInterface): void {
  const level = desiredSessionLevel()
  if (needsSessionEffort(level)) {
    queueSessionEffort(engine, level)
  }
}

function queueSessionEffort(engine: EngineInterface, level: ModelEffort): void {
  runtime.manager.session = { level: runtime.manager.session.level, pending: true }
  // Claude Code compacts at the session effort; `/effort` sets it for this session only. A hook's
  // command is queued until the session is idle, so it is never awaited (that would deadlock a
  // turn). A command that ran set the level (the next main step's own effort confirms it); a
  // refused or dropped one leaves it, and the next turn end compares the levels again.
  void engine.command.run({ command: EFFORT_COMMAND, args: level }).then(() => settleCommand(level), () => settleCommand(runtime.manager.session.level))
}

function settleCommand(level: ModelEffort | undefined): void {
  runtime.manager.session = { level, pending: false }
}

// While the session sits at the compaction level, a step that inherited it (a subagent, or any
// step once dynamic mode is off) goes back to the original. An explicit subagent `max` looks the
// same and is restored too.
const inheritsCompactionLevel = (effort: ModelEffort | number | undefined): boolean =>
  effort === COMPACTION_EFFORT && runtime.manager.session.level === COMPACTION_EFFORT && runtime.manager.original !== COMPACTION_EFFORT

// Subagents always inherit the session effort; main steps inherit it only while dynamic mode is off.
const inheritsSessionEffort = (agentId: string | undefined): boolean => agentId !== undefined || !runtime.manager.enabled

const restoredEffort = (agentId: string | undefined, effort: ModelEffort | number | undefined): ModelEffort | undefined =>
  inheritsCompactionLevel(effort) && inheritsSessionEffort(agentId) ? runtime.manager.original : undefined

const isOwnEcho = (origin: PromptOrigin): boolean =>
  origin.kind === 'plugin' && origin.name === PLUGIN

// Context size comes from the previous main response's usage, so with the threshold cached a
// step is decided synchronously: no host call, no promise, no microtask before the request.
const dynamicEffort = (limit: number): ModelEffort =>
  withRecovery(rampEffort(runtime.manager.contextTokens, limit), runtime.manager.floor.steps)

// The status line changes only with the level, so a steady session makes no host call.
const sendEffort = (engine: EngineInterface, effort: ModelEffort): ModelEffort =>
  effort === runtime.manager.lastSent ? effort : announceEffort(engine, effort)

function announceEffort(engine: EngineInterface, effort: ModelEffort): ModelEffort {
  engine.ui.status(STATUS_TEXTS.get(effort))
  runtime.manager.lastSent = effort
  return effort
}

// The input side of a main response, as the status line counts the context window; any other
// step leaves the count as it was.
const contextAfter = (agentId: string | undefined, usage: TurnUsage | null): number =>
  agentId !== undefined || usage === null
    ? runtime.manager.contextTokens
    : usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens

const status = (): string =>
  [
    `${MODE_DYNAMIC}=${runtime.manager.enabled ? ACTION_ON : ACTION_OFF}`,
    `original=${runtime.manager.original ?? UNKNOWN} session=${runtime.manager.session.level ?? UNKNOWN} pending=${runtime.manager.session.pending}`,
    `last=${runtime.manager.lastSent ?? NONE}`,
    `min=${runtime.policy.start} end=${END} levels=${runtime.policy.operational.join(',')}`,
    `compactLimits=${[...runtime.manager.limits.values()].join(',') || UNKNOWN}`,
    `context=${runtime.manager.contextTokens}`,
    `recoverySteps=${runtime.manager.floor.steps}`,
  ].join(' ')

const startSession = (engine: EngineInterface): Promise<unknown> =>
  Promise.all([
    resetSession(engine, false),
    engine.command.register({
      name: COMMAND,
      description: `Dynamic reasoning effort: ${ACTIONS.join(' | ')}`,
      argumentHint: ACTION_USAGE,
    }),
  ])

function setEnabled(engine: EngineInterface, enabled: boolean): void {
  runtime.manager.enabled = enabled
  // Steps sent while off carried the session effort, so the next dynamic choice starts unpinned.
  runtime.manager.lastSent = undefined
  engine.ui.status(enabled ? undefined : STATIC_STATUS)
}

function runAction(engine: EngineInterface, action: string): string {
  if (action === ACTION_ON || action === ACTION_OFF) {
    setEnabled(engine, action === ACTION_ON)
    rememberSwitch(engine, action === ACTION_ON)
  }
  return isAction(action) ? status() : `Usage: /${COMMAND} ${ACTION_USAGE}`
}

// Every `/effort` that ran sets the session level; the mod's own clears its pending command and
// the person's becomes the original.
function observeEffortCommand(origin: PromptOrigin, level: string): void {
  if (!isEffort(level)) {
    return
  }
  const isOwn = isOwnEcho(origin)
  runtime.manager.session = { level, pending: !isOwn && runtime.manager.session.pending }
  if (!isOwn) {
    runtime.manager.original = level
  }
}

// What a value holds: a short value itself, a long one as length plus head and tail slices. Most
// results are short, so they are kept as they are instead of being sliced and stored twice.
const edges = (value: string, chars: number): string =>
  value.length <= chars * 2 ? value : `${value.length}\u0000${value.slice(0, chars)}\u0000${value.slice(-chars)}`

// Drops per-call ids, labels and the tool name (keyed separately), and condenses a long string (a
// written file's content) to its edges before it is serialized, so a large input is never copied
// whole just to be sliced.
const condensed = (value: unknown): unknown =>
  typeof value === 'string' && value.length > LONG_STRING_CHARS ? edges(value, SIGNATURE_INPUT_CHARS) : value

const compactValue = (key: string, value: unknown): unknown => (VOLATILE_KEYS.has(key) ? undefined : condensed(value))

// What a call asked: the tool, then its arguments as length plus head and tail slices, so a long
// edit that differs mid-way is a different call. The tool is keyed apart from the arguments, so a
// replayed call and a live one compare equal wherever the engine put the `tool` key.
const inputKey = (tool: string, input: unknown): string =>
  `${tool}\u0000${edges(JSON.stringify(input, compactValue) ?? '', SIGNATURE_INPUT_CHARS)}`

// The call plus what it got back: equal signatures mean a repeat that produced the same outcome.
const toolSignature = (key: string, { isError, text }: Pick<CallRecord, 'isError' | 'text'>): string =>
  `${key}\u0000${isError}\u0000${edges(text ?? '', SIGNATURE_TEXT_CHARS)}`

// The floor is at the cap, the failing rounds there reached the limit, and no note was given yet.
const claimsStuckNote = ({ steps, stuck, nudged }: Floor): boolean =>
  steps >= runtime.policy.last && stuck >= STUCK_ROUNDS - 1 && !nudged

// Adds a classified call to the round and reports whether it claims the one stuck note.
function tallyCall({ key, failed, held, fixed }: CallOutcome): boolean {
  if (failed) {
    pushRing(runtime.manager.failedInputs, key)
  }
  runtime.manager.round.calls += 1
  runtime.manager.round.failed ||= failed
  runtime.manager.round.repeated ||= held
  runtime.manager.round.verified ||= fixed
  const stuck = failed && claimsStuckNote(runtime.manager.floor)
  // The note is given once per run of failures; only that call allocates a new floor.
  if (stuck) {
    runtime.manager.floor = { ...runtime.manager.floor, nudged: true }
  }
  return stuck
}

// Only the main agent's calls count, and only while dynamic; checked before anything is built.
const tracksCall = (agentId: string | undefined): boolean => agentId === undefined && runtime.manager.enabled

// A failed call raises the round, a fixed one verifies recovery, and a repeat with the same
// outcome or a malformed call holds it. It reports whether this call should carry the stuck note.
function recordCall({ tool, input, isError, text }: CallRecord): boolean {
  const key = inputKey(tool, input)
  const malformed = isError && MALFORMED_CALL.test(text ?? '')
  // Both rings are read inside the literal, before tallyCall records this call's failure.
  return tallyCall({
    key,
    failed: isError && !malformed,
    held: pushRing(runtime.manager.recentCalls, toolSignature(key, { isError, text })) || malformed,
    fixed: !isError && runtime.manager.failedInputs.items.includes(key),
  })
}

// A tool result, with the stuck note appended when this call ends a run of failures at the cap.
// The call's input is the event itself, so one record is the only allocation per counted call.
const noteToolResult = <ToolResult extends { readonly isError?: boolean; readonly text?: string; readonly context?: readonly string[] }>(
  event: { readonly agentId?: string; readonly tool: string },
  result: ToolResult,
): ToolResult =>
  tracksCall(event.agentId) && recordCall({ tool: event.tool, input: event, isError: result.isError === true, text: result.text })
    ? { ...result, context: [...(result.context ?? []), STUCK_NOTE] }
    : result

// The step is not rewritten: its inherited level is restored, dynamic mode is off, or it carries a budget.
const keepsStepEffort = (restored: ModelEffort | undefined, effort: TurnStepInput['effort']): boolean =>
  restored !== undefined || !runtime.manager.enabled || typeof effort !== 'string'

// The effort a main step is sent with: the original for an inherited compaction level, the
// dynamic level for a managed step, and the step's own effort otherwise. Only a step whose model
// has no threshold yet returns a promise.
function planMainEffort(engine: EngineInterface, step: TurnStepInput): TurnStepInput['effort'] | Promise<ModelEffort> {
  recordMainStep(step)
  const restored = restoredEffort(undefined, step.effort)
  return keepsStepEffort(restored, step.effort) ? keepStepEffort(restored ?? step.effort) : managedEffort(engine, step.model)
}

// A main step closes the tool round before it, and its effort is the level the session holds.
function recordMainStep(step: TurnStepInput): void {
  runtime.manager.stepped = true
  observeRound(step)
  if (typeof step.effort !== 'string') {
    return
  }
  // Written only on a change, so a main step allocates and writes nothing in the common case.
  if (step.effort !== runtime.manager.session.level) {
    runtime.manager.session = { level: step.effort, pending: runtime.manager.session.pending }
  }
  runtime.manager.original ??= step.effort
}

// A step sent with its own effort pins nothing, so the next dynamic choice announces itself again.
function keepStepEffort(effort: TurnStepInput['effort']): TurnStepInput['effort'] {
  runtime.manager.lastSent = undefined
  return effort
}

function managedEffort(engine: EngineInterface, model: string): ModelEffort | Promise<ModelEffort> {
  const limit = cachedLimit(model)
  // The breakdown is a local estimate, so the one lookup per model sends no request.
  return limit === undefined
    ? engine.session.usage({ breakdown: 'summary' }).then(({ context }) => sendEffort(engine, dynamicEffort(rememberLimit(model, thresholdOf(context)))))
    : sendEffort(engine, dynamicEffort(limit))
}

const planEffort = (engine: EngineInterface, step: TurnStepInput): TurnStepInput['effort'] | Promise<ModelEffort> =>
  step.agentId === undefined ? planMainEffort(engine, step) : (restoredEffort(step.agentId, step.effort) ?? step.effort)

// The core writes a command's output row while it runs, so the compaction level's row is hidden
// where drawn. Its one-line echo stays: a UserMessage hook would run for every message redraw.
// The exact match first skips allocating a trimmed copy for the common case.
const isCompactionRow = (command: string, args: string): boolean =>
  command === EFFORT_COMMAND && (args === COMPACTION_EFFORT || args.trim() === COMPACTION_EFFORT)

// Claude Code draws its own effort from the session setting; the spinner suffix adds the sent one.
const spinnerTag = (): string | undefined =>
  runtime.manager.enabled && runtime.manager.lastSent !== undefined ? SPINNER_TAGS.get(runtime.manager.lastSent) : undefined

const completeTurn = (engine: EngineInterface, agentId: string | undefined): void =>
  agentId === undefined ? syncSessionEffort(engine) : undefined

// Dynamic mode wants the compaction level, and the session has not reached it yet.
const compactionLevelPending = (): boolean =>
  runtime.manager.enabled && runtime.manager.original !== undefined && runtime.manager.session.level !== COMPACTION_EFFORT

// A precompute made before the compaction level applies would be reused at the old effort.
const precomputeSkip = (trigger: SessionCompactTrigger): SessionCompactResult | undefined =>
  trigger === 'precompute' && compactionLevelPending() ? { skip: PRECOMPUTE_SKIP }
    : undefined

// The person's `/compact` focus leads; the guideline follows it. Subagent compactions are left alone.
const guidedCompaction = (input: SessionCompactInput): SessionCompactInput =>
  input.agentId !== undefined
    ? input
    : { ...input, instructions: input.instructions === undefined ? COMPACTION_GUIDELINE : `${input.instructions}\n\n${COMPACTION_GUIDELINE}` }

const isMainCompaction = ({ trigger, agentId }: SessionCompactInput): boolean => trigger !== 'precompute' && agentId === undefined

// A compaction that replaced the main transcript resets the context the ramp reads and the repeat
// window (re-reading what the summary dropped is not a repeat); the recovery floor survives it.
function noteCompaction(input: SessionCompactInput, result: SessionCompactResult): SessionCompactResult {
  // `messages` stays inline: the check narrows `result` to the variant that carries `tokensAfter`.
  if (!isMainCompaction(input) || result.messages === undefined) {
    return result
  }
  runtime.manager.contextTokens = result.tokensAfter ?? 0
  clearRing(runtime.manager.recentCalls)
  return result
}

// The step goes out with its planned effort (the same event when unchanged); its response's usage
// becomes the next step's context size. The plan is awaited only when the model has no threshold yet.
async function* stepWithEffort(
  engine: EngineInterface,
  step: TurnStepInput,
  next: (input: TurnStepInput) => HookStream<TurnStepChunk, TurnStepResult>,
): AsyncGenerator<TurnStepChunk, TurnStepResult> {
  const planned = planEffort(engine, step)
  const effort = planned instanceof Promise ? await planned : planned
  const result = yield* next(effort === step.effort ? step : { ...step, effort })
  runtime.manager.contextTokens = contextAfter(step.agentId, result.usage)
  return result
}

// A redraw without a tag passes the engine's own event on, allocating nothing.
function withSpinnerTag<SpinnerRender extends { readonly props: { readonly suffix: string } }>(render: SpinnerRender): SpinnerRender {
  const tag = spinnerTag()
  return tag === undefined ? render : { ...render, props: { ...render.props, suffix: render.props.suffix + tag } }
}

// Runs an effect, then the rest of the chain. A synchronous effect continues at once, without the
// microtask an `await` would add to every turn completion.
function before<Value>(effect: () => unknown, rest: () => Promise<Value>): Promise<Value> {
  const pending = effect()
  return pending instanceof Promise ? pending.then(rest) : rest()
}

// Lets the rest of the chain run, then hands its result to an effect and passes it on unchanged.
async function after<Value>(result: Promise<Value>, effect: (value: Value) => unknown): Promise<Value> {
  const value = await result
  effect(value)
  return value
}

export const register: Register = (on, options) => {
  runtime.policy = policyFor(minimumOf(options))
  on('session.start', (engine, event, next) => before(() => startSession(engine), () => next(event)))
  // `session.start` fires once per process; an in-process `/clear`, `/resume` or fork starts a new
  // conversation that must not inherit this one's context size or failure evidence.
  on('classic.SessionStart', { source: ['clear', 'resume', 'fork'] }, (engine, event, next) =>
    before(() => resetSession(engine, true), () => next(event)),
  )
  on('command.run', { command: COMMAND }, async (engine, event) => ({ text: runAction(engine, event.args.trim() || ACTION_STATUS) }))
  on('command.run', { command: EFFORT_COMMAND }, (engine, event, next) =>
    after(next(event), () => observeEffortCommand(event.origin, event.args.trim())),
  )
  on('ui.render', { component: 'CommandOutput' }, (engine, event, next) =>
    isCompactionRow(event.props.command, event.props.args) ? EMPTY_ROW : next(event),
  )
  // The explicit type argument skips inferring a tool name across every tool's input type (the
  // generic overload of `next`), the costliest check in this file; it is erased at runtime.
  on('tool.call', async (engine, event, next) => noteToolResult(event, await next<string>(event)))
  on('turn.step', stepWithEffort)
  on('ui.render', { component: 'Spinner' }, (engine, event, next) => next(withSpinnerTag(event)))
  // The queued command runs only once the session is idle, so it is queued before the turn's
  // completion settles, without awaiting it.
  on('turn.complete', (engine, event, next) => before(() => completeTurn(engine, event.agentId), () => next(event)))
  on('session.compact', async (engine, event, next) => precomputeSkip(event.trigger) ?? noteCompaction(event, await next(guidedCompaction(event))))
}
