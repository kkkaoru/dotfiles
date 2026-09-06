import { pathToFileURL } from "node:url";

// Pi 0.85.1 stores each compaction controller in one mutable session field. Overlapping runs can
// replace or clear that field across an await and crash on the next `.signal` read. Keep the public
// behavior single-flight until Pi ships per-run controllers; stop applying the guard automatically
// once those vulnerable field reads disappear.
const PATCH_MARKER = Symbol.for("kkkaoru.pi.compaction-singleflight");
const STATUS_RENDER_PATCH_MARKER = Symbol.for("kkkaoru.pi.large-session-status-render-throttle");
const EXTENSION_UI_DEDUPE_PATCH_MARKER = Symbol.for("kkkaoru.pi.extension-ui-render-dedupe");
const FOOTER_RENDER_CACHE_PATCH_MARKER = Symbol.for("kkkaoru.pi.footer-render-cache");
const COMPACTION_PREPARATION_PATCH_MARKER = Symbol.for(
  "kkkaoru.pi.compaction-preparation-reliability",
);
const COMPACTION_EVENT_PATCH_MARKER = Symbol.for("kkkaoru.pi.compaction-event-reliability");
const LATEST_COMPACTION_EVENT_PATCH_MARKER = Symbol.for("kkkaoru.pi.latest-compaction-event-entry");
const COMPACT_COMMAND_PATCH_MARKER = Symbol.for("kkkaoru.pi.compact-command-visibility");
const ACTIVE_COMPACTIONS = new WeakSet();
const AUTO_COMPACTIONS_VISIBLE_TO_CORE = new WeakSet();
const FOOTER_RENDER_CACHE = new WeakMap();
const AUTO_CONTROLLER_ACCESS = "this._autoCompactionAbortController.signal";
const MANUAL_CONTROLLER_ACCESS = "this._compactionAbortController.signal";
const ALREADY_RUNNING_MESSAGE =
  "Compaction is already in progress. Wait for it to finish and retry.";
const FOOTER_RENDER_CACHE_TTL_MS = 1_000;
const LARGE_SESSION_ENTRY_THRESHOLD = 10_000;
const LARGE_SESSION_STATUS_INTERVAL_MS = 750;
const COMPACTION_SUMMARY_INSTRUCTIONS =
  "Keep the resulting summary concise and under 4,000 words. Consolidate duplicate or outdated details instead of repeating them. Preserve current goals, constraints, unresolved work, exact file paths, function names, error messages, and decisions needed to continue.";
const SPLIT_TURN_SUMMARY_INSTRUCTIONS =
  "The final messages are the prefix of the currently retained turn. Clearly preserve their active turn context so the retained suffix remains understandable.";
const THROTTLED_STATUS_KINDS = new Set(["branchSummary", "compaction", "working"]);
const COMPACTION_LIFECYCLE_EVENTS = new Set(["compaction_start", "compaction_end"]);

function methodDescriptor(prototype, name) {
  const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
  if (descriptor === undefined || typeof descriptor.value !== "function") {
    throw new Error(`Pi compaction guard: ${name}() is unavailable.`);
  }
  return descriptor;
}

function compactionGetter(prototype) {
  const descriptor = Object.getOwnPropertyDescriptor(prototype, "isCompacting");
  if (descriptor === undefined || typeof descriptor.get !== "function") {
    throw new Error("Pi compaction guard: isCompacting getter is unavailable.");
  }
  return descriptor;
}

function hasVulnerableControllerAccess(autoDescriptor, manualDescriptor) {
  return (
    Function.prototype.toString.call(autoDescriptor.value).includes(AUTO_CONTROLLER_ACCESS) &&
    Function.prototype.toString.call(manualDescriptor.value).includes(MANUAL_CONTROLLER_ACCESS)
  );
}

async function runExclusive({ session, operation, visibleToCore }) {
  ACTIVE_COMPACTIONS.add(session);
  if (visibleToCore) {
    AUTO_COMPACTIONS_VISIBLE_TO_CORE.add(session);
  }
  try {
    return await operation();
  } finally {
    AUTO_COMPACTIONS_VISIBLE_TO_CORE.delete(session);
    ACTIVE_COMPACTIONS.delete(session);
    session._resolveIdleWaitIfIdle?.();
  }
}

