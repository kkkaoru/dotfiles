// This TypeScript file is executed by the Claude Code mod engine.
import type {
  EngineInterface,
  ModelEffort,
  PromptOrigin,
  Register,
  SessionCompactInput,
  SessionCompactResult,
  SessionCompactTrigger,
  SessionContextUsage,
  Settings,
  HookStream,
  TurnStepChunk,
  TurnStepInput,
  TurnStepResult,
} from 'claude-code'

// The session effort Claude Code holds, and whether a queued `/effort` has not run yet. One
// immutable record, replaced whole, so both always change together.
interface SessionEffort {
  readonly level: ModelEffort | undefined
  readonly pending: boolean
}

interface Manager {
  enabled: boolean
  lastSent: ModelEffort | undefined
  original: ModelEffort | undefined
  session: SessionEffort
  sessionId: string | undefined
  generation: number
  model: string | undefined
  turnId: string | undefined
  manualPasses: boolean
}

// A switch read back for a session: applied only while no newer reset owns the state.
interface StoredSwitch {
  readonly generation: number
  readonly sessionId: string
  readonly off: boolean
}

interface Runtime {
  manager: Manager
}

type DynamicAction = 'on' | 'off' | 'status'

type EffortMode = 'dynamic' | 'static'

type MissingValue = 'unknown' | 'none'

const LEVELS: readonly ModelEffort[] = ['low', 'medium', 'high', 'xhigh', 'max']
// The person's `/effort auto` (or `unset`) hands the level back to Claude Code; the next main step reports it.
const RESET_ARGS: ReadonlySet<string> = new Set(['auto', 'unset'])
// Compaction runs at the session effort and no hook can set the effort of that request alone, so
// the session rises to this level for the length of one compaction (README "How it works").
const COMPACTION_EFFORT: ModelEffort = 'xhigh'
// Only a level below it is raised: a person at `max` already compacts higher.
const RAISABLE: ReadonlySet<ModelEffort> = new Set(LEVELS.slice(0, LEVELS.indexOf(COMPACTION_EFFORT)))
// `/effort` is queued until the session is idle, so the mod compacts at a main turn end a little
// before the engine's threshold, which saves stopping the next turn.
// shortcut: the ratio is not tuned; raise it if sessions compact noticeably early.
const PREEMPT_RATIO = 0.9 satisfies number
// Sessions switched off with `/dynamic-effort off`, one store key each, so sessions switching at
// once never overwrite each other. A resumed session keeps its id and so its key; a new session's
// id has none, so it starts on. The store lists keys in insertion order, so only the newest are kept.
const OFF_KEY_PREFIX: string = 'off:'
const MAX_OFF_SESSIONS = 32 satisfies number
const COMMAND: string = 'dynamic-effort'
const EFFORT_COMMAND: string = 'effort'
const COMPACT_COMMAND: string = 'compact'
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
// The sent effort is drawn at the end of the hint line under the prompt (the permission mode's
// row), so it takes no row of its own. Hot-path texts are built once per level, not per redraw.
const HINT_TAILS: ReadonlyMap<ModelEffort, string> = new Map(LEVELS.map(level => [level, `🧠 ${level}`]))
const STATIC_TAIL: string = `🧠 ${MODE_STATIC}`
// The hint after the permission-mode label starts with `(shift+tab to cycle)`; the mode label itself
// (`⏵⏵ bypass permissions on`) is the engine's and out of reach. The key hint and the separator it
// leaves at the start are dropped.
const CYCLE_HINT = / ?\(shift\+tab to cycle\)/u
const LEADING_SEPARATOR = /^ ?· /u
// The engine draws its own separator before the tail; one between tails keeps plugins apart.
const TAIL_SEPARATOR: string = ' · '
const PRECOMPUTE_SKIP: string = 'dynamic-effort: compaction effort is not applied yet'
const RERUN_SKIP: string = `dynamic-effort: compacting again at ${COMPACTION_EFFORT} effort`
const CONTINUE_PROMPT: string = 'The conversation was compacted while you were working. Continue the task from where you stopped.'
// Claude Code's summary keeps errors and fixes but not abandoned approaches; losing what failed
// invites repeating it (step repetition, MAST arXiv:2503.13657), and a compression guideline that
// names what must survive keeps accuracy at a fraction of the tokens (ACON arXiv:2510.00615).
const COMPACTION_GUIDELINE: string =
  'Also list approaches that were tried and abandoned, with the error or reason, so they are not repeated.'
