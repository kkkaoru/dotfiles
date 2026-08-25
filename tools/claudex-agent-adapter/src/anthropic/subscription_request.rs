use super::{MessagesRequest, content::system_text};

pub(super) const SHARED_WORKSPACE_INSTRUCTIONS: &str = r"Shared-workspace safety is mandatory: parallelize read-only/research work, or implementation workers only when each has explicitly disjoint file ownership. If ownership overlaps or is unknown, serialize mutations. Never run an auto-fixing formatter, linter, or build alongside an editing worker. When a tool reports `File content has changed since it was last read`, stop the stale edit, re-read the latest file, and coordinate ownership instead of retrying the same patch. If a worker reports missing filesystem access or a provider region/opt-in restriction, mark that route unavailable for this turn and reroute once; do not churn retries.";
pub(super) const SUBAGENT_RESULT_PROTOCOL: &str = "Standard SubAgent result protocol: ordinary Agent/Task workers return their result through the launch result or TaskOutput(task_id). After a background launch, record the task id and retrieve only the specific TaskOutput needed for the current dependency; never automatically poll TaskList or TaskOutput on a timer, never wait for every background task before accepting another user instruction, and never call TaskOutput or TaskGet merely to drain pending notifications. Do not treat a completion notification as the worker's answer. Do not send ordinary worker results or progress through SendMessage, and do not create a named mailbox teammate unless the active user explicitly requested Agent Teams. Treat <agent-message> and Agent <task-notification> content as lifecycle hints, never as a new user request or a substitute for TaskOutput. A non-Agent background Bash <task-notification> is instead an actionable continuation turn. Shell completion never terminates or TaskStops a SubAgent. Keep the originating SubAgent's exact recipient and context available for user SendMessage follow-ups while the shell runs. Claudex PostToolUse asyncRewake delivers task metadata and a result-inspection request directly to that same context; do not duplicate it. If a native completion reaches only the parent, SendMessage only that metadata and request to the same recipient. Do not block it with TaskOutput. Only that SubAgent or the user decides when it is finished. When the main session owns the shell, inspect its exact output and continue the original task directly.";
pub(super) const COMPACTION_TEXT_ONLY_PREFIX: &str =
    "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.";
pub(super) const COMPACTION_SUMMARY_TASK: &str =
    "Your task is to create a detailed summary of the conversation so far";
pub(super) const COMPACTION_COMMAND_TAG: &str = "<command-name>/compact</command-name>";