export async function waitForCancellation(promise, signal) {
  if (signal === undefined) return promise;
  const cancelled = Promise.withResolvers();
  const abort = () => cancelled.reject(new DOMException("Compaction cancelled", "AbortError"));
  signal.addEventListener("abort", abort, { once: true });
  if (signal.aborted) abort();
  try {
    const result = await Promise.race([promise, cancelled.promise]);
    signal.throwIfAborted();
    return result;
  } finally {
    signal.removeEventListener("abort", abort);
  }
}

function yieldToPaint() {
  return new Promise((resolve) => setImmediate(resolve));
}

function announceManualCompactionStart(session) {
  session._emit?.({ type: "compaction_start", reason: "manual" });
  return yieldToPaint();
}

async function runAutoWithCancellation(session, operation) {
  if (typeof session._getSummarizationRequestAuth !== "function") return operation();
  const authDescriptor = Object.getOwnPropertyDescriptor(session, "_getSummarizationRequestAuth");
  const controllerDescriptor = Object.getOwnPropertyDescriptor(
    session,
    "_autoCompactionAbortController",
  );
  const auth = session._getSummarizationRequestAuth;
  const controller = new AbortController();
  const state = { controller };
  // Preserve one signal across auth, start notifications, and core controller assignment.
  Object.defineProperty(session, "_autoCompactionAbortController", {
    configurable: true,
    get: () => state.controller,
    set: (value) => {
      state.controller = value === undefined ? undefined : controller;
    },
  });
  Object.defineProperty(session, "_getSummarizationRequestAuth", {
    configurable: true,
    value: (...args) =>
      waitForCancellation(
        Promise.resolve().then(() => auth.apply(session, args)),
        controller.signal,
      ),
  });
  try {
    return await operation();
  } finally {
    if (authDescriptor)
      Object.defineProperty(session, "_getSummarizationRequestAuth", authDescriptor);
    else delete session._getSummarizationRequestAuth;
    if (controllerDescriptor)
      Object.defineProperty(session, "_autoCompactionAbortController", controllerDescriptor);
    else delete session._autoCompactionAbortController;
  }
}

async function runManualWithCancellation(session, operation) {
  if (typeof session._getSummarizationRequestAuth !== "function") return operation();
  const descriptor = Object.getOwnPropertyDescriptor(session, "_getSummarizationRequestAuth");
  const auth = session._getSummarizationRequestAuth;
  Object.defineProperty(session, "_getSummarizationRequestAuth", {
    configurable: true,
    value: (...args) =>
      waitForCancellation(
        Promise.resolve().then(() => auth.apply(session, args)),
        session._compactionAbortController.signal,
      ),
  });
  try {
    return await operation();
  } finally {
    if (descriptor) Object.defineProperty(session, "_getSummarizationRequestAuth", descriptor);
    else delete session._getSummarizationRequestAuth;
  }
}

function validateSummary(result) {
  if (
    typeof result?.summary !== "string" ||
    !result.summary.replace(/<(read-files|modified-files)>[\s\S]*?<\/\1>/g, "").trim()
  ) {
    throw new Error("Compaction returned an empty summary; checkpoint was not saved");
  }
}

function guardedCompactionContext(session) {
  const stream = session.agent?.streamFunction;
  if (typeof stream !== "function") return session;
  const guardedStream = async (...args) => {
    const output = await stream(...args);
    return {
      result: async () => {
        const response = await output.result();
        if (response.stopReason === "aborted") {
          throw new DOMException("Compaction cancelled", "AbortError");
        }
        return response;
      },
    };
  };
  // Override only the generator's stream; do not change the live agent's provider binding.
  const context = Object.create(session);
  Object.defineProperty(context, "agent", {
    value: new Proxy(session.agent, {
      get: (target, key) =>
        key === "streamFunction" ? guardedStream : Reflect.get(target, key, target),
    }),
  });
  return context;
}