const EFFORT_NAMES: ReadonlySet<string> = new Set(LEVELS)
const ACTION_NAMES: ReadonlySet<string> = new Set(ACTIONS)
const REQUEUE_FAILED: string = `dynamic-effort: could not compact at ${COMPACTION_EFFORT} effort; run /compact again`
const RERUN_TRIGGERS: ReadonlySet<SessionCompactTrigger> = new Set<SessionCompactTrigger>(['manual', 'auto'])
const UNKNOWN_SESSION: SessionEffort = { level: undefined, pending: false }

// shortcut: module state restarts on a hot reload (development only); move to
// the engine's `state` with a contract if reload persistence starts to matter.
const fresh = (generation: number, original: ModelEffort | undefined): Manager => ({
  enabled: true,
  lastSent: undefined,
  original,
  session: UNKNOWN_SESSION,
  sessionId: undefined,
  generation,
  model: undefined,
  turnId: undefined,
  manualPasses: false,
})

// The live state, replaced by reference on a reset: nothing is copied field by field.
const runtime: Runtime = { manager: fresh(0, undefined) }

const isAction = (value: string): value is DynamicAction => ACTION_NAMES.has(value)

const isEffort = (value: string): value is ModelEffort => EFFORT_NAMES.has(value)

// Starts a session's state over. The on/off switch is awaited, so no step is sent before it
// applies. An in-process `/clear`, `/resume` or fork keeps the person's own effort, which outlives
// the conversation.
async function resetSession(engine: EngineInterface, keepOriginal: boolean): Promise<void> {
  const generation = runtime.manager.generation + 1
  runtime.manager = fresh(generation, keepOriginal ? runtime.manager.original : undefined)
  // A store that cannot be read fails the start hook, which the engine reports; the session stays on.
  await restoreSwitch(engine, generation)
}

const offKey = (sessionId: string): string => `${OFF_KEY_PREFIX}${sessionId}`

async function restoreSwitch(engine: EngineInterface, generation: number): Promise<void> {
  const sessionId = await engine.session.id()
  applySwitch({ generation, sessionId, off: (await engine.store.get(offKey(sessionId))) === true })
}

function applySwitch({ generation, sessionId, off }: StoredSwitch): void {
  if (runtime.manager.generation !== generation) {
    return
  }
  runtime.manager.sessionId = sessionId
  if (off) {
    runtime.manager.enabled = false
  }
}

// Deleting first moves a session switched off again to the newest end; the oldest beyond the bound
// are then dropped, so the store does not grow with the number of sessions.
// Each old key is deleted on its own, so one failed delete does not stop the others.
const storeOff = (engine: EngineInterface, key: string): Promise<void> =>
  engine.store
    .delete(key)
    .then(() => engine.store.set(key, true))
    .then(() => engine.store.keys())
    .then(keys =>
      settleAll(
        'pruning old switches',
        keys
          .filter(listed => listed.startsWith(OFF_KEY_PREFIX))
          .slice(0, -MAX_OFF_SESSIONS)
          .map(listed => engine.store.delete(listed)),
      ),
    )

// A failed write loses the switch on resume and is logged.
function rememberSwitch(engine: EngineInterface, enabled: boolean): void {
  const sessionId = runtime.manager.sessionId
  if (sessionId === undefined) {
    return
  }
  void (enabled ? engine.store.delete(offKey(sessionId)) : storeOff(engine, offKey(sessionId))).catch(reportFailure(engine, 'saving the switch'))
}

// `engine.command.run` is refused inside a command hook, so commands are queued at a main turn end
// or from a timer, and run once the session is idle. A hook's command is never awaited (that would
// deadlock a turn). A command that ran set the level (observed by the `/effort` hook); a refused
// or dropped one leaves it, and the next turn end compares the levels again.
function queueSessionEffort(engine: EngineInterface, level: ModelEffort): void {
  runtime.manager.session = { level: runtime.manager.session.level, pending: true }
  void engine.command.run({ command: EFFORT_COMMAND, args: level }).then(
    () => settleCommand(level),
    (error: unknown) => refuseCommand(engine, `/${EFFORT_COMMAND} ${level}`, error),
  )
}

