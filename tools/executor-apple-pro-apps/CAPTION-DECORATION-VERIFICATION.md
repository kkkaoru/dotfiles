# Caption decoration and dual-duration verification

## Delivered evidence and limits

The 26-tool feature release passed **179 tests / 32 suites**, all **31 production
files** at >=95% lines/functions (minimum 95.24% / 96.77%), separate full TSan/ASan,
restored normal full coverage and a warnings-as-errors release build. A subsequent
refactor shares unchanged mask/cue count constants between schemas and domain
validation; its final full gate also passed all 179 tests, both sanitizers, restored
coverage, release refresh and source/output hashes. No quality exception or exclusion was added.

Two real outputs are indexed in the user's Movies verification directory:

| Evidence | 60 seconds | 90 seconds |
|---|---:|---:|
| Output | 720×1280, 30fps | 720×1280, 30fps |
| Full end-of-stream video decode | 1800 frames | 2700 frames |
| Whole mono PCM16/16kHz decode | 960,000 samples | 1,440,000 samples |
| Output RMS | 0.08254 | 0.08043 |
| Output peak | 0.96085 | 0.96085 |
| Caption midpoints sampled by local OCR | 5/5 | 8/8 |
| Isolated SE onsets / silent control windows | 5/5 | 8/8 |

White 26px bold text has a 3px expanded-alpha black outline. An opaque black mask
covers the observed source-subtitle band; custom bottom positioning places new
text over it, not at an unrelated bottom location. Source voice gain is 0.65;
SEs are 80ms/880Hz smoothly windowed tones at peak setting 0.08, precomposed into
one track to avoid adding a track per cue. Metadata never claims background repair.

Adjacent-frame OCR confirms text switches at 7.2667s in both versions, 44.2333s
in the 60s version, and 84.1333s after a caption gap in the 90s version. The entire
masked band measures black during the sampled gap. Beginning/middle/end mask-edge
regions are black, and upper control regions agree with corresponding source
frames to about 1/255 average RGB. Original and output artifact hashes matched.
These are selected-region/OCR checks, not all-pixel equality or flawless OCR.

ASR used three 30-second source selections. Three overlapping-context re-recognitions
and seven source-OCR frames corroborate some content but expose disagreements in
names, disfluencies and the clip ending. The same recognizer can repeat an error;
burned-in captions and OCR are not ground truth. No human listening review or
speech accuracy percentage is claimed. Original text is kept, marked unreviewed,
with an explicit private evaluation and correction candidates, not silent edits.
Output OCR itself occasionally misses characters, so it is not a substitute for
recognition-content evaluation. Audio checks are mono, not channel/perceptual proof.

Native-clock/container-end discrepancies within one native result tick are clipped
inward, preserving `originalDurationSeconds`. Larger overshoots, invalid clocks
and nonpositive intervals fail; strict transcript construction still rejects the
raw out-of-range input. Subtitle times then snap to output frames and SE times to
PCM samples, with both original and mapped data retained privately.

## Regression discoveries

- A slowed continuous fixture lost one frame in the effects pass at both 60/90s.
  Apple's supported mutable CI composition now uses the explicit recipe cadence;
  1800/2700 expectations were not reduced.
- CLI integration tests must invoke the product binary rather than XCTest's main
  executable. The injected boundary still performs real CLI serialization/I/O.
- Cue write failure is injected after project publication; the operation directory
  is removed and the original error retained.
- Frame-generator ownership transfers with checked `sending`, not unchecked
  sendability. OCR rectangles that slightly exceed the image are clipped and flagged;
  infinite/null/outside rectangles are rejected.

## Swift rules review

- **SW01–04:** Existing SwiftPM/Swift 6/macOS 15+ and pinned MCP SDK retained. Plan
  preceded changes. Failures remained failures until reproduced and corrected;
  no skips, warning suppressions, reduced frame assertions or retry-to-hide defects.
- **SW05–10:** Typed Codable values and bounded optional style fields; checked
  Sendable ownership, narrow access and named shared resource caps. No force casts,
  force unwraps, new type erasure or raised deployment target. Vision uses macOS 15
  APIs; Speech keeps its macOS 26 guard.
- **SW11–15:** Shared geometry/source preparation and canonical schema/domain caps;
  explicit local ASR clock normalization separated from strict input validation.
  Preserve errors and refuse invalid text/timing/geometry/budgets.
- **SW16–18:** New outputs are operation-owned, non-overwriting and private. Cue
  publication rollback is tested. No new ARC/unsafe-pointer exception was introduced;
  existing NATIVE-BOUNDARIES.md exceptions remain unchanged.
- **SW19–26:** Existing bounded Dispatch-backed actors retained; only SwiftUI glyph
  rasterization is MainActor. Vision and native media use native async APIs with
  cancellation and disposable-process deadlines. No detached tasks, unchecked
  isolation or unowned captures. Cancellation/cleanup tests and full sanitizers run.
- **SW27–30:** Offscreen SwiftUI has no live UI state or side-effectful body.
  Diagnostics are English and omit recognized text/private paths. No speculative
  low-level allocation or new dependency; safe value shifts encode PCM.
- **SW31–36:** Swift Testing covers masks/styles, limits, visible outlines, crop
  mapping, cue waveform/onset/silence, failure cleanup and native timing edges.
  Both 60/90 combined synthetic video/audio cases and real outputs are checked.
  Per-file coverage is enforced; LLVM does not supply Swift branch counters here.
- **SW37:** This report records the diff review, commands, evidence scope and passed
  final constant-sharing audit. No unsupported verification is represented as passed.

## Verification commands

```sh
swift format format --in-place --recursive Sources Tests Package.swift
swift format lint --strict --recursive Sources Tests Package.swift
swift test -Xswiftc -warnings-as-errors --enable-code-coverage
xcrun llvm-cov export .build/debug/AppleProAppsPackageTests.xctest/Contents/MacOS/AppleProAppsPackageTests \
  -instr-profile .build/debug/codecov/default.profdata -summary-only \
  -ignore-filename-regex='(/\.build/|/Tests/)'
# Require a nonempty production-file set and every line/function percentage >=95.
swift test -Xswiftc -warnings-as-errors --sanitize=thread
swift test -Xswiftc -warnings-as-errors --sanitize=address
swift test -Xswiftc -warnings-as-errors --enable-code-coverage
swift build -c release -Xswiftc -warnings-as-errors
git diff --check
```

Executor tool schemas were inspected before real calls; success required both the
Executor envelope and MCP result plus artifact evidence. Raw transcripts, paths,
hashes and detailed native diagnostics remain outside Git. No video/audio upload,
playback, TCC changes or incidental model installation was performed in this phase.
