# Verification status — 2026-09-14

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