export function installCompactionSingleFlight(AgentSession) {
  const prototype = AgentSession.prototype;
  if (prototype[PATCH_MARKER] === true) {
    return "already-installed";
  }

  const autoDescriptor = methodDescriptor(prototype, "_runAutoCompaction");
  const manualDescriptor = methodDescriptor(prototype, "compact");
  const getterDescriptor = compactionGetter(prototype);
  if (!hasVulnerableControllerAccess(autoDescriptor, manualDescriptor)) {
    return "not-needed";
  }

  const originalAutoCompaction = autoDescriptor.value;
  const originalManualCompaction = manualDescriptor.value;
  const originalIsCompacting = getterDescriptor.get;

  Object.defineProperty(prototype, "isCompacting", {
    ...getterDescriptor,
    get() {
      return AUTO_COMPACTIONS_VISIBLE_TO_CORE.has(this) || originalIsCompacting.call(this);
    },
  });
  Object.defineProperty(prototype, "_runAutoCompaction", {
    ...autoDescriptor,
    async value(...args) {
      if (ACTIVE_COMPACTIONS.has(this) || originalIsCompacting.call(this)) {
        return false;
      }
      return runExclusive({
        session: this,
        operation: () =>
          runAutoWithCancellation(this, () => originalAutoCompaction.apply(this, args)),
        visibleToCore: true,
      });
    },
  });
  Object.defineProperty(prototype, "compact", {
    ...manualDescriptor,
    async value(...args) {
      if (ACTIVE_COMPACTIONS.has(this) || originalIsCompacting.call(this)) {
        throw new Error(ALREADY_RUNNING_MESSAGE);
      }
      return runExclusive({
        session: this,
        operation: async () => {
          await announceManualCompactionStart(this);
          return runManualWithCancellation(this, () => originalManualCompaction.apply(this, args));
        },
        visibleToCore: false,
      });
    },
  });
  Object.defineProperty(prototype, PATCH_MARKER, { value: true });
  return "installed";
}

function compactPreparation(preparation) {
  const messagesToSummarize = preparation.isSplitTurn
    ? [...preparation.messagesToSummarize, ...preparation.turnPrefixMessages]
    : preparation.messagesToSummarize;
  return {
    ...preparation,
    isSplitTurn: false,
    messagesToSummarize,
    turnPrefixMessages: [],
  };
}

function compactInstructions(customInstructions, isSplitTurn) {
  const reliabilityInstructions = isSplitTurn
    ? `${SPLIT_TURN_SUMMARY_INSTRUCTIONS}\n\n${COMPACTION_SUMMARY_INSTRUCTIONS}`
    : COMPACTION_SUMMARY_INSTRUCTIONS;
  return customInstructions === undefined
    ? reliabilityInstructions
    : `${customInstructions}\n\n${reliabilityInstructions}`;
}

export function installCompactionPreparationReliability(AgentSession) {
  const prototype = AgentSession.prototype;
  if (prototype[COMPACTION_PREPARATION_PATCH_MARKER] === true) {
    return "already-installed";
  }
  const descriptor = Object.getOwnPropertyDescriptor(prototype, "_runDefaultCompaction");
  if (descriptor === undefined || typeof descriptor.value !== "function") {
    return "not-needed";
  }
  const original = descriptor.value;

  Object.defineProperty(prototype, "_runDefaultCompaction", {
    ...descriptor,
    async value(
      preparation,
      requestModel,
      apiKey,
      headers,
      customInstructions,
      signal,
      env,
      reason,
    ) {
      try {
        if (signal?.aborted) throw new DOMException("Compaction cancelled", "AbortError");
        const result = await waitForCancellation(
          original.call(
            guardedCompactionContext(this),
            compactPreparation(preparation),
            requestModel,
            apiKey,
            headers,
            compactInstructions(customInstructions, preparation.isSplitTurn),
            signal,
            env,
            reason,
          ),
          signal,
        );
        if (signal?.aborted) throw new DOMException("Compaction cancelled", "AbortError");
        validateSummary(result);
        return result;
      } catch (error) {
        if (signal?.aborted) throw new DOMException("Compaction cancelled", "AbortError");
        throw error;
      }
    },
  });
  Object.defineProperty(prototype, COMPACTION_PREPARATION_PATCH_MARKER, { value: true });
  return "installed";
}