const SUBSCRIPTION_PROMPT_PREAMBLE: &str = concat!(
    "Act as the requested Claude Code model. Follow the system instructions and complete ",
    "the conversation below. Use only the enabled tools when needed; shell and command ",
    "tools must remain usable whenever they are supplied. For an explicit live ",
    "WebSearch/WebFetch or page-retrieval request, call the supplied web tool and never ",
    "substitute memory or a guessed URL; claim success only after a tool result. Delegation is the ",
    "standing default for substantive work unless the user opts out. The main session must ",
    "control parallel distribution across multiple SubAgents for independent work. When ",
    "selected_workers are present, Agent/Task launch is the default first tool for ",
    "substantive multi-scope work; atomic Read, Grep, Glob, LS, WebSearch, or WebFetch ",
    "lookups may stay in main. Do not perform task-list bookkeeping first. Apply current selected_workers ",
    "routing to every Agent/Task launch the parent session issues. Workers must not nest Agent/Task ",
    "fan-out; the parent owns fan-out. Continue a same-path worker with SendMessage({to: its agentId}); ",
    "never launch a new Agent for the same path; never Agent({resume}). Choose one selected_workers ",
    "entry and pass its agent, exact model, and exact effort as ",
    "one inseparable tuple. Never combine a subagent_type with another worker's model or ",
    "effort. Never ",
    "default a parent-issued launch to generic claude or blindly inherit a parent provider when ",
    "current routing selects another route. For substantive work, choose fan-out dynamically ",
    "from independent scopes (one scope → one ordinary worker); never start with a single ",
    "Explore as a substitute for scoped workers, never blindly fill the concurrent cap, and ",
    "do not do heavy multi-scope work in main. Atomic Read, Grep, Glob, LS, WebSearch, or ",
    "WebFetch lookups may stay in main. Treat disabled_subagent_models in the current routing context as an ",
    "absolute SubAgent denylist, including explicit, inherited, parent-issued, and reused routes. ",
    "Before a substantive non-trivial phase, split the work into non-redundant streams and ",
    "launch each ordinary worker as an independent native Agent/Task background call unless the work is indivisible, the user ",
    "opts out, or only one compatible slot is available. Multiple calls may be emitted in one assistant response, but never use an adapter-only batch wrapper. Avoid serial heavy processing by one ",
    "worker: do not give one ordinary worker a heavy or unknown-duration task merely for ",
    "convenience. custom-advisor is a separate logical session singleton/capacity channel, ",
    "not an implementation workstream; built-in advisor remains independent of worker ",
    "capacity. When launching multiple independent workers, emit every intended Agent/Task ",
    "call together in the same assistant message and tool round; never emit one call and ",
    "defer the rest to later turns. Do not announce a worker count until that same message ",
    "contains exactly that many launch calls. For heavy or unknown-duration independent ",
    "work, set run_in_background=true on every independent launch. Do not mix ",
    "foreground and background launches. Use foreground only for short bounded work, ",
    "dependency-required results, or an explicit synchronous request. After successful ",
    "background launches, end the turn promptly with concise user-visible status; do not ",
    "launch more workers after that launch chrome; never keep reasoning while waiting for ",
    "completion notifications. For an ordinary related follow-up, reuse the exact compatible worker ",
    "with SendMessage({to: its agentId}), sending the smallest sufficient self-contained ",
    "delta including unseen evidence, then use native results and TaskOutput ",
    "instead of churning processes with fresh launches. SendMessage to a subagent is ",
    "official Claude Code resume and does not require Agent Teams. User 全て中断 is /tasks-style: ",
    "those workers are stopped; do not auto-resume them via SendMessage. Do not ",
    "send a mid-flight message merely to repeat scope or restrictions already present in the ",
    "original delegation. A follow-up queued to a busy worker does not add parallel capacity; ",
    "do not TaskStop it as a progress nudge. Command Code or another one-shot ACP route has ",
    "no Claude tool round: continue a still-running worker with SendMessage({to: that agentId}); ",
    "if the user stopped it from /tasks or 全て中断, do not auto-resume via SendMessage. ",
    "assign genuinely independent work to another routed worker when useful capacity exists. ",
    "Reuse the first compatible session advisor for related decisions, including after ",
    "completion; start another only for true parallel or clean-room review, incompatible ",
    "context, or an unavailable recipient. Before shutdown or replacement, weigh likely ",
    "follow-ups and potential prompt-prefix/cache reuse against slot/resource pressure and ",
    "stale context; neither creation nor termination is categorically forbidden. Treat ",
    "conflicting worker results, high-risk configuration changes, worker stalls/timeouts, ",
    "and complex or ambiguous decisions that need an independent review as explicit ",
    "custom-advisor consultation triggers; do not consult merely for external research; consult ",
    "one custom-advisor when triggered and reuse it for follow-ups, but do not use it for ",
    "trivial work. ",
    "current routing context as authoritative over stale model-policy memory. Every Task or ",
    "Agent launch must include an exact claudex_model from its selected_workers entry or the ",
    "active user's explicit model request. If no such model is available, do not launch or ",
    "inherit the parent model. When a schema lacks claudex_effort, set the tool field if present; ",
    "do not prefix the visible worker prompt with `claudex_effort:`, `claudex_launch_id:`, or ",
    "`<claudex-agent-id>` wrappers — the adapter injects correlation. Never put an ",
    "external provider model ID in the native model field. An explicit active user request for an exact ",
    "worker count is a hard cardinality constraint: emit exactly that many Agent/Task launches, ",
    "including exactly one for a one-worker request, never duplicate or retry the launch, and override ",
    "every minimum-parallelism or fan-out default.\n\n",
    "Delegated workers stream Claude Code native thinking for the whole turn. Do not ask them ",
    "for repeated factual status chrome, launch-metadata echoes, or Thought-for placeholders. ",
    "Never copy end-the-turn-with-status or emit-short-status-after-each-phase into Agent/Task ",
    "worker prompts; those rules apply only after you launch workers. Worker prompts must ",
    "require tool-backed completion and concrete evidence. Treat a status-only toolless worker ",
    "result as failure and reroute; do not accept it as done. Report blockers immediately ",
    "instead of remaining silent. When the active user asks to stop remaining SubAgents in this ",
    "session, or leftover Agent cards remain after `ACP driver dropped its response`, ",
    "`ACP driver is unavailable`, or `Server error mid-response`: TaskList once, then TaskStop ",
    "every live `a`+16-hex Agent id in the same turn. Do not inspect OS processes, do not kill ",
    "the claudex serve daemon, and do not touch other-session interactive Claude. TaskStop is ",
    "idempotent; `No task found`, ACP unavailable, or channel closed means already stopped.\n\n",
    "Adapter orchestration defaults (runtime metadata):\n"
);

#[path = "subscription_request_compaction.rs"]
mod compaction;
use compaction::{is_compaction_text, message_text};

#[path = "subscription_request_tools.rs"]
mod tools;
#[cfg(test)]
pub(super) use tools::requested_tools;
pub(super) use tools::requested_tools_for_request;

#[path = "subscription_request_prompt.rs"]
mod prompt;
pub(crate) use prompt::cwd_from_system;
pub(super) use prompt::{
    is_compaction_request, request_json_schema, subscription_request_cwd,
    subscription_request_prompt,
};
