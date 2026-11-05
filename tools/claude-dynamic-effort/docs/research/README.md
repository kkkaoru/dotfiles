# Dynamic effort research (2026-11-05)

A narrative synthesis of the literature, vendor documentation and direct measurements behind a review of this mod for Claude Opus 5.5. Written in Japanese. Start with [report.md](report.md).

| File | Contents |
|---|---|
| `report.md` | Findings, options and what remains unverified |
| `notes/capture_experiments.md` | Captured requests and controlled effort experiments |
| `notes/local_session_measurements.md` | Aggregates from one person's local transcripts |
| `notes/opus_5_5_guidance.md` | Anthropic documentation for Opus 5.5 |
| `notes/audit.md` | Check of key numbers against primary sources |
| `notes/context_size_degradation.md`, `compaction_trigger.md`, `effort_context_interaction.md`, `summary_effort.md`, `failure_escalation.md` | Per-question literature notes |

`audit.md` overrides the other notes wherever they differ. The notes are kept as written, so some of their values are superseded by it.

## What was decided afterwards

The report compares options; these are the choices made after it, so read them over its recommendations.

- The effort ladder and the recovery floor are removed. Requests run at the effort the person chose.
- `autoCompactWindow` is 200,000.
- Compaction stays at `max`, the person's choice, where the report recommended the session effort.
- The report treats an auto-compaction inside a turn as out of reach of `max`. Stopping the turn with `$.turn.abort` reaches it; see section 5 of `notes/capture_experiments.md`.
- The stuck note is removed with the floor it depended on.
