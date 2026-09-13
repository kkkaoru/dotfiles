# Editing expansion — delivered native slice and boundaries

The requested outcome is useful video/audio editing through Executor MCP, not a
larger catalog of wrappers. Existing source inspection and Compressor submission
are insufficient. No universal control of every proprietary editor is claimed.

## Delivered scope

The native slice now covers steps 1–3 and the bounded step-4 features: SDR color
controls, static white text overlays, video/audio crossfades and a DTD-validated
four-clip FCPXML example. FCPXML effects are baked into referenced media, not
parameter-preserving native FCP effects. Step 5 includes the real-media matrix,
advanced-effect examples and updated six Skills. See VERIFICATION.md for exact
counts, commands, decoded evidence and remaining limitations.

The latest Executor UI check still lacks Accessibility/Event Synthesizing.
Proprietary-app GUI operation and isolated import confirmation require the user's
consent/workflow; they are not claimed complete. Animated titles, full subtitle
support, arbitrary filters/plugins, HDR mastering and universal editor control
are not provided by this bounded slice.

## Delivery sequence

1. Typed edit recipes and deterministic timeline planning: ordered source ranges,
   concatenation/reordering, per-clip speed, video/audio-only output, original
   audio muting, clip volume and fades, additional timed audio tracks.
2. Native AVFoundation rendering: source validation, crop/fit/fill/quarter-turn
   transforms, explicit canvas/requested frame cadence, audio mixing and time scaling. Preserve
   every source and publish only new outputs atomically. Use the existing bounded
   child-process boundary; do not install a daemon or shell-based rendering layer.
3. Evidence-producing verification: deterministic colored video and tone fixtures,
   full short-output frame decoding, selected-frame image measurements and decoded
   audio measurements. Prefer managed CoreImage/CoreGraphics APIs and the system
   afconvert utility over adding unsafe memory exceptions. Tests must verify actual
   effects, timing, boundaries, cancellation, failed exports and collision refusal.
4. Titles/overlays and color effects using typed CoreImage APIs, then transitions
   and editable timeline interchange. Validate FCPXML against the installed Apple
   DTD before claiming structural validity; actual isolated-app import is separate.
5. Expand the matrix using the user-approved source video through discovered and
   described Executor tools. Never substitute CLI exit zero, metadata or one frame
   for a verification that was not performed. Update all six Skills and the
   verification report to the exact capabilities and limits delivered.

## Contracts and ownership

- Recipes are bounded Codable request DTOs, validated before execution. Explicit
  defaults are documented; invalid/missing required data must not be discarded.
- Each render owns its composition, asset references, output staging and cleanup.
  Mutable AVFoundation objects do not cross isolation boundaries. Blocking file
  operations use the existing dedicated execution boundary; native async APIs
  handle loading/export. Cancellation must stop and join owned work.
- No arbitrary command flags, external URLs, project overwrites, live concert
  changes, audio playback, new approval policy, or TCC modification.
- Viewing outputs belong beneath `~/Movies/Apple-Pro-Apps-Verification/`, resolved
  to an absolute path. Diagnostic data remains private and outside Git.
- New native memory/type-erasure exceptions are not assumed from this plan. Seek
  explicit narrow approval if a safe supported approach cannot satisfy a feature.

## Verification gate

For each logical change: focused tests, strict formatter/linter, warnings-as-errors
build/tests, at least 95% lines and functions for each production source file, and
separate Thread/Address Sanitizer runs where required. No coverage exclusions for
handwritten implementation. Branch-counter limitations remain separately stated.

The first slice must demonstrate real trim/concat/speed/geometry/audio behavior,
not merely successful serialization of requested options. Keep synthetic native
integration tests separate from pure model tests and from real-user-media checks.
GUI permission blockers do not block independent native implementation work.
