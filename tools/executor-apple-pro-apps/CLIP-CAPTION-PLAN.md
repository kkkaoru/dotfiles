# Motion, transcription and one-minute clip verification

## Requested deliverables

1. A video with animation authored/rendered in Motion, not a substitute renderer.
2. A different source transcribed locally and exported with timed captions.
3. Approximately 60 seconds assembled from explicitly recorded ranges in multiple
   original videos. Until the user supplies editorial ranges, test three distinct
   recordings with 20-second selections and report their source/output mapping.

All media/project outputs stay under ~/Movies/Apple-Pro-Apps-Verification. Sources
remain unchanged. No uploads, automatic model installation, playback, recording,
TCC changes or mutation of existing app projects.

## Contracts and implementation sequence

- Recheck Executor-hosted Motion permissions/capabilities. Native Motion has no
  established public animation/render API; use isolated-project GUI operations
  only when actual permissions allow them. If blocked, report that separately.
- Inspect installed SpeechAnalyzer/SpeechTranscriber APIs (macOS 26 available on
  this host), locale/model availability, privacy and cancellation contracts. Add
  typed local transcription through the existing Swift CLI/MCP if feasible. Never
  replace recognized speech with invented captions or quietly use cloud fallback.
- Add typed timed captions and native offscreen raster/compositing as needed.
  Bound cue/text/pixel counts, validate ordering/times and retain transcript files.
  Distinguish ASR output from human-reviewed transcription and word-level alignment.
- Extend full-video verification with an explicit bounded long-clip budget, keeping
  the default short-clip safety policy. Test old defaults, opt-in limits, over-budget
  refusal, full decode, cancellation and MCP/CLI schema parity. A one-minute export
  must not be represented as fully verified by first-frame inspection alone.
- Render the three-source timeline via Executor. Verify duration, source selection
  mapping, joins, full frame decode, audio and unchanged source hashes. Retain recipes.

## Ownership and verification

Native media/process I/O retains operation-owned bounded executors. SwiftUI stays
on MainActor; Speech tasks must have observed results and cancellation/join cleanup.
Gate newer APIs without raising the package's macOS 15 deployment target. No new
unsafe interoperability exceptions are authorized by this plan.

Use focused Swift Testing regressions, strict formatting/lint and warnings-as-errors,
per-production-file line/function coverage >=95%, separate TSan/ASan and restored
normal instrumentation. Refresh and describe affected Executor tools before real
calls. Inspect structured envelopes/artifacts rather than tmux status alone.

Current blocker: the 2026-09-14 recheck still reports Accessibility and Event
Synthesizing not granted to the Executor-hosted UI path; Screen Recording is granted.