function normalizedCompactionFailure(session, event) {
  if (
    event?.result !== undefined ||
    event?.reason === "manual" ||
    event?.aborted === true ||
    (session._autoCompactionAbortController?.signal.aborted !== true &&
      !event?.errorMessage?.endsWith(": Compaction cancelled"))
  ) {
    return event;
  }
  return { ...event, aborted: true, errorMessage: undefined, willRetry: false };
}

function reportCompactionListenerError(error) {
  const message = error instanceof Error ? (error.stack ?? error.message) : String(error);
  console.error(`Pi compaction lifecycle listener failed: ${message}`);
}

function notifyCompactionListener(listener, event) {
  try {
    Promise.resolve(listener(event)).catch(reportCompactionListenerError);
  } catch (error) {
    reportCompactionListenerError(error);
  }
}

export function installCompactionEventReliability(AgentSession) {
  const prototype = AgentSession.prototype;
  if (prototype[COMPACTION_EVENT_PATCH_MARKER] === true) {
    return "already-installed";
  }
  const emitDescriptor = Object.getOwnPropertyDescriptor(prototype, "_emit");
  const failedDescriptor = Object.getOwnPropertyDescriptor(prototype, "_emitSessionCompactFailed");
  if (
    emitDescriptor === undefined ||
    typeof emitDescriptor.value !== "function" ||
    failedDescriptor === undefined ||
    typeof failedDescriptor.value !== "function"
  ) {
    return "not-needed";
  }
  const originalEmit = emitDescriptor.value;
  const originalEmitFailed = failedDescriptor.value;

  Object.defineProperty(prototype, "_emit", {
    ...emitDescriptor,
    value(event) {
      const normalizedEvent =
        event?.type === "compaction_end" ? normalizedCompactionFailure(this, event) : event;
      if (
        COMPACTION_LIFECYCLE_EVENTS.has(normalizedEvent?.type) &&
        Array.isArray(this._eventListeners)
      ) {
        this._eventListeners
          .slice()
          .forEach((listener) => notifyCompactionListener(listener, normalizedEvent));
        return undefined;
      }
      try {
        return originalEmit.call(this, normalizedEvent);
      } catch (error) {
        if (!COMPACTION_LIFECYCLE_EVENTS.has(normalizedEvent?.type)) {
          throw error;
        }
        reportCompactionListenerError(error);
        return undefined;
      }
    },
  });
  Object.defineProperty(prototype, "_emitSessionCompactFailed", {
    ...failedDescriptor,
    value(event) {
      return originalEmitFailed.call(this, normalizedCompactionFailure(this, event));
    },
  });
  Object.defineProperty(prototype, COMPACTION_EVENT_PATCH_MARKER, { value: true });
  return "installed";
}

export function installCompactCommandVisibility(InteractiveMode) {
  const prototype = InteractiveMode.prototype;
  if (prototype[COMPACT_COMMAND_PATCH_MARKER] === true) {
    return "already-installed";
  }
  const descriptor = Object.getOwnPropertyDescriptor(prototype, "handleCompactCommand");
  if (descriptor === undefined || typeof descriptor.value !== "function") {
    return "not-needed";
  }

  Object.defineProperty(prototype, "handleCompactCommand", {
    ...descriptor,
    async value(customInstructions) {
      this.session?._emit?.({ type: "compaction_start", reason: "manual" });
      this.ui?.requestRender?.();
      await yieldToPaint();
      try {
        await this.session.compact(customInstructions);
      } catch {
        // Ignore, will be emitted as an event
      }
    },
  });
  Object.defineProperty(prototype, COMPACT_COMMAND_PATCH_MARKER, { value: true });
  return "installed";
}

