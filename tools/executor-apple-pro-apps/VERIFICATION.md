# Head geometry candidate — verification incomplete

- `video_head_detect` is implemented in source, not deployed. Strict formatting
  and warnings-as-errors compilation pass. Blank synthetic video checks actual
  frame clocks, missing detections, source preservation and cancellation; typed
  synthetic bounding boxes check coordinate conversion and bounds. These do not
  establish real face-detection accuracy or caption placement.
- Full normal and ASan runs: 376 of 379 tests pass. The remaining three require
  the existing Whisper Core ML/tokenizer fixtures. No ASan diagnostic was found.
- `FrameHead.swift` reaches 100% line/function coverage after sharing the face and
  body coordinate converter. The full-file gate still fails on the existing
  Whisper/tokenizer files; it is not waived.
- The full TSan run aborted with a sanitizer SEGV in the malloc/realloc path while
  executing `CueSound.wave()`. This is an unresolved failure, not a clean TSan run
  or proof of a particular root cause. Separate TSan isolation runs passed all
  six CueSound tests and all four new head-geometry/service tests; neither had a
  sanitizer diagnostic. These do not replace the failed full TSan gate.
- Release deployment and real-video/FCP head placement remain unverified.

# Earlier verification status — 2026-10-03

## Final Cut Pro safety, merge paste, key derivation, English UI (62-tool release, 2026-10-08)

- Gates: strict lint, warnings-as-errors debug and release builds; **457/457 tests**
  (model fixtures set); TSan and ASan 457/457 with no reports; all 64 production
  files ≥95% lines and functions (FinalCutEffects 99.39/97.26,
  UIAutomation+FinalCutEffects 99.82/100, UIAutomation+FinalCut 99.40/100).
  Executor refresh: 62 tools, healthy; warm-up read after the serve restart.
- Incident and fix: a probe batch continued after a failed `fcp_project_open`
  and edited another session's project; reverted (see the replay checkout's
  edits/work/fcp-live/FINDINGS.md). Added the `project` guard, foreground Open
  Clip with navigate-back, busy wait and stop-on-error in the probe harness.
- Real acceptance (disposable libraries):
  - Merge paste kept existing effects (カラー調整, ガウス) and keyframes (opacity
    5→10 s; rotation, グロー) while adding new ones; XML shows position keyframes
    at the requested clip-relative times (timing Maintain). Carrier library closed
    and inspector reselected afterwards.
  - Key derivation: 22 structures (1731 of 1736 published parameters) matched
    Final Cut Pro's own XML; 360° Patch "Target Angle" stayed unconfirmed.
  - FxPlug: ドロップシャドウ (`FxPlug:9C13F991-…`) pasted; non-offered コミック is
    ignored by Final Cut Pro, so the catalog lists only `finalCutSimplifiedList`.
  - English UI (relaunch with -AppleLanguages (en)): open, read, seek, moves,
    blade, select, delete, clip range, menu effects, inspector read/set, merge and
    replace paste, Export XML, Share export (2250/2250 frames) all passed after
    fixing four titles (Add Color Adjustment, check box / pop up suffixes,
    Maintain, Video Codec:). The Share default is matched by its marker.
  - `fcp_library_close` closed the verification library; Final Cut Pro was
    relaunched normally and the user's timeline restored (01:15:09:03, 22:17).
- Not covered: audio effects (Audio Units, Logic, third-party), undo, list-view
  browsers, 360°-only parameters.

## Final Cut Pro effects, parameters and keyframes (60-tool release, 2026-10-08)

- Gates (full run before two string-only description edits, which were then
  covered by lint, focused tool tests and the release build): strict lint,
  warnings-as-errors debug and release builds; **445/445 tests**
  with the verified model directories; TSan and ASan 445/445 with **no sanitizer
  reports**; all 64 production files ≥95% lines and functions (FinalCutEffects
  99.66/97.87, UIAutomation+FinalCutEffects 100/100, FinalCutTools 99.22/100,
  UILiveBackend 95.88/96.55). The generated carrier DTD-validates against the
  installed Final Cut Pro 12.3 (FCPXML 1.13).
