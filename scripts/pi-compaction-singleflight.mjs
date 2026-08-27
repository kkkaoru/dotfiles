import { pathToFileURL } from "node:url";

// Pi 0.84.3 stores each compaction controller in one mutable session field. Overlapping runs can
// replace or clear that field across an await and crash on the next `.signal` read. Keep the public
// behavior single-flight until Pi ships per-run controllers; stop applying the guard automatically
// once those vulnerable field reads disappear.
const PATCH_MARKER = Symbol.for("kkkaoru.pi.compaction-singleflight");
const ACTIVE_COMPACTIONS = new WeakSet();
const AUTO_CONTROLLER_ACCESS = "this._autoCompactionAbortController.signal";
const MANUAL_CONTROLLER_ACCESS = "this._compactionAbortController.signal";
const ALREADY_RUNNING_MESSAGE = "Compaction is already in progress. Wait for it to finish and retry.";

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

async function runExclusive(session, operation) {
  ACTIVE_COMPACTIONS.add(session);
  try {
    return await operation();
  } finally {
    ACTIVE_COMPACTIONS.delete(session);
  }
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
      return ACTIVE_COMPACTIONS.has(this) || originalIsCompacting.call(this);
    },
  });
  Object.defineProperty(prototype, "_runAutoCompaction", {
    ...autoDescriptor,
    async value(...args) {
      if (ACTIVE_COMPACTIONS.has(this) || originalIsCompacting.call(this)) {
        return false;
      }
      return runExclusive(this, () => originalAutoCompaction.apply(this, args));
    },
  });
  Object.defineProperty(prototype, "compact", {
    ...manualDescriptor,
    async value(...args) {
      if (ACTIVE_COMPACTIONS.has(this) || originalIsCompacting.call(this)) {
        throw new Error(ALREADY_RUNNING_MESSAGE);
      }
      return runExclusive(this, () => originalManualCompaction.apply(this, args));
    },
  });
  Object.defineProperty(prototype, PATCH_MARKER, { value: true });
  return "installed";
}

const packageRoot = process.env.PI_CODING_AGENT_PACKAGE_ROOT;
delete process.env.PI_CODING_AGENT_PACKAGE_ROOT;
if (packageRoot !== undefined) {
  const entryUrl = pathToFileURL(`${packageRoot}/dist/index.js`).href;
  const codingAgent = await import(entryUrl);
  installCompactionSingleFlight(codingAgent.AgentSession);
}
