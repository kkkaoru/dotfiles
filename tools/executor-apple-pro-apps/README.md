# Apple Pro Apps — native Swift MCP through Executor

**Machine integration first, UI last.** The Swift source catalog now defines 34 typed
MCP tools, including the editing expansion, for the existing Executor Desktop runtime.
See the verification report for the most recently confirmed deployed catalog. The separate optional
Peekaboo integration handles UI gaps. There is no new Shell implementation,
parallel Executor runtime, permanent daemon, model-provider account or API key.

**Verification is incomplete.** See [VERIFICATION.md](VERIFICATION.md) for actual
results, current quality gates and remaining application-specific verification. The user's native-boundary approval is
scoped in [NATIVE-BOUNDARIES.md](NATIVE-BOUNDARIES.md); it does not waive quality
checks. Registration is not production sign-off.

## Replay editing development (not yet deployed)

See [REPLAY-EDITING-PLAN.md](REPLAY-EDITING-PLAN.md). The working source extends
`video.masks` with optional `blurRadius` (1–64 output pixels) and paired
`startSeconds` / `endSeconds` (half-open output-time intervals). Missing radius
retains black concealment; missing times retain whole-output application. Blur is
blended by the existing opacity and cropped to the requested rectangle before new
titles/captions. It does not guarantee unreadability or reconstruct the background.
The existing eight-mask and bounded-render limits remain unchanged. Full tests,
per-file coverage, sanitizer checks, release deployment and real 60/90-second
acceptance must pass before this becomes a verified deployed feature. Reference
BGM removal, speech-aware cutting and full-replay orchestration remain planned,
not implemented by this mask change.

## Motion animation compositing prototype

The deployed recipe adds optional `additionalVideo`: up to 16 output-timed,
video-only placements with selection/rate, geometry and opacity. Later entries
are on top; alpha is composited, while opaque sources cover the base. Layer audio
is ignored and global effects run afterward. This is intended to combine a
Motion-rendered animation with native AVFoundation editing, not to represent an
AVFoundation timeline as a Motion project. All 230 tests, 37 per-file coverage gates,
separate full sanitizers and release verification passed. Real 60/90-second
**diagnostic opaque-band** composites fully decoded and passed selected temporal,
audio and source-control checks. Synthetic ProRes alpha compositing passed, but
actual Motion-to-ProRes export failed. Template text is unchanged; experimental
Motion copy-edit authorization is pending. This is not finished animation-editing
acceptance. See [MOTION-NATIVE-PLAN.md](MOTION-NATIVE-PLAN.md).

## Follow-up deployment boundary

The verified deployment has **26 tools**, adding `audio_cue_track` and local
`video_text_recognize` to caption rendering and on-device transcription. The latest
feature gate passed **179 tests / 32 suites**, all 31 production-file gates and
separate full sanitizers. Real decorated 60/90-second outputs were decoded fully,
with audio, caption boundaries, SE/control windows and source-preservation checks.
Recognition remains unreviewed; an explicit evaluation records disagreements, not
a fabricated accuracy score. Executor-hosted permissions now pass. An unchanged
Motion template has rendered through Compressor CLI; edited TikTok animation
acceptance remains pending (see [MOTION-NATIVE-PLAN.md](MOTION-NATIVE-PLAN.md)).
See [CAPTION-DECORATION-PLAN.md](CAPTION-DECORATION-PLAN.md) and
[CAPTION-DECORATION-VERIFICATION.md](CAPTION-DECORATION-VERIFICATION.md).

- `video.captions`: up to 120 ordered/nonoverlapping `{text,startSeconds,endSeconds}`
  output-time cues, with half-open intervals. Text is automatically wrapped, white
  and bold on a translucent bottom box; overflow is refused. Minimum canvas 160×90.
  Static titles and captions share a 16-megapixel aggregate bitmap limit and the
  existing effects pass. `video.captionStyle` adds a black expanded-alpha outline
  (0–6px), box opacity (0–1), optional font size (16–64px) and bottom margin. Text
  must fit above the selected margin. `video.masks` supplies up to eight black
  rectangles in top-left output pixels, applied before text, with opacity 0–1.
  Concealment does not reconstruct the original background. Cues are supplied
  text, not automatic recognition.
- `audio_transcribe`: local M4A/WAV/AIFF/CAF up to 60 seconds, macOS 26+, explicit
  locale and already-installed SpeechTranscriber assets. It returns final phrases,
  approximate times, `onDevice: true` and `humanReviewed: false`. No download,
  microphone, cloud fallback, speaker identification or forced word alignment.