- Executor refresh (org and user): 60 tools, health `healthy`.
- Real acceptance (disposable library only, synthetic media):
  - Background inspector: read 24 controls; `blur boost` 1.5, 回転 0, 手ぶれ補正 off,
    ガウス effect disabled/enabled — all read back, `frontmostChanged: false`.
  - `fcp_effects_paste` on FX90 (2.5 s, no focus change): Final Cut Pro's own XML
    (`fcp_xml_export`) shows position keyframes 30/40 s, opacity 60/70 s, ガウス
    Horizontal 37 and Amount keyframes 0/4 s. Export: 2700/2700 frames; edges 0.96
    at 0 s → 0 at 2 s; left strip 102 (29 s) → 37 (35 s) → 16 (45 s); luma 95
    (59 s) → 71 (65 s) → 16 (70 s).
  - FX60c bladed at 30 s, one paste onto both clips: 1800/1800 frames; each clip
    fades 5–10 s after its own start (luma 126/100/16 at 4/7/11 s and 34/37/41 s).
  - Through Executor: catalog → グロー uid, paste with rotation keyframes onto
    clip 2, inspector shows グロー, XML shows rotation 30/35 s and the glow key.
  - Audio of the effect exports (re-exported after review): FX60c 1800/1800 and
    FX90 2700/2700 frames decoded, audio 60.000/90.000 s 48 kHz stereo decoded
    fully, RMS -27.10 dB / peak -22.28 dB — identical to the no-effect Export60, so
    Paste Effects did not change audio levels. Files deleted after measurement.
  - Findings: Paste Effects **replaces** existing effects/animated attributes of
    the targets (earlier opacity keyframes were removed); opacity midpoints render
    ~70% luma because Final Cut Pro blends in linear light; inspector keyframe
    buttons and parameter menus ignore AXPress even when active.
- Not covered: Paste Attributes (selective merge), FxPlug-only effects without a
  Motion template, English UI suffixes, title/text inspector panes.

## Final Cut Pro live timeline tools (55-tool release, 2026-10-08)

- Gates: strict `swift format lint`, warnings-as-errors debug and release builds.
  `swift test --enable-code-coverage`: **422 of 422 tests pass** with
  `WHISPER_TEST_MODEL`, `WHISPER_TEST_TOKENIZER` and `DEMUCS_TEST_MODEL` pointing
  at the verified repository-local model directories of the replay checkout.
  Separate TSan and ASan runs: 422/422 pass, **no sanitizer reports**.
- Coverage (lines/functions): FinalCutModels 100/100, UIAutomation+FinalCut
  99.65/100, FinalCutTools 98.53/100, UIAutomation 99.04/95.56, UIModels 100/100,
  UILiveBackend 98.46/98.25; all 62 production files ≥95% lines and functions.
- Executor refresh (org and user default) lists 55 tools including the 7 `fcp_*`
  tools; health `healthy`. The held `serve` child was restarted to load the build.
- Real acceptance, Final Cut Pro Creator Studio 12.3 (Japanese UI), disposable
  library `FCPLive-Verification-20261008` with synthetic 1080p30 testsrc2 + 440 Hz
  media only; the user's open project was read but never edited:
  - Background (`frontmostChanged: false` on every call): project open from the
    library sidebar/filmstrip; seek 00:00:20:15 (617 steps, ~0.6 s), blade, seek
    00:01:10:00, blade → clips 20:15 / 49:15 / 20:00; select clip 1 → delete →
    2 clips, 40:15; stale clip description rejected; seek past the end fails
    with the reached position.
  - Export (activation authorized): Export60 → 60.000 s, 1800/1800 frames fully
    decoded (`media_verify_video`), AAC 48 kHz stereo decoded; Export90 → 90.000 s,
    2700/2700 frames, 90 s audio; edited Live90 → 1215 frames, with frame-level
    PSNR peaks at output 614↔source 614 and output 615↔source 2100 (the cut).
    The previous frontmost app was restored every time; an early failure (before
    the column-wait fix) closed the dialog and restored focus as designed.
  - Through Executor itself (`executor call`): open, seek (902 steps), blade,
    read, and export of Export60 → 1800/1800 frames decoded, audio decoded.
