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
