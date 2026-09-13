# Apple Pro Apps — native Swift MCP through Executor

**Machine integration first, UI last.** The Swift source catalog now defines 22 typed
MCP tools, including the editing expansion, for the existing Executor Desktop runtime.
See the verification report for the most recently confirmed deployed catalog. The separate optional
Peekaboo integration handles UI gaps. There is no new Shell implementation,
parallel Executor runtime, permanent daemon, model-provider account or API key.

**Verification is incomplete.** See [VERIFICATION.md](VERIFICATION.md) for actual
results, current quality gates and remaining application-specific verification. The user's native-boundary approval is
scoped in [NATIVE-BOUNDARIES.md](NATIVE-BOUNDARIES.md); it does not waive quality
checks. Registration is not production sign-off.

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
| `media_verify_video` | Full video decode to end-of-stream | ≤30 seconds / 1800 frames; does not verify audio or every effect |
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