- Not covered: English UI titles (unverified), list-view browsers, multicam/
  compound clip internals, other share destinations, undo. Export completion is
  verified on the file, never by the tool result.

## Single-pass part rendering, batch and FCPXML export (47-tool release)

- Gates: strict lint; warnings-as-errors debug and release builds; **359 of 364 tests
  pass** (the same 5 model-fixture tests need DEMUCS/WHISPER env fixtures); TSan and
  ASan report nothing. Coverage (lines/functions): EditSinglePass 100/100,
  VideoWriterSettings 100/100, EditingService 100/100, NativeEditor+SinglePass
  98.4/100, NativeEditor+FCPXML 97.4/100, FCPXMLTimeline 98.9/95.1,
  StyledCaptionRenderer 95.7/100.
- Real parts (1080×1920, HEVC full-range VFR source; scratch converter took clips,
  masks and the burned ASS events): frame counts equal the plan on 30/30 parts.
  Those 30 FCPXML files were written before the `frameDuration` fix (they said
  `1s`, which the DTD cannot catch), so their DTD passes prove nothing about frame
  units; post-fix FCPXML evidence is the 3 MCP batch items and one FCP import below.
  Caption fill bounding box vs libass: ±10 px horizontal, ±2 px vertical. Colors
  match in RGB (slope 0.99). Frame alignment: on the same part without captions the
  single pass matches the existing export-session renderer exactly (offset 0 on
  259/315 frames, 50.6 dB), and both show source frames one output frame later than
  FFmpeg on this VFR source (offset −1 on 249/315). This is pre-existing native vs
  FFmpeg behavior, not a single-pass regression.
- Size: on one 10.5 s part native was 8.00 MB vs FFmpeg 6.35 MB (~6.1 vs ~4.8 Mbps);
  `averageBitRate` is honored but FFmpeg's `-maxrate`/`-bufsize` cap is not.
- Speed, 30 parts / 315.6 s output / 1258 captions, 3 at a time: FFmpeg part_render
  38.3 s; native 33.6 s (debug) / 35.0 s (release), including FCPXML export.
- Through Executor MCP: `media_edit_batch` rendered 3 real-source parts in 4.74 s
  (315/315/316 frames, FCPXML DTD-valid); one FCPXML was imported into the
  disposable verification library via the background chooser flow and Final Cut
  Pro reported the 10.5 s project. Title positions in FCP were not visually checked.
- Found and fixed during verification: VFR cadence, color-space round trip,
  libass OS/2 font metrics, FCPXML frameDuration. Not enforced: maxrate/bufsize.

## Native background UI tools (46-tool release)

- Gates: strict `swift format lint`, warnings-as-errors debug and release builds.
  `swift test --enable-code-coverage`: **328 of 333 tests pass**; the 5 failures are
  the pre-existing model-fixture tests that require `DEMUCS_TEST_MODEL`,
  `WHISPER_TEST_MODEL` and `WHISPER_TEST_TOKENIZER` (not set on this host). TSan and
  ASan runs show the same 5 failures and **no sanitizer reports**.
- Coverage (lines/functions): UIAutomation 99.3/95.6, UIModels 100/100,
  UILiveBackend 98.4/98.1, UITools 99.1/100; all other changed files ≥95%. The
  Demucs/Whisper files stay below the gate until their model fixtures are supplied —
  a pre-existing blocker, not waived.
