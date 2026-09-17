# Replay editing verification — 2026-09-14, 23:47

Overall requested video editing is **not complete**. This report covers the new native timed-mask, reference-analysis, sound-activity and supplied-span cut-plan slices. Existing concurrent video-layer/Compressor development was preserved; passing shared tests does not imply a full independent semantic review of that separate feature.

## Final current-checkout gate

Detached job ending `-35` completed successfully:

- `swift format lint --strict --recursive Sources Tests Package.swift`
- `swift test -Xswiftc -warnings-as-errors --sanitize=thread`: 229 tests / 45 suites, passed.
- `swift test -Xswiftc -warnings-as-errors --sanitize=address`: 229 tests / 45 suites, passed separately.
- `swift test -Xswiftc -warnings-as-errors --enable-code-coverage`: 229 tests / 45 suites, passed after sanitizer runs.
- `xcrun llvm-cov export ... -summary-only -ignore-filename-regex='(/\.build/|/Tests/)'`: all 37 production files passed 95% lines and functions. Minimum 95.2381% lines / 96.7742% functions. LLVM emits no Swift branch counters here; region coverage is not branch coverage.
- `swift build -c release -Xswiftc -warnings-as-errors`: passed, no warnings.
- Scoped `git diff --check`: passed.

Earlier failures were fixed, not skipped: optional-tuple type inference in a test, multiline range syntax, shared Compressor URL-expression type inference, new tool annotation expectations and formatting in concurrent video-layer tests. New mask filter failure paths initially left 93.55% line coverage; a shared explicit error boundary plus a nil-output regression brought that file to 100%. No exclusions, thresholds, hooks or signing settings were weakened.

## Rule review (new replay implementation scope)

- **SW01–04:** Existing SwiftPM/toolchain and pinned MCP SDK retained. Plan recorded before changes. Focused tests followed by full gates above. No new package, model, runtime, unsafe exception or diagnostic suppression.
- **SW05–15:** Typed Codable/Sendable request/result structs; masks retain nil-option compatibility. Finite values, ranges, interval ordering, operation/sample limits and normalized PCM are checked at entry. Unknown JSON properties are rejected by existing schema enforcement. No force unwrap/cast, `Any`, `try?`, invariant trap or silent clipping. Classification downcast is conditional and reports unexpected results. Error values are preserved; reference mismatch is not silently treated as success.
- **SW16–26:** Existing private/non-overwriting file publication retained. DSP and synchronous file classification run on dedicated serial actor executors, not UI/cooperative work queues. Observer lifetime extends through analyzer request removal; callback state uses Mutex. No new raw pointer, unsafe continuation, unchecked Sendable, detached task or unowned capture. Native synchronous classification checks cancellation before/after and remains child-deadline bounded; immediate in-process interruption is not claimed. The synthetic cancellation test owns, cancels and awaits its task.
- **SW27–30:** No new GUI state or playback. New CoreImage filters are callback-local managed objects. New files are cohesive, typed, and bounded; DSP's brute-force search has an explicit product budget rather than unsafe optimization. Comments/diagnostics are English. Private recordings and paths stay outside public examples/test fixtures.
- **SW31–37:** Related Swift Testing regressions accompany models, schema, adapters and native behavior. Synthetic WAV/video fixtures only. Exact mask intervals, crop/opacity, caption order, compatibility, invalid data, file preservation, callback failures and cut maps are exercised. Sanitizers are supplemental evidence, not a proof. Required line/function gates passed per file; no claim of unsupported branch coverage.

## Actual Executor evidence (kept privately with edit output)

The existing org/default native connection was refreshed and its schemas rediscovered. 29 tools were exposed, including `audio_reference_analyze`, `audio_sound_activity`, `speech_cut_plan`, and optional timed mask blur arguments.

- Real 60/90-second diagnostic exports: 1800/2700 video frames, complete audio/video decode, source preservation checks. Selected OCR frames show the diagnostic centered caption during [1,10), absent before 1 and at 10; source subtitle text reappears outside the test blur interval. Not a full-video subtitle localization or final caption accuracy test. Audio codec inspection reports AAC/24 kHz stereo in these native samples; original audio rate preservation is not claimed.
- On-device ASR: all 103 overlapping chunks recognized; phrase timing, boundary duplication and word/name errors remain unreviewed.
- SoundAnalysis: 103 chunks and 22,484 speech/music windows. Scores are estimates, not reviewed speech boundaries.
- Supplied-span planner: thresholds 0.2/0.35/0.5 produce differing candidate lengths. Conservative 342-interval FCPXML references original/proxy and passes installed DTD 1.14 validation. Frame boundaries round outward. FCP import, precise proxy correspondence and cut accuracy are unverified.
- Reference matching: strong low-frequency candidates did not survive 16 kHz checking (returned fine correlations approximately 0.06–0.13; two refused). No BGM subtraction was applied to a final recording. Synthetic orthogonal-component cancellation does not establish real voice preservation.

## Pending user-dependent step and deliverables

A possible next step is Core ML HTDemucs inference using an explicitly approved local model. The proposed third-party card states a 75 MB download, about 1 GB peak RAM, MIT licensing and CPU-only inference. Exact artifact hash/provenance, model contract and practical quality require verification. Download/installation approval was requested and has not been received. Nothing was downloaded and no media/fingerprint was uploaded.

BGM separation, accurate speech cuts/transcript mapping, complete timed source-subtitle masking, final outlined captions, actual FCP import and final vertical encoded media remain unfinished. Diagnostic media remains available and was not removed. No goal completion, app import success, perceptual quality score, or final TikTok export is claimed.
