# Approved native interoperability scope

The user explicitly approved the requested exceptions on 2026-09-13 and requested
conformance with the Swift coding skill. This approval is **not** a waiver of
coverage, warnings, sanitizer checks, cancellation safety, or error handling.

## SW04 / SW18 exception

Only the existing necessary CoreMIDI and POSIX framework boundaries are covered:

- `MIDIControl.swift`: the CFString returned by CoreMIDI is consumed using its
  documented retained ownership; the MIDI event list is stack-owned for the
  synchronous send. No pointer survives its borrowed scope or an `await`.
- `Files.swift`: opened-descriptor metadata prevents path-validation races;
  O_NOFOLLOW/O_EXCL and atomic hard-link publication prevent following/replacing
  a destination. Foundation now owns byte-buffer writes; handwritten raw-buffer
  writes were removed. Staging files belong to this operation, never to another project.
- `OSC.swift`: stack-owned IPv4 sockaddr is rebound for the synchronous sendto
  call with its actual size. The packet buffer remains borrowed until sendto
  returns. The destination is loopback only, and the descriptor is closed.
- `Main.swift`: each strdup allocation belongs to argv, which is nil-terminated.
  A successful exec replaces the process; a failed exec frees each allocation.
- `NativeTransportTests.swift`: matching socket receiver boundaries use only
  test-owned descriptors and bounded local buffers.

## Native UI automation boundary (approved 2026-10-03)

The user explicitly approved, for moving Peekaboo-only work into this package,
SW08/SW18 exceptions confined to `Sources/ProAppsCore/UILiveBackend.swift`:

- Accessibility: `AXUIElementCopyAttributeValue` returns `CFTypeRef`. Every value
  is checked with `CFGetTypeID` before conversion into the closed
  `UIAttributeValue` enum; arrays are bridged through `[AnyObject]` and rejected
  unless every item is an `AXUIElement`. Single CF elements/AXValues are
  converted by a conditional one-element array bridge, never a forced cast.
  `AXValueGetValue` writes into a stack-local `CGPoint`/`CGSize`.
- Text Input Sources: `TISCopy…` results use the create/copy rule
  (`takeRetainedValue`); `TISGetInputSourceProperty` pointers use the get rule
  and are consumed immediately with `Unmanaged…takeUnretainedValue`. No pointer
  is stored or crosses an `await`.
- CoreGraphics/ScreenCaptureKit: windows are matched by owning PID plus title/frame;
  ambiguity fails instead of guessing. PNG encoding uses ImageIO with a managed
  `NSMutableData`.

Scope limits: only the five supported Pro apps' documented bundle IDs are
targetable through MCP (`UITarget`). Operations never synthesize keyboard/pointer
input and, except the Final Cut Pro export scope below, never activate an app or
focus a window; each result reports the frontmost app before/after. Handles live only inside one `ui-native` child process.

Compensating checks: the core logic runs against a synthetic fake tree; the live
boundary is exercised against the synthetic accessory fixture
`Tests/UIFixture` (not an Apple production app) for attribute conversion, AXPress,
AXValue/AXSelected writes, menus, background capture, launch/terminate and
non-mutating input-source re-selection. Per-file 95% coverage, TSan and ASan apply.

Removal condition: replace these conversions when Apple provides typed Swift
Accessibility/TIS APIs with equivalent lifetime guarantees.

## Final Cut Pro live control scope (approved 2026-10-08)

The user asked for direct live timeline control and export in Final Cut Pro and,
when shown that sharing is disabled while the app is inactive, explicitly chose
"activate Final Cut Pro only during export" (keyboard/pointer synthesis stays
forbidden). This extends the native UI boundary above as follows:

- `UISettableValue.elements` writes `AXSelectedChildren` / `AXSelectedRows` as one
  CFArray of handles issued by the same backend instance (`lookup` rejects
  unknown handles). Only the `fcp_*` operations use it; `ui_set_value` still
  accepts strings and booleans only. Element lists are not JSON-encodable, so
  they never cross the child-process boundary.
- `fcp_project_open` and `fcp_export` write `AXFocused` on a Final Cut Pro
  browser group / the timeline so the menu command applies to that panel.
  Generic `ui_set_value` still cannot set focus.
- `UIBackend.activate(pid:)` (NSRunningApplication.activate) is called only by
  `fcp_export`, which requires `allowForeground: true`, checks `AXFrontmost`, and
  restores the previously frontmost process on success and on every failure
  (including cancellation), reporting `foregroundRestored`.
- Final Cut Pro Save panels are navigated by selecting the sidebar home row and
  one column-browser item per path component; destinations are limited to
  existing, visible, symlink-free folders inside the home folder, and existing
  files are refused instead of confirming a replacement.