- Executor refresh (org and user default) exposes the `ui_*` tools.
- Real acceptance through Executor MCP only: FCPXML Open Document →
  `ui_wait` chooser → `ui_set_value` row `selected` (read back) → `ui_perform` press
  → `ui_wait` absent. Final Cut Pro's read-only dictionary then listed the new 1.0 s
  project in the disposable verification library; the user's other library was
  unchanged. `frontmostChanged` stayed false for every call (no activation).
- `ui_capture` captured an occluded Logic Pro window (3600×2260 PNG) in the
  background. Menu listing, window listing and input-source status were read live.
- `ui_inspect` initially aborted on real Final Cut Pro trees (one element returned
  AXError -25200 for AXSubrole). Element-specific failures are now recorded per
  element in `unreadable` instead of aborting; busy (-25204) and permission errors
  still propagate. Re-verified through Executor on the Final Cut Pro main window.
- Compressor: normal `app_quit`, then `app_launch` with `activate: false`
  (`frontmostChanged: false`), then a 5 s `compressor_submit` reached `Successful`
  100%; `media_verify_video` decoded all 150 frames. (Earlier, a job stayed at 0%
  until the app was running.)
- Not covered: keyboard-only steps such as a Save panel's Go to Folder sheet;
  AXConfirm and pid-targeted keys did not commit it on this host. Final Cut Pro
  itself can become frontmost after an import completes; the tools report, not
  prevent, such app-initiated activation.

# Earlier status — 2026-09-14

## Latest decorated 60/90-second deliverables

Both real 720×1280 outputs are rendered and verified: **1800/2700 frames**,
**960,000/1,440,000 mono samples**, outlined/positioned captions over an opaque
source-subtitle mask, and 5/8 onset SEs. Selected boundary-frame OCR, black mask/gap
regions, upper source/control comparisons and hashes passed. ASR is explicitly
unreviewed; contextual re-recognition and source OCR expose unresolved names and
phrases. No accuracy percentage or human-listening sign-off is claimed.

The 26-tool feature release passed **179 tests / 32 suites**, 31 file gates and
full separate sanitizers. See [CAPTION-DECORATION-VERIFICATION.md](CAPTION-DECORATION-VERIFICATION.md)
for rules review, commands, limits and the final unchanged-cap consolidation audit.
Private viewing indexes include both videos, raw recognition, SRT and evaluation.

## Earlier verified transcription follow-up

The native **24-tool user/default release** passed **155 test functions / 27 suites**,
all **30 production-file** gates (minimum 95.24% lines / 96.55% functions), separate
full TSan/ASan runs, restored full normal instrumentation, strict formatting,
warnings-as-errors release build and Executor refresh. No source exclusions or
failing-test waivers were added. Discovery/schema inspection preceded real calls.

After explicit user approval, Apple's setup request prepared Japanese assets.
Readiness differed across applications: a supported locale must also be reserved
by the calling app. A separate mutating `speech_locale_reserve` tool now performs
that approved app-scoped reservation, without download or eviction. Read-only
`audio_transcribe` never reserves/downloads implicitly. Tests prove recognition,
idempotent preparation and rejection of an unreserved alternative without reserving it.

Executor transcribed a distinct 12-second source excerpt into two unreviewed phrases
and rendered a 14-second contextual video with captions during [1,13). It decoded
all **420 frames**. At 0.5/1/4/9/13/13.5 seconds, matching-time comparisons against
a caption-free control found bottom-region average RGB differences of 3–5/255
while visible, and at most about 1/255 outside the interval and in the central
control region. This is not full-pixel or OCR verification. Both audio tracks had
224,000 mono samples; RMS differed by about 1e-9. Source/output hashes were unchanged.
Original ASR JSON, mapped cues and SRT are retained privately with the viewing output;
text is uncorrected, timing approximate, and the approximately 23.5ms container tail
is clipped to the original 12-second selection before adding the context offset.