- `audio_measure.maximumDurationSeconds`: opt-in up to 120 seconds; omission
  retains 30 seconds, with fixed PCM byte/window and subprocess limits.

`installedLocales` alone does not prove app-scoped readiness. After user-approved
bootstrap installation, a separate application's Japanese status still reported
`supported` until that app reserved the locale. Use `speech_locale_reserve` only
with approval for the selected locale; it retains existing reservations and never
downloads/releases/evicts. Check `readyForTranscription`, then call `audio_transcribe`.
Missing assets still need separately approved setup; creating an
`assetInstallationRequest` can itself reserve locales and is not read-only.

Apple references: [SpeechAnalyzer](https://developer.apple.com/documentation/speech/speechanalyzer),
[privacy](https://developer.apple.com/documentation/speech/asking-permission-to-use-speech-recognition),
[asset requests](https://developer.apple.com/documentation/speech/assetinventory/assetinstallationrequest(supporting:)).

## Motion text and local Whisper update

The current catalog adds `motion_text_inspect`, `motion_text_copy` and
`audio_transcribe_whisper`. Discover/describe them through Executor before use.
Motion text copying is source-SHA-bound and updates the text, character objects
and style-run length together in a new file. The deliberately narrow adapter
supports observed ozml 4.0, single-style, neutral-kerning, single-line BMP text;
unsupported rich formatting and non-BMP text fail rather than being flattened.
It does not generate arbitrary scene factories, animate parameters or prove a render.

Whisper uses explicit installed local Core ML/tokenizer directories, no download
or cloud fallback, and bounded disposable-child inference. Native Float alignment
timestamps are recovered as integer 20-ms ticks, avoiding CMTime truncation of
values such as 0.9. Speech accuracy remains unreviewed.

The current update passed 274 tests / 59 suites, strict format/warnings, separate
full TSan and ASan, restored normal coverage for all 47 production files (minimum
95.238% lines/functions), and release build in verification job 216. No runtime
`warning:` messages were found in that job. These gates are separate from actual
media acceptance. Executor discovery confirmed all three tools afterward.
A same-length Japanese Motion copy rendered via the official Compressor CLI and
fully decoded (180 frames); different-length adapter and real Whisper comparisons
are ongoing. One-minute studies and complete new captioned replay output remain
unfinished. See the Motion/Whisper plans for the latest artifact evidence.

## Build / register

Requires macOS 15+, Swift 6.2+ and the existing Executor installation.

```sh
cd tools/executor-apple-pro-apps
swift build -c release -Xswiftc -warnings-as-errors
.build/release/apple-pro-apps setup \
  --repo /absolute/path/to/dotfiles --approve-registration
```

To also register the existing Homebrew Peekaboo as a **separate fallback**:

```sh
.build/release/apple-pro-apps setup \
  --repo /absolute/path/to/dotfiles --approve-registration --with-ui \
  --peekaboo /opt/homebrew/bin/peekaboo
```

`--approve-registration` permits only adding these MCP integrations and creating
no-auth connections. It does **not** install an allow-all policy, approve nested
prompts, grant TCC permissions, accept licenses, record audio, edit an existing
project or configure MIDI routing. Existing registrations/connections are retained
rather than overwritten. A zero CLI exit from Executor can still contain a pause
or a tool error; setup checks the result and fails closed.

The native MCP SDK is pinned at **0.12.1**; commit `Package.resolved` with dependency
changes. No Bun package or duplicate Peekaboo binary is installed here. This Mac's
existing `/opt/homebrew/bin/peekaboo` is 4.3.4; the source checkout at
`/Users/kkk4oru/ghq/github.com/openclaw/Peekaboo` is available for implementation
research. It is not modified or automatically built by this package.

Executor registers the compiled executable directly:

- `apple-pro-apps`: `apple-pro-apps serve`
- `apple-pro-apps-ui`: `apple-pro-apps serve-ui --repo <checkout> --peekaboo <existing binary>`

Persistent stdio (`spawnPerCall: false`) preserves MCP/Peekaboo snapshot lifetime.
The optional launcher replaces itself with the existing native Peekaboo process;
there is no shell interpolation or new Bridge listener. Do not add duplicate
servers to `.mcp.json` or Pi. On another Mac, build locally and re-register; if a
synced/existing registration points to another checkout/build, correct its command
and arguments in Executor's management UI before verification. Native app data,
licenses and MIDI/audio configuration are not replicated by this setup. Settings
sync maps the optional installed `peekaboo` executable independently through PATH
on each Mac; importing a UI registration without that executable fails closed.
The native release binary must still be built for each Mac's architecture.

## Native surface

| Tools | Implementation | Important limits |
|---|---|---|
| `media_edit_plan`, `media_edit`, `media_project_read` | Typed recipes, AVFoundation timeline rendering and reusable project sidecars | Native media editing, not proprietary editor timeline mutation |
| `audio_cue_track` | Local synthesized 16kHz mono PCM16 WAV | New private output, ≤120s/120 nonoverlapping 80ms cues, peak setting ≤0.25; mix with headroom |
| `video_text_recognize` | Local Vision Japanese/English OCR | 1–8 frames, ≤64 lines/frame, ≤32KiB text; actual times and bounded crop-mapped rectangles; not ground truth |
| `speech_locale_reserve` | Explicit app-scoped Speech locale reservation | Requires approval; no downloads, release or eviction; returns readiness |
| `audio_transcribe` | On-device SpeechAnalyzer/SpeechTranscriber, installed locale only | macOS 26+, ≤60s audio file, approximate phrases; no download/cloud fallback or human-review claim |
| `media_verify_video` | Full video decode to end-of-stream | Defaults to 30s/1800 frames; explicit budgets up to 120s/7200; not audio or every effect |
| `audio_measure` | Native conversion and bounded PCM analysis | Mono PCM16/16000 Hz; RMS/peak/zero crossings, not per-channel/LUFS/pitch proof |
| `video_frame_measure` | Managed CoreImage region RGB averages | 1–8 samples, display-oriented top-left pixels; selected-frame evidence only |
| `media_inspect` | AVFoundation metadata and first-frame BGRA decode in a deadline-limited child | Scalars only, no playback/capture; not full-stream or editor import validation |
| `app_capabilities` | AppKit/LaunchServices + actual bundle metadata | No app launch, screen access or license proof |
| `app_open_document` | Native Open Document delivery to exact app edition | Acceptance is not completed import; existing project/import state can change |
| `fcpxml_validate` | Selected installed Final Cut Pro DTD + system xmllint | Private snapshots, no external schema references/network/catalogs; not import or media-reference validation |
| `interchange_inspect`, `interchange_query` | Foundation XML, bounded UTF-8 files/XPath | No DTD validation; entities/external DTDs refused |
| `interchange_write`, `interchange_patch` | New-file publication; existing leaf/attribute edits | Never overwrite; Motion ozml is undocumented and opt-in |
| `compressor_inspect`, `compressor_submit`, `compressor_status`, `compressor_control` | Official installed Compressor CLI with argument arrays | Explicit files/preset/ID; no arbitrary flags, service reset or cancel-all |
| `midi_file_create` | Standard MIDI type-0 encoder with tempo and paired notes | File only, no playback; import separately into Logic |
| `midi_destinations`, `midi_send` | CoreMIDI, MIDI 1 UMP events | Exact destination ID/name; CC/program/pitch bend; routing must be verified |
| `osc_send` | Typed OSC Float32 packet over loopback UDP | Explicit port/path, no scanning, broadcast, remote host or receiver acknowledgment |

Mutation input schemas are enforced at runtime, including nested unknown fields,
enums, string sizes, numeric ranges and counts. Directly written interchange/MIDI
files are mode 0600 and published atomically without replacing an existing path,
even a dangling symlink. Native encoder media is contained in private 0700 output
directories; its file mode is encoder-managed.
XML is capped at 8 MiB; query results at 10 fragments of 2048 characters. Motion
writes require `allowUndocumentedFormat: true` and a known-good version-compatible
project/template. This is not an official Motion schema API.

Compressor outputs go into a new private UUID directory beneath the requested
output directory. Presets must be trusted: their own behavior is not sandboxed by
the CLI wrapper. Commands have a 30-second process deadline, private stdout/stderr
capture and a polled 1 MiB capture limit (not a hard instantaneous disk quota).
Only bounded stdout is returned; native exception/stderr details are not echoed.
The render itself can outlive submission: store its returned ID/path and use the
bounded status tool. Failure/timeout may follow partial dispatch; never retry a
submission blindly. Setup uses the existing Executor wrapper so scope/sync
behavior remains consistent with this repository.

## Video and audio editing

`media_edit_plan` validates a recipe and reports output-time spans without claiming
that source files were loaded. `media_edit` produces a new MP4 when `recipe.video`
is present, or M4A when it is absent. A saved `edit-request.json` beside the result
can be read with `media_project_read`, revised and rendered to another new output.

- Ordered clips select source `startSeconds`, `durationSeconds` and `rate` (0.25–4).
  All three selection fields are explicit; concatenation/reordering follows array order.
- Optional clip `transitionInSeconds` overlaps adjacent clips: 0–5 seconds, at most
  half either adjacent output duration; the first clip must have zero/no transition.
  Video uses incoming-over-outgoing cross-dissolve; audio uses linear crossfades.
  The timeline shortens by each overlap. Explicit audio fades and transition fades
  use the longer duration; an overlapping in/out envelope within one clip is refused.
- Optional audio adjustments specify linear `volume` (0–1), `fadeInSeconds` and
  `fadeOutSeconds`. Fades and additional audio `offsetSeconds` use output time.
  Omitted adjustments mean unity gain/no fades. Muting retains zero-gain source
  samples so audio-only replacement does not silently shorten the timeline.
- Video settings explicitly choose even width/height, frame rate and fit/fill.
  Geometry supports 0/90/180/270-degree rotations and display-oriented pixel crops.
  Invalid/out-of-source crops and unsupported inverse crop geometry fail rather
  than silently selecting extra pixels.
- Optional `video.color` specifies `brightness` (-1–1), `contrast` (0–4) and
  `saturation` (0–2); identity is 0/1/1. It clamps input working RGB to SDR 0–1
  and adds an encoding pass after geometry/mixing. This is not HDR-preserving;
  omission keeps the original single-pass path unless titles are requested.
- Optional `video.titles` adds up to eight static, single-line, white bold system
  titles: `text`, `x`, `y`, `fontSize` (8–128). Origins use top-left output pixels;
  text is bounded to 120 characters/1024 UTF-8 bytes. Oversized text is refused,
  not clipped or truncated. Managed offscreen SwiftUI renders glyphs without a
  window or screenshot. Titles and color share one additional encoding pass.
  This is not animated text, subtitles, Motion templates or font installation.
  Inspect the deployed schema before use.
- Limits: 60 clips, 16 additional audio layers, 600 output seconds, 4K pixel budget,
  1–60 fps. The native child has a 300-second processing deadline. A timeout may
  leave private staging; it is not proof of completion and is not automatically retried.
- Each result is published under a new private `edit-UUID` directory. For this
  user's viewing examples use the absolute form of
  `~/Movies/Apple-Pro-Apps-Verification/` as `outputDirectory`.
- Current completion checks validate native export, tracks and duration. They do
  **not** prove full-stream decoding, every requested effect, or import into FCP/
  Motion/Logic. The expanded measurement matrix is tracked in EDITING-PLAN.md.

CoreMIDI/OSC are not app-isolation boundaries. A shared route can reach other apps
or hardware, and OSC send success is not semantic acknowledgment. No virtual port,
IAC device, network MIDI session, control-surface assignment or OSC listener is
created. Confirm the actual user's routing before any live control. Offline MIDI
file generation is not evidence that live routes work or that MainStage imports
MIDI files as concerts.

## Verify through Executor

```sh
./scripts/executor tools search app_capabilities --namespace apple-pro-apps --limit 2
./scripts/executor tools describe '<exact returned path>'
# Call the discovered path with its described arguments, normally {}.
./scripts/executor tools search interchange --namespace apple-pro-apps --limit 5
```

Discover/describe each tool instead of guessing schemas. Inspect both the Executor
result and MCP `isError`/structured content, not just process status. Read-only
inventory and synthetic file tests are distinct from real app import/render/live
MIDI validation. Never use an existing project as a test fixture.

The shared `local-skills` MCP serves these repository-managed skills:
`apple-pro-apps`, `apple-motion`, `apple-compressor`, `apple-final-cut-pro`,
`apple-logic-pro`, `apple-mainstage`. Load the common skill, then the relevant app
skill. Reload agent resources as needed. Skill text is not permission to mutate
projects or bypass Executor approval.

## Optional GUI fallback

The `apple-pro-apps-ui` namespace uses Peekaboo's AX, screenshot, menu, input,
window/dialog, clipboard and state-verification tools. Dedicated shell, browser,
autonomous-agent and recording tools are not allowlisted. This is a tool-name
filter, not an argument sandbox: inspect schemas and do not select optional AI
analysis/provider features without explicit authorization. Foreground mode is explicitly
enabled for timeline/canvas dragging; it can move the shared physical cursor.
This is general macOS automation, **not** a five-app sandbox. Keep operations scoped
to the approved app/window, serialize with the user, and keep captures private.

Check `permissions` **through Executor's fallback MCP**, not standalone Peekaboo:
a standalone CLI may report an unrelated Bridge host. The user grants Screen &
System Audio Recording and Accessibility to the reported host; keyboard/physical
pointer delivery also needs Event Synthesizing. Never manipulate TCC storage or
automate consent. Do not kill other users' Peekaboo hosts. Screenshots and inline
base64 results must not be committed or dumped unbounded into logs.

## Approved Demucs model development

The current native Demucs integration tests require `DEMUCS_TEST_MODEL` to point
to an explicitly approved, checksum-verified compiled `.mlmodelc` directory.
They never download or install model assets and fail clearly when the fixture is
absent; there is no skip or fallback. The approved archive SHA-256 is
`0fbb941e15a5b2fa425d14fe630ed4c14b6dee72780c1f5b2b05f58803bce5f7`.
Despite its F32 filename, the inspected model uses Float16 spectral/waveform
inputs and outputs; the native adapter validates the actual fixed contract and
uses CPU-only inference. Models and private sample media must not be committed.
This is under development, not yet a verified real-audio separation feature.

## Quality checks

```sh
cd tools/executor-apple-pro-apps
swift format format --in-place --recursive Sources Tests Package.swift
swift format lint --strict --recursive Sources Tests Package.swift
swift build -Xswiftc -warnings-as-errors
swift test -Xswiftc -warnings-as-errors --enable-code-coverage
xcrun llvm-cov export \
  .build/debug/AppleProAppsPackageTests.xctest/Contents/MacOS/AppleProAppsPackageTests \
  -instr-profile .build/debug/codecov/default.profdata -summary-only \
  -ignore-filename-regex='(/\.build/|/Tests/)' \
  | jq -e '(.data[0].files | length) > 0 and all(.data[].files[]; .summary.lines.percent >= 95 and .summary.functions.percent >= 95)'
```

Swift 6 strict concurrency, Swift Testing, typed requests, runtime schema checks,
non-overwrite/XXE tests, MIDI/OSC encoding tests, process deadline tests and mocked
Executor registration tests provide the quality boundary. Native adapter tests
also exercise the compiled CLI with synthetic non-Executor/non-Peekaboo fixtures,
and MIDI/OSC delivery only to test-owned temporary endpoints, disposed afterward.
No test launches an Apple production app, grants permissions or changes a real
project. Real imports, encoding and live sound need separately approved tests.
The required gate checks **95% line and function coverage of every production
Swift file**, including the executable adapter. The editing/measurement expansion passed;
subsequent changes must pass again before verified completion. This
LLVM report emits no Swift branch counters; region coverage is not a substitute
for branch coverage. Any unsupported-check exemption requires explicit approval.

## What is not promised

There is no universal public all-functions API across these five apps. FCPXML is
interchange, not arbitrary live timeline control; Custom Share Destinations and
Workflow Extensions require separate app/extension work. Motion has no established
public headless project renderer. Logic/MainStage control routing is user-specific,
and MainStage has no documented native OSC listener. Plugin/protected UI and
licensing may still require a human. Native delivery, experimental file editing,
UI fallback and verified completion are always reported separately.

## Research

Apple references are also linked in each skill. Sources consulted:

- https://developer.apple.com/documentation/professional-video-applications/sending-data-programmatically-to-final-cut-pro
- https://developer.apple.com/documentation/professional-video-applications/importing-fcpxml-data
- https://support.apple.com/guide/compressor/cpsr9be73312/mac
- Installed Compressor Creator Studio 5.3 `-help` (submission/monitoring/control flags)
- Installed Final Cut Pro Creator Studio 12.3 `ProEditor.sdef` (library inspection, not a full editor API)
- https://support.apple.com/guide/motion/welcome/mac
- https://support.apple.com/guide/logicpro/osc-message-paths-ctlsf67f4bdc/mac
- https://support.apple.com/guide/logicpro-css/control-surfaces-overview-ctls036b3e21/mac
- https://help.apple.com/pdf/mainstage/en_US/mainstage-user-guide.pdf
- https://peekaboo.sh/MCP.html and https://peekaboo.sh/commands/mcp.html
- https://github.com/modelcontextprotocol/swift-sdk

The skills.sh leaderboard and Final Cut Pro/Logic Pro CLI searches did not surface
a well-established skill covering this native Executor setup and all five apps;
these skills are locally authored rather than importing unrelated/low-use skills.