// A refused command leaves the level as it was, so the next main turn end compares them again.
function refuseCommand(engine: EngineInterface, command: string, error: unknown): void {
  settleCommand(runtime.manager.session.level)
  reportFailure(engine, command)(error)
}

function settleCommand(level: ModelEffort | undefined): void {
  runtime.manager.session = { level, pending: false }
}

// A level is known and below the compaction level, and nothing the mod queued is still waiting.
const canRaiseSession = (): boolean =>
  runtime.manager.original !== undefined && RAISABLE.has(runtime.manager.original) && !runtime.manager.session.pending

// The session sits at the compaction level the person did not choose: a restore that was dropped.
const strandedAtCompactionLevel = (): boolean => canRaiseSession() && runtime.manager.session.level === COMPACTION_EFFORT

// Raise, compact, restore: three separate commands queued at once, so the restore is already in
// the engine's queue before the level rises, and a skipped or failed compaction still ends at the original.
function queueCompaction(engine: EngineInterface, original: ModelEffort, instructions: string): void {
  runtime.manager.session = { level: runtime.manager.session.level, pending: true }
  void engine.command.run({ command: EFFORT_COMMAND, args: COMPACTION_EFFORT }).catch(reportFailure(engine, `/${EFFORT_COMMAND} ${COMPACTION_EFFORT}`))
  void engine.command.run({ command: COMPACT_COMMAND, args: instructions }).catch((error: unknown) => allowOneCompaction(engine, error))
  queueSessionEffort(engine, original)
}

// `/effort <level>` also saves the level as the default (only `max` is session-only), so the raise
// saves the compaction level until the restore puts the original back. The sequence is used only when it rewrites what the user settings already hold, for the session's
// model too. A level from `--effort` or another source is never saved by the mod.
const field = (value: unknown, key: string): unknown =>
  typeof value === 'object' && value !== null && !Array.isArray(value) ? Reflect.get(value, key) : undefined

const modelEffortLevel = (settings: Settings): unknown =>
  runtime.manager.model === undefined ? undefined : field(field(settings.modelSettings, runtime.manager.model), 'effortLevel')

const restoreSavesNothing = (settings: Settings): boolean =>
  settings.effortLevel === runtime.manager.original && (modelEffortLevel(settings) ?? runtime.manager.original) === runtime.manager.original

// An unreadable settings file rejects: the hook that read it fails, the engine reports it, and the
// compaction runs at the session effort as if the mod were not there.
const userSettings = (engine: EngineInterface): Promise<Settings> => engine.settings.read({ source: 'user' })

// A refused re-run must not swallow the compaction: the next one runs at the session effort.
function allowOneCompaction(engine: EngineInterface, error: unknown): void {
  runtime.manager.manualPasses = true
  engine.ui.toast(REQUEUE_FAILED)
  reportFailure(engine, `/${COMPACT_COMMAND}`)(error)
}

const queueIfRaisable = (engine: EngineInterface, instructions: string): void =>
  canRaiseSession() && runtime.manager.original !== undefined ? queueCompaction(engine, runtime.manager.original, instructions) : undefined

// Only an engine that auto-compacts is compacted ahead of time. `tokens` is the last response's
// input alone; a step's own usage sums the responses of a call, which an advisor call doubles.
const autoCompactDue = ({ breakdown, tokens }: SessionContextUsage): boolean =>
  breakdown?.autoCompactThreshold !== undefined && (tokens ?? breakdown.totalTokens) >= breakdown.autoCompactThreshold * PREEMPT_RATIO

const preemptIfSaved = (engine: EngineInterface, settings: Settings): void =>
  restoreSavesNothing(settings) ? queueIfRaisable(engine, '') : undefined

const preemptIfDue = (engine: EngineInterface, context: SessionContextUsage): Promise<void> | undefined =>
  autoCompactDue(context) ? userSettings(engine).then(settings => preemptIfSaved(engine, settings)) : undefined