The real one-minute edit additionally yielded **960,000 mono samples**, RMS 0.18689,
with nonzero join-window RMS. Both this and the caption output peak at 1; no absence
of clipping, perceptual fidelity or per-channel validation is claimed.

**Motion animation remains unfinished:** the latest Executor-hosted checks still
denied Accessibility and Event Synthesizing. Chat approval does not grant OS consent.
No Motion animation or proprietary-app GUI sign-off is claimed.

## Historical checkpoint: before approved Speech preparation (superseded)

The following records the earlier blocked state, not the current deployment.

The deployed 22-tool release subsequently passed **129 tests / 23 suites**, all
28 production-file gates and separate sanitizers for explicit long-video decode
budgets (default 30s/1800 frames, opt-in up to 120s/7200). A real three-source
60-second, 720×1280 output decoded all **1800 frames**; all three source hashes
matched. At 19.9/20.1/39.9/40.1 seconds, whole-frame and central-region measurements
matched the corresponding source times, with maximum average RGB difference about
2/255. This is region agreement, not exact pixel identity. Audio-wide measurement
of the real one-minute output remains pending deployment.

**New Speech source is not deployed or verified complete.** Nine transcript/lifecycle
tests passed, but native Japanese recognition failed its installed-asset guard.
Both SpeechTranscriber and DictationTranscriber report AssetInventory `supported`,
not `installed`, despite their installedLocales listings containing Japanese.
No download/reservation was performed and the failing test was not skipped or
weakened. Further model preparation needs user permission. The source catalog
currently includes a pending 23rd `audio_transcribe` tool; the deployed catalog is
still 22. Timed captions and explicit long-audio measurement are now implemented,
but remain undeployed with the pending Speech changes. The follow-up full run had
**153 test functions / 27 suites and two failures**, both from the Speech asset guard.
Failed-run raw LLVM profiles were merged explicitly (SwiftPM did not create its
usual merged profile). All production files except `SpeechProbe.swift` meet the
95% line/function gate; that file has **61.43% lines / 100% functions** because
recognition is blocked. No source was excluded to conceal it.

The **38 focused tests / 8 suites** for captions, long audio, schema round-trips,
Speech lifecycle and model-independent native failure paths passed separate TSan
and ASan runs. Normal instrumentation was restored with the same focused set.
These are not a replacement for the failing full suite. Caption tests verify
visibility at the exact start, absence at the exact end, overflow refusal and
preserved audio; a 60-second PCM fixture yields 960,000 samples. Motion GUI
permissions still fail. No recognition asset reservation/download was performed.

The evidence and plan are separate from the earlier completed slice below; see
`CLIP-CAPTION-PLAN.md`. Do not apply its older counts to the pending source changes.

**Native editing/measurement slice verified; proprietary-app GUI integration is
not signed off.** Do not conflate export, decoded measurements, DTD validity,
Open Document delivery and completed editor import. No quality gate was waived.

## Earlier native-editing release and quality gates

- Swift 6.3.3, Swift 6 language mode, macOS 15+ deployment; MCP SDK 0.12.1.
- Executor's refreshed **user/default** native connection exposes **22 tools**.
  Changed tools were discovered/described before real-media calls.
- **127 test functions in 23 suites** passed, including parameterized cases,
  compiled CLI and actual SDK list/call handlers. Separate Thread Sanitizer and
  Address Sanitizer runs passed, then normal coverage instrumentation was restored.
- All **28 production Swift files** passed at least 95% line and function coverage:
  minimum **95.24% lines / 96.55% functions**. Only `.build` and `Tests` are excluded.
  LLVM emits no Swift branch counters here; region coverage is not branch coverage.
- Strict `swift format lint`, warnings-as-errors release build and `git diff --check`
  passed. Executor Skills checks passed TypeScript, Biome, **62 unit tests** and
  the setup/OAuth shell suites. No second Executor/runtime/dependency was installed.