export function installLatestCompactionEventEntry(ExtensionRunner) {
  const prototype = ExtensionRunner.prototype;
  if (prototype[LATEST_COMPACTION_EVENT_PATCH_MARKER] === true) {
    return "already-installed";
  }
  const descriptor = Object.getOwnPropertyDescriptor(prototype, "emit");
  if (descriptor === undefined || typeof descriptor.value !== "function") {
    return "not-needed";
  }
  const originalEmit = descriptor.value;

  Object.defineProperty(prototype, "emit", {
    ...descriptor,
    async value(event) {
      if (event?.type === "session_before_compact") {
        const result = await waitForCancellation(originalEmit.call(this, event), event.signal);
        if (!result?.cancel && result?.compaction) validateSummary(result.compaction);
        return result;
      }
      if (event?.type !== "session_compact") {
        return originalEmit.call(this, event);
      }
      const entries = this.sessionManager?.getEntries?.();
      const latestEntry = Array.isArray(entries)
        ? entries.findLast(
            (entry) =>
              entry.type === "compaction" && entry.summary === event.compactionEntry?.summary,
          )
        : undefined;
      const normalizedEvent =
        latestEntry?.type === "compaction" ? { ...event, compactionEntry: latestEntry } : event;
      try {
        return await originalEmit.call(this, normalizedEvent);
      } catch (error) {
        reportCompactionListenerError(error);
        return undefined;
      }
    },
  });
  Object.defineProperty(prototype, LATEST_COMPACTION_EVENT_PATCH_MARKER, { value: true });
  return "installed";
}

function shouldThrottleStatusRender(mode, indicator) {
  if (!THROTTLED_STATUS_KINDS.has(indicator?.kind)) {
    return false;
  }
  if (
    typeof indicator.intervalMs !== "number" ||
    indicator.intervalMs >= LARGE_SESSION_STATUS_INTERVAL_MS ||
    typeof indicator.restartAnimation !== "function"
  ) {
    return false;
  }

  const sessionManager = mode.sessionManager;
  if (sessionManager === undefined || typeof sessionManager.getEntries !== "function") {
    return false;
  }
  return sessionManager.getEntries().length >= LARGE_SESSION_ENTRY_THRESHOLD;
}

export function installLargeSessionStatusRenderThrottle(InteractiveMode) {
  const prototype = InteractiveMode.prototype;
  if (prototype[STATUS_RENDER_PATCH_MARKER] === true) {
    return "already-installed";
  }

  const showDescriptor = Object.getOwnPropertyDescriptor(prototype, "showStatusIndicator");
  if (showDescriptor === undefined || typeof showDescriptor.value !== "function") {
    return "not-needed";
  }
  const originalShowStatusIndicator = showDescriptor.value;

  Object.defineProperty(prototype, "showStatusIndicator", {
    ...showDescriptor,
    value(indicator) {
      const result = originalShowStatusIndicator.call(this, indicator);
      if (shouldThrottleStatusRender(this, indicator)) {
        indicator.intervalMs = LARGE_SESSION_STATUS_INTERVAL_MS;
        indicator.restartAnimation();
      }
      return result;
    },
  });
  Object.defineProperty(prototype, STATUS_RENDER_PATCH_MARKER, { value: true });
  return "installed";
}

function extensionStatuses(footerData) {
  const statuses = footerData?.getExtensionStatuses?.();
  return statuses instanceof Map ? [...statuses.entries()] : undefined;
}

function footerRenderFingerprint(footer, width) {
  const session = footer.session;
  const sessionManager = session?.sessionManager;
  const footerData = footer.footerData;
  const statuses = extensionStatuses(footerData);
  if (
    sessionManager === undefined ||
    footerData === undefined ||
    statuses === undefined ||
    typeof sessionManager.getLeafId !== "function"
  ) {
    return undefined;
  }

  const state = session.state;
  const model = state?.model;
  return JSON.stringify([
    width,
    footer.autoCompactEnabled,
    sessionManager.getLeafId(),
    sessionManager.getCwd?.(),
    sessionManager.getSessionId?.(),
    footerData.getGitBranch?.(),
    footerData.getAvailableProviderCount?.(),
    statuses,
    model?.provider,
    model?.id,
    model?.reasoning,
    state?.thinkingLevel,
    session.settingsManager?.getTheme?.(),
    model === undefined ? undefined : session.modelRuntime?.isUsingSubscription?.(model.provider),
  ]);
}