// A dropped restore is queued again; otherwise a dynamic session near the threshold compacts now.
function completeMainTurn(engine: EngineInterface): Promise<void> | undefined {
  runtime.manager.turnId = undefined
  if (strandedAtCompactionLevel() && runtime.manager.original !== undefined) {
    queueSessionEffort(engine, runtime.manager.original)
    return undefined
  }
  return runtime.manager.enabled && canRaiseSession()
    ? engine.session.usage({ breakdown: 'summary' }).then(({ context }) => preemptIfDue(engine, context))
    : undefined
}

const errorText = (error: unknown): string => (error instanceof Error ? error.message : String(error))

const rejectionOf = (result: PromiseSettledResult<unknown>): unknown[] => (result.status === 'rejected' ? [result.reason] : [])

// Independent tasks all run to the end, so one failure neither stops nor hides another; every
// failure is then rethrown together, never dropped.
async function settleAll(action: string, tasks: readonly Promise<unknown>[]): Promise<void> {
  const failures = (await Promise.allSettled(tasks)).flatMap(rejectionOf)
  if (failures.length > 0) {
    throw new AggregateError(failures, `${action} failed: ${failures.map(errorText).join('; ')}`)
  }
}

// Work no hook awaits has no engine to report its failure, so each such failure is logged with
// what failed; work a hook awaits rejects instead, and the engine reports the hook and skips it.
// No failure is retried on the spot: every call here goes to the local engine, so an immediate
// retry meets the same cause, could compact twice or stack effort changes. A dropped restore is
// queued again at the next main turn end, an undeleted old switch is pruned at the next
// `/dynamic-effort off`, and a refused `/compact` re-run lets the next compaction through.
// A failed `/dynamic-effort` registration and a failed save of the off switch stay unrepaired.
const reportFailure =
  (engine: EngineInterface, action: string) =>
  (error: unknown): void =>
    engine.ui.log(`${PLUGIN}: ${action} failed: ${errorText(error)}`)

const isOwnEcho = (origin: PromptOrigin): boolean =>
  origin.kind === 'plugin' && origin.name === PLUGIN

const dynamicTail = (): string | undefined =>
  runtime.manager.lastSent === undefined ? undefined : HINT_TAILS.get(runtime.manager.lastSent)

const effortTail = (): string | undefined => (runtime.manager.enabled ? dynamicTail() : STATIC_TAIL)

// Joins two parts of the hint line; an absent or empty part adds no separator.
const joined = (head: string | undefined, tail: string | undefined): string | undefined =>
  head === undefined || head === '' || tail === undefined ? (tail ?? head) : `${head}${TAIL_SEPARATOR}${tail}`

// Another plugin's tail is kept ahead of this one. Only a line with the key hint is rewritten, the
// tails joined into it since a rewritten hint replaces the whole line; any other keeps the engine's
// line and gets the tail, and one with no tail to add passes as it is.
function withShortHint<HintRender extends { readonly props: { readonly hint: string; readonly tail?: string } }>(render: HintRender): HintRender {
  const hint = render.props.hint.replace(CYCLE_HINT, '').replace(LEADING_SEPARATOR, '')
  const tail = joined(render.props.tail, effortTail())
  if (hint !== render.props.hint) {
    return { ...render, props: { ...render.props, hint: joined(hint, tail) ?? '', tail: undefined } }
  }
  return tail === render.props.tail ? render : { ...render, props: { ...render.props, tail } }
}

const status = (): string =>
  [
    `${MODE_DYNAMIC}=${runtime.manager.enabled ? ACTION_ON : ACTION_OFF}`,
    `original=${runtime.manager.original ?? UNKNOWN} session=${runtime.manager.session.level ?? UNKNOWN} pending=${runtime.manager.session.pending}`,
    `last=${runtime.manager.lastSent ?? NONE}`,
  ].join(' ')

// Resetting the state and registering the command are independent: a store that cannot be read
// must not abandon the registration, so both finish before the hook reports a failure.
const startSession = (engine: EngineInterface): Promise<void> =>
  settleAll('starting the session', [
    resetSession(engine, false),
    engine.command.register({
      name: COMMAND,
      description: `Compaction at ${COMPACTION_EFFORT}, every other request at your effort: ${ACTIONS.join(' | ')}`,
      argumentHint: ACTION_USAGE,
    }),
  ])