Commands used in the Swift package:

```sh
swift format format --in-place --recursive Sources Tests Package.swift
swift format lint --strict --recursive Sources Tests Package.swift
swift test -Xswiftc -warnings-as-errors --enable-code-coverage
swift test -Xswiftc -warnings-as-errors --sanitize=thread
swift test -Xswiftc -warnings-as-errors --sanitize=address
swift test -Xswiftc -warnings-as-errors --enable-code-coverage
swift build -c release -Xswiftc -warnings-as-errors
```

The README documents `llvm-cov export` and the per-file gate. The native render
suite serializes shared hardware-codec work; independent model tests stay parallel.
Required checks must be repeated after subsequent source changes.

## Implemented native editing

- Source ranges, concatenation/reordering, rate 0.25–4, MP4 or audio-only M4A.
- Fit/fill, display-oriented crop, quarter-turn rotation and explicit canvas.
- Linear volume, fades, muting, timed additional/replacement/mixed audio.
- Clip overlap transitions: incoming-over-outgoing video dissolve and linear
  audio crossfade, with a shorter output timeline and bounded audio envelopes.
- SDR brightness/contrast/saturation and static single-line white bold titles.
  These share one extra encoding pass; input color normalization does not promise
  HDR preservation. Offscreen SwiftUI/CoreImage uses no new pointer/type-erasure
  exception. Text exceeding the canvas is refused, not silently truncated.
- Private new output directories, atomic publication, reusable `edit-request.json`.
- Bounded full-video decode, mono PCM measurement and selected-frame/region RGB
  measurement. CLI and MCP enforce the same strict request schemas.

### Operation-specific regression evidence

- A trailing empty audio interval was shortened by AVFoundation. Muting now keeps
  zero-gain samples, and publication checks tracks and actual timeline duration.
- Repeating one compressed sample was coalesced into one frame. A time-varying
  opacity pass generates the continuous fixture; assertions still require **30
  source frames and 45 concatenated frames**, not a weakened count.
- Colored fixtures verify rotation, grayscale/black/white/contrast behavior and
  title placement, with a control region outside the title. Audio survives effects.
- Tone fixtures measure unity/half gain and fade windows. Transition tests check
  red → mixed → blue and coherent audio levels before/during/after the overlap,
  including audio-only output. They do not claim general perceptual pitch fidelity.
- SDR input clamping removes extended-RGB residual color at brightness −1.
- Window endpoints round to the nearest sample; a 1.65–1.85-second window at 16kHz
  contains **3200**, not 3199 samples due to floating-point truncation.
- Native and display-transformed geometry are both budget-checked before frame
  generation; oversized scaling/translation metadata cannot bypass that check.
- An active-export test observes staging creation, cancels, joins the operation,
  and verifies staging deletion and absence of a published media/project file.
- Stdout/stderr capture tests cover cached file-size invalidation and fast overflow
  after exit. Capture size is polled, not an instantaneous filesystem quota.

## Real outputs through Executor

All viewing files are beneath **`~/Movies/Apple-Pro-Apps-Verification/`**. Its
`README.md` links every viewing result. Private source paths, hashes, job IDs and
raw diagnostics remain outside Git. No test played audio or recorded hardware.

### Initial nine-case matrix

Trim, reorder/concat, slowdown with fades, speedup, rotation, crop, audio extraction,
replacement and mix all exported and their projects read back successfully.
Eight MP4s decoded all **599 frames**; nine audio streams yielded **367,460** mono
samples. Sixteen selected video frames were measured. Original/source and output
hash comparisons passed. Replacement-audio windows before/after insertion had
zero RMS/peak; the middle window contained audio.

Viewing index: `native-edit-matrix.ju9hMS/README.md` under the Movies root.

### Color, title and dissolve