Compensating checks: a synthetic Final Cut Pro tree (FakeFinalCut) covers every
operation and each export failure with cleanup/focus restoration; the live
backend test writes `AXSelectedRows` on the accessory fixture and exercises
`activate` only on an impossible pid so test runs never move the user's focus.
Real acceptance ran against a disposable library only (see VERIFICATION.md).
Removal condition: replace activation if Final Cut Pro enables Share while
inactive or a supported export API (for example a Workflow Extension or Custom
Share Destination flow) supersedes the Share dialog.

## Final Cut Pro effects and keyframes scope (approved 2026-10-08)

The user asked for effect and keyframe support and approved (a) activating Final
Cut Pro only during an operation, with the same safeguards as export, and (b)
the clipboard-based carrier route. Accordingly:

- `fcp_xml_export` uses the same `inForeground` helper as `fcp_export`
  (activate, then restore the previous app on success and every failure).
- `fcp_effects_paste` imports a generated carrier through
  `UIBackend.open(document:applicationAt:)`, which asks LaunchServices not to
  activate the app; if Final Cut Pro still comes forward the previous app is
  restored. Carriers go to a disposable `Claude-Effect-Carriers` library inside
  the caller's work directory, never into the user's library. The tool runs
  Edit > Copy and Edit > Paste Effects, so it overwrites the user's clipboard.
- Inspector writes reuse `AXValue` + `AXConfirm` on value fields and `AXPress` on
  enable checkboxes; keyboard and pointer synthesis remain forbidden (effect
  double-clicks and keyframe buttons are therefore not used).

Compensating checks: synthetic inspector, carrier import and Export XML panel
(FakeEffectsUI) with every failure path; carrier XML is DTD-validated against the
installed Final Cut Pro in the native tests; real 60/90-second acceptance is in
VERIFICATION.md.

## Final Cut Pro foreground operations (extended 2026-10-08)

Under the same user approval ("activate only during an operation"), the
foreground helper now also wraps `fcp_project_open`, the copy/paste part of
`fcp_effects_paste` (Paste Attributes needs an active app) and
`fcp_library_close`. Each restores the previous frontmost app on success and
failure. Writes to `AXSelectedChildren` may now be an empty list (clearing a
Save panel column). No keyboard or pointer input is synthesized.

## Single-pass writer settings boundary (approved 2026-10-03)

The user approved an SW08 exception confined to
`Sources/ProAppsCore/VideoWriterSettings.swift`: AVAssetWriterInput output settings,
the pixel-buffer adaptor attributes and the composition reader's pixel format are
`[String: Any]` only because AVFoundation requires it. Every dictionary is built
from fixed keys and typed values (`EditEncoding` codec H.264/HEVC, average bit rate,
frame reordering, profile, BT.709 color properties, canvas size, frame rate). No
caller-supplied dictionary or key is accepted.

Compensating checks: tests assert the exact keys/values for both codecs; real
single-pass renders of synthetic fixtures verify frame counts, full decode and
region colors; per-file 95% coverage, TSan and ASan apply. `maxrate`/`bufsize`
(VideoToolbox data-rate limits) are not exposed through these typed keys and are
therefore not enforced. Removal condition: replace when AVFoundation offers typed
writer settings.

## AVFoundation verification boundary

The approved native-interoperability scope also covers AVFoundation's unavoidable
NSDictionary output-settings parameter (SW08): the production decoder supplies
only the fixed BGRA pixel-format key/value. No arbitrary caller dictionary or
unvalidated type-erased values are accepted. The synthetic fixture was generated
with AVAssetWriter/CoreImage and a managed CVPixelBuffer reference; it contains
no user media. MediaProbe itself uses typed framework handles, not handwritten
raw pixel pointers. Decoding runs in a disposable process with Runner's deadline,
so a stalled native decoder cannot indefinitely occupy the MCP process.

No unsafe continuation, unchecked Sendable conformance, raw-memory cast between
unrelated values, unowned object capture, or detached task is authorized by this
scope. Avoidable MIDI/OSC integer encoding has been changed to safe byte shifts.

Compensating checks: per-file 95% line/function coverage; negative argument and
file-collision tests; test-owned MIDI/UDP endpoints; cancellation/deadline tests;
separate Thread Sanitizer and Address Sanitizer runs. Pending checks remain
pending, not waived. See VERIFICATION.md for the current evidence.

Removal condition: replace these boundaries when a supported safe framework API
preserves the same routing, lifetime, non-following and non-overwriting guarantees.
Reassess this scope when the deployment target, SDK or boundary behavior changes.
A missing Swift branch-counter implementation is assessed separately; LLVM region
coverage is never represented as branch coverage.

The user's permission for testing videos does not mean macOS TCC has been granted.
Use only the actual Executor host's reported status; do not edit TCC databases,
change approval policies, overwrite sources or accept unrelated purchases.