function runAction(engine: EngineInterface, action: string): string {
  if (action === ACTION_ON || action === ACTION_OFF) {
    runtime.manager.enabled = action === ACTION_ON
    rememberSwitch(engine, action === ACTION_ON)
  }
  return isAction(action) ? status() : `Usage: /${COMMAND} ${ACTION_USAGE}`
}

// Every `/effort` that ran sets the session level. The person's also becomes the original. `auto`
// clears the original, since the next main step reports the level Claude Code then holds.
function observeEffortCommand(origin: PromptOrigin, level: string): void {
  const isOwn = isOwnEcho(origin)
  if (!isOwn && RESET_ARGS.has(level)) {
    runtime.manager.original = undefined
  }
  if (!isEffort(level)) {
    return
  }
  runtime.manager.session = { level, pending: runtime.manager.session.pending }
  if (!isOwn) {
    runtime.manager.original = level
  }
}

// A main step's effort is the level the session holds. Any level below the compaction level is the
// person's, however it was set; the compaction level is theirs only through `/effort xhigh`, since the
// mod raises the session to it. Each field is written only on a change, so a step allocates nothing.
function recordMainStep({ model, effort }: TurnStepInput): void {
  if (typeof effort !== 'string') {
    return
  }
  if (model !== runtime.manager.model) {
    runtime.manager.model = model
  }
  if (effort !== runtime.manager.session.level) {
    runtime.manager.session = { level: effort, pending: runtime.manager.session.pending }
  }
  if (effort !== COMPACTION_EFFORT && effort !== runtime.manager.original) {
    runtime.manager.original = effort
  }
}

const lastSentOf = (effort: TurnStepInput['effort']): ModelEffort | undefined => (typeof effort === 'string' ? effort : undefined)

// Written only on a change; the hint line reads it when it is next drawn, with no host call.
function noteSent(effort: TurnStepInput['effort']): void {
  if (lastSentOf(effort) !== runtime.manager.lastSent) {
    runtime.manager.lastSent = lastSentOf(effort)
  }
}

// The session is at the compaction level, or on its way there, for the mod's sequence.
const isRaised = ({ session }: Manager): boolean => session.pending || session.level === COMPACTION_EFFORT

// Only the compaction runs at the raised level: a step that inherits it, such as a background
// subagent's during the sequence, is sent at the person's level. `/effort xhigh` makes that level xhigh,
// and outside the sequence an agent keeps an xhigh of its own.
const sentEffort = (effort: TurnStepInput['effort']): TurnStepInput['effort'] =>
  effort === COMPACTION_EFFORT && isRaised(runtime.manager) ? (runtime.manager.original ?? effort) : effort

function planEffort(step: TurnStepInput): TurnStepInput['effort'] {
  if (step.agentId !== undefined) {
    return sentEffort(step.effort)
  }
  recordMainStep(step)
  noteSent(sentEffort(step.effort))
  return sentEffort(step.effort)
}

// The step goes out unchanged, the same event, unless it inherits the raised level.
async function* stepWithEffort(
  _engine: EngineInterface,
  step: TurnStepInput,
  next: (input: TurnStepInput) => HookStream<TurnStepChunk, TurnStepResult>,
): AsyncGenerator<TurnStepChunk, TurnStepResult> {
  const effort = planEffort(step)
  return yield* next(effort === step.effort ? step : { ...step, effort })
}

// The person's `/compact` focus leads; the guideline follows it. Subagent compactions are left alone.
const guidedCompaction = (input: SessionCompactInput): SessionCompactInput =>
  input.agentId !== undefined
    ? input
    : { ...input, instructions: input.instructions === undefined ? COMPACTION_GUIDELINE : `${input.instructions}\n\n${COMPACTION_GUIDELINE}` }

// Dynamic mode may raise the session to the compaction level, and it is not there yet.
const compactionLevelPending = (): boolean =>
  runtime.manager.enabled && canRaiseSession() && runtime.manager.session.level !== COMPACTION_EFFORT