- Monochrome: **2 seconds / 60 frames**, audio present; three sampled regions had
  RGB differences at most 1/255. Input hash unchanged. Folder `native-color.1SW39S`.
- Title: **2 seconds / 60 frames**, audio present; title-region mean RGB increased
  from about 0.565 to 0.631, while the distant control region was unchanged at the
  measurement precision. Input hash unchanged. Folder `native-title.vEbspI`.
- Dissolve: two 2-second clips overlap by 0.5 seconds, producing **3.5 seconds /
  105 frames**. Pre/mid/post video/audio and both source reference frames were
  measured; both input hashes matched. Folder `native-dissolve.be316E`.

These are bounded measurements, not proof of every pixel, all channels or all effects.

### Compressor

One approved job, using a trusted Apple `.setting`, completed **Successful / 100%**
with native elapsed time 2 seconds. No resubmission was used. The installed 5.3 CLI
rejected legacy `-format`; **`-outputformat json`** worked and is regression-tested.

The export is **5.016 seconds, 608×1080, one audio track, 5,635,342 bytes**.
Subsequent Executor checks decoded **152 video frames** and **80,256 mono samples**.
The original source SHA-256 remained unchanged, and the viewing copy was moved
with byte comparison into `compressor-5sec.FWp0ul/sample.mp4`.

`compressor_inspect` returned zero with empty stdout on this source: invocation
only, not successful media analysis. Job success and actual output checks remain
separate requirements.

### FCPXML

`interchange_write` created a four-clip, 9.5-second timeline referencing the existing
rendered variants. `fcpxml_validate` accepted it against the selected installed
Final Cut Pro **DTD 1.14**. An XML-well-formed but DTD-invalid control was refused.
XPath read-back returned four expected clips, and the XML hash was unchanged.

File: `fcpxml-timeline.GJ6Jvp/rendered-variants.fcpxml`. It is cut-level interchange;
color/title/dissolve effects are baked into referenced MP4s, not editable native
FCP effect parameters. **Application import was not attempted.** The validator
reports `importVerified: false` and `mediaReferencesVerified: false`; DTD validation
alone cannot prove external media, visual fidelity or application behavior.

## Boundaries and remaining user-dependent work

- Latest **Executor-hosted** UI result: Screen Recording granted; **Accessibility
  and Event Synthesizing not granted**. GUI-dependent operations stop here. Chat
  approval is not TCC consent; no TCC database or blanket policy was changed.
- Actual isolated FCP import, Motion compatibility/rendering and Logic/MainStage
  assignments remain unverified. No live project/concert mutation, routing change,
  purchase, license acceptance or sound-library installation was performed.
- There is no universal all-functions Apple Pro Apps API. Static titles are not
  animated text/subtitles; JSON projects are not native FCP/Motion/Logic projects.
- Audio measurements normalize to mono PCM16/16000 Hz: not per-channel, LUFS,
  clipping-policy, pitch-preservation or perceptual-quality certification.
- Video frame-rate settings request compositor cadence; sparse/VFR sources and
  encoder timing must be measured rather than assumed constant-frame-rate.
- The 300-second render and 60-second measurement limits govern the disposable
  direct child. Hard termination may leave private staging; they are not a promise
  to synchronously stop arbitrary descendants/XPC services. Never retry a mutation
  blindly. In-process cooperative export cancellation has separate tested cleanup.
- Approved native exceptions remain limited to `NATIVE-BOUNDARIES.md`. No new unsafe
  continuation, unchecked Sendable, raw pixel access or arbitrary dictionary was added.
- Seven pinned Swift dependencies previously had no OSV advisory matches; this is
  database evidence, not a security guarantee. Optional sync checks passed 173 tests
  and encrypted-transfer smokes; second-physical-Mac restoration remains untested.
- Unrelated working-tree changes were preserved. This Apple task is not committed
  or pushed; registration/deployment is not a Git commit or production sign-off.
