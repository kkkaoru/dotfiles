# Native replay editing expansion

## User scope

The user requested development of the missing functionality in the Executor MCP for a preserved-original, approximately 91-minute replay edit. Existing working-tree changes belong to earlier work and must be retained. No commits, UI consent changes, external media uploads, new model downloads, or unsafe-interoperability exceptions are authorized by this development request.

## Existing evidence and missing contracts

- Existing native transcription completed 103 overlapping local chunks; output is unreviewed, phrase-level, and not a voice-activity/word-alignment ground truth.
- Existing native FCPXML writing and installed-DTD validation succeeded for original/proxy source assembly. App import and proxy time correspondence remain separate checks.
- Existing masks are static black rectangles, not blur or interval-aware.
- Existing native audio tools cannot align/subtract a reference BGM or distinguish speech from background music. Energy-threshold silence detection must not be labeled speech detection.
- GUI connection failed via Executor; development must not bypass this through another UI runtime.

## Implementation slices

1. **Timed source-region blur:** extend existing mask DTO/schema/validation compatibly with optional output-time interval and blur radius. Missing options preserve the existing static black concealment behavior. Crop managed CoreImage output strictly to the requested rectangle, blend blurred pixels by opacity, and apply before replacement captions. Validate finite radius, matched interval endpoints, bounds and timeline duration. Test exact half-open boundaries, cropped effects, no changes outside the rectangle, compatibility, schema rejection and cancellation. No raw image buffers or new concurrency boundary.
2. **Reference audio analysis and removal:** define bounded, explicit source/reference selections, alignment-search budgets, confidence/fit metrics and refusal conditions. First establish synthetic reference-plus-independent-voice recovery and mismatched-reference rejection. Only then implement local native reference alignment/filtering without claiming arbitrary music source separation. Keep decoded audio buffers and allocations bounded, new files private and non-overwriting, and input hashes unchanged. A reference that does not match the recording must produce a diagnostic, not a voice-damaging guessed subtraction.
3. **Speech-aware cuts and transcript mapping:** add an independently justified local speech-activity method, or expose explicit reviewed speech spans as input without relabeling energy/ASR spans as proven speech. Provide pure validated interval padding/merge/complement and output-time maps. Preserve raw recognition and overlap evidence. No implicit model installation or cloud fallback.
4. **Long-form orchestration/interchange:** process bounded chunks with durable manifests and per-job status/cancellation instead of weakening existing child/time/memory caps. Produce FCPXML with original/proxy references and mapped cut/title intervals. Do not equate interchange validity with imported FCP fidelity.
5. **Actual deliverables:** use deployed tools through Executor, validate 60-second and 90-second samples including complete audio/video decode, exact blur/outline boundaries and source preservation, then produce the full-length requested vertical deliverable. Full output remains pending until all edits and actual media verification pass.

## Verification and deployment

Before changing behavior, record the full current test baseline. Each slice requires focused synthetic regressions, strict swift format, warnings-as-errors build/tests, all production files >=95% line/function coverage, and separate TSan/ASan where applicable. Do not add exceptions or lower existing gates. Review Swift rule IDs against the diff. Build release only after gates; refresh the existing Executor integration through its documented management boundary, rediscover schemas, then perform real calls. Keep mock/unit, native fixture, real-media render, full decode, DTD validation and app import evidence distinct.