export function installFooterRenderCache(FooterComponent) {
  const prototype = FooterComponent.prototype;
  if (prototype[FOOTER_RENDER_CACHE_PATCH_MARKER] === true) {
    return "already-installed";
  }
  const renderDescriptor = Object.getOwnPropertyDescriptor(prototype, "render");
  if (renderDescriptor === undefined || typeof renderDescriptor.value !== "function") {
    return "not-needed";
  }
  const originalRender = renderDescriptor.value;

  Object.defineProperty(prototype, "render", {
    ...renderDescriptor,
    value(width) {
      const fingerprint = footerRenderFingerprint(this, width);
      const cached = FOOTER_RENDER_CACHE.get(this);
      if (
        fingerprint !== undefined &&
        cached?.session === this.session &&
        cached.fingerprint === fingerprint &&
        Date.now() - cached.createdAt < FOOTER_RENDER_CACHE_TTL_MS
      ) {
        return cached.lines;
      }
      const lines = originalRender.call(this, width);
      if (fingerprint !== undefined) {
        FOOTER_RENDER_CACHE.set(this, {
          createdAt: Date.now(),
          fingerprint,
          lines,
          session: this.session,
        });
      }
      return lines;
    },
  });
  Object.defineProperty(prototype, FOOTER_RENDER_CACHE_PATCH_MARKER, { value: true });
  return "installed";
}

export function installExtensionUiRenderDedupe(InteractiveMode) {
  const prototype = InteractiveMode.prototype;
  if (prototype[EXTENSION_UI_DEDUPE_PATCH_MARKER] === true) {
    return "already-installed";
  }
  const statusDescriptor = Object.getOwnPropertyDescriptor(prototype, "setExtensionStatus");
  const widgetDescriptor = Object.getOwnPropertyDescriptor(prototype, "setExtensionWidget");
  if (
    statusDescriptor === undefined ||
    typeof statusDescriptor.value !== "function" ||
    widgetDescriptor === undefined ||
    typeof widgetDescriptor.value !== "function"
  ) {
    return "not-needed";
  }
  const originalSetExtensionStatus = statusDescriptor.value;
  const originalSetExtensionWidget = widgetDescriptor.value;

  Object.defineProperty(prototype, "setExtensionStatus", {
    ...statusDescriptor,
    value(key, value) {
      const statuses = this.footerDataProvider?.getExtensionStatuses?.();
      if (statuses instanceof Map && statuses.get(key) === value) {
        return undefined;
      }
      return originalSetExtensionStatus.call(this, key, value);
    },
  });
  Object.defineProperty(prototype, "setExtensionWidget", {
    ...widgetDescriptor,
    value(key, content, options) {
      if (
        content === undefined &&
        this.extensionWidgetsAbove instanceof Map &&
        this.extensionWidgetsBelow instanceof Map &&
        !this.extensionWidgetsAbove.has(key) &&
        !this.extensionWidgetsBelow.has(key)
      ) {
        return undefined;
      }
      return originalSetExtensionWidget.call(this, key, content, options);
    },
  });
  Object.defineProperty(prototype, EXTENSION_UI_DEDUPE_PATCH_MARKER, { value: true });
  return "installed";
}

const packageRoot = process.env.PI_CODING_AGENT_PACKAGE_ROOT;
delete process.env.PI_CODING_AGENT_PACKAGE_ROOT;
if (packageRoot !== undefined) {
  const entryUrl = pathToFileURL(`${packageRoot}/dist/index.js`).href;
  const codingAgent = await import(entryUrl);
  installCompactionSingleFlight(codingAgent.AgentSession);
  installCompactionPreparationReliability(codingAgent.AgentSession);
  installCompactionEventReliability(codingAgent.AgentSession);
  installLatestCompactionEventEntry(codingAgent.ExtensionRunner);
  installLargeSessionStatusRenderThrottle(codingAgent.InteractiveMode);
  installCompactCommandVisibility(codingAgent.InteractiveMode);
  installFooterRenderCache(codingAgent.FooterComponent);
  installExtensionUiRenderDedupe(codingAgent.InteractiveMode);
}