// A precompute made below the compaction level would be reused at the old effort; one in a session
// that will never be raised (the restore would change the saved default) is kept.
const precomputeSkip = async (engine: EngineInterface, trigger: SessionCompactTrigger): Promise<SessionCompactResult | undefined> =>
  trigger === 'precompute' && compactionLevelPending() && restoreSavesNothing(await userSettings(engine)) ? { skip: PRECOMPUTE_SKIP } : undefined

// A main compaction below the compaction level is run again inside the mod's sequence; its own
// re-run arrives at the compaction level, or while the sequence is pending.
const rerunsCompaction = ({ trigger, agentId }: SessionCompactInput): boolean =>
  RERUN_TRIGGERS.has(trigger) && agentId === undefined && compactionLevelPending()

// A refused re-run lets exactly one later compaction through at the session effort.
const passesCompaction = (input: SessionCompactInput): boolean => !rerunsCompaction(input) || consumeManualPass()

function consumeManualPass(): boolean {
  const passes = runtime.manager.manualPasses
  runtime.manager.manualPasses = false
  return passes
}

// The sequence, then the prompt that resumes a stopped turn. The engine runs them in order once idle.
function queueRerun(engine: EngineInterface, instructions: string, resumes: boolean): void {
  queueIfRaisable(engine, instructions)
  if (resumes) {
    void engine.prompt.submit({ text: CONTINUE_PROMPT }).catch(reportFailure(engine, 'resuming the turn'))
  }
}

// `/effort` waits for an idle session, so an auto-compaction inside a turn stops the turn first: the
// tool results are already in the transcript, and no interruption marker is left. The skip ends the
// hook, so the timer queues the sequence outside the command the turn waits on. A failed stop
// rejects, and the engine reports it and compacts at the session effort.
async function rerunAtCompactionLevel(engine: EngineInterface, instructions: string | undefined): Promise<SessionCompactResult> {
  const turnId = runtime.manager.turnId
  if (turnId !== undefined) {
    await engine.turn.abort({ turnId })
  }
  engine.clock.after(0, () => queueRerun(engine, instructions ?? '', turnId !== undefined))
  return { skip: RERUN_SKIP }
}

// A restore that would change the saved default leaves the compaction at the session effort.
const rerunIfSaved = (engine: EngineInterface, input: SessionCompactInput, settings: Settings): Promise<SessionCompactResult> | undefined =>
  restoreSavesNothing(settings) ? rerunAtCompactionLevel(engine, input.instructions) : undefined

const compactionSkip = async (engine: EngineInterface, input: SessionCompactInput): Promise<SessionCompactResult | undefined> =>
  passesCompaction(input) ? undefined : rerunIfSaved(engine, input, await userSettings(engine))

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

export const register: Register = on => {
  on('session.start', (engine, event, next) => before(() => startSession(engine), () => next(event)))
  // `session.start` fires once per process; an in-process `/clear`, `/resume` or fork starts a new
  // conversation with its own switch.
  on('classic.SessionStart', { source: ['clear', 'resume', 'fork'] }, (engine, event, next) =>
    before(() => resetSession(engine, true), () => next(event)),
  )
  on('command.run', { command: COMMAND }, async (engine, event) => ({ text: runAction(engine, event.args.trim() || ACTION_STATUS) }))
  on('command.run', { command: EFFORT_COMMAND }, (_engine, event, next) =>
    after(next(event), () => observeEffortCommand(event.origin, event.args.trim())),
  )
  on('turn.start', (_engine, event, next) => {
    if (event.agentId === undefined) {
      runtime.manager.turnId = event.turnId
    }
    return next(event)
  })
  on('turn.step', stepWithEffort)
  on('ui.render', { component: 'PromptHint' }, (_engine, event, next) => next(withShortHint(event)))
  // The queued command runs only once the session is idle, so it is queued before the turn's
  // completion settles, without awaiting it.
  on('turn.complete', (engine, event, next) => before(() => (event.agentId === undefined ? completeMainTurn(engine) : undefined), () => next(event)))
  on('session.compact', async (engine, event, next) =>
    (await precomputeSkip(engine, event.trigger)) ?? (await compactionSkip(engine, event)) ?? next(guidedCompaction(event)),
  )
}
