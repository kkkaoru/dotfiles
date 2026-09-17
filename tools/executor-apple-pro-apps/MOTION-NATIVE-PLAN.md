# Motion native animation and approved sound-pack work

## Current priority: UI-free Motion expansion (2026-09-16)

The user explicitly stopped the UI-heavy approach, requested expanding the skill
and MCP, and then authorized all proposed work autonomously, including the
experimental Motion-XML method that was described immediately beforehand.
This permits edits to NEW disposable copies, not overwrites of originals,
installed templates, unrelated projects or blanket changes to approval policies.
Do not continue Project Browser clicking. No further GUI actions are queued.

The canonical app skills were found under `.agents/skills-stroage/`; the formerly
advertised `~/.agents/skills/apple-motion/SKILL.md` does not currently exist.
Update the canonical skill and restore narrowly scoped discoverability rather
than treating a missing startup path as evidence that no native workflow exists.
The older paragraphs below are historical and include superseded blockers.

Native implementation sequence:

1. Add a bounded Motion-specific project inventory: document version, scene-node
   IDs/names, text layers, parameter paths, existing animation curves and local
   dependencies. Use actual installed-template XML, not guessed node factories.
2. Add typed, source-fingerprint-bound copy editing for existing text/parameters
   and keyframes. Reject missing/ambiguous IDs, stale source hashes, invalid times,
   conflicting changes and unsupported structures. Preserve originals and use
   private non-overwriting publication. Never pretend XML validity proves a render.
3. Use the existing official Compressor MCP for actual Motion-to-video rendering;
   previous unchanged Snap Lower Third -> H.264 success is evidence of a viable
   narrow path, not a universal renderer. Investigate the failed alpha preset
   separately. Do not substitute AVFoundation compositing and label it Motion.
4. Verify edited Japanese text and changing animation with actual decoded output,
   then create the approximately one-minute subtitle study and multiple distinct
   VTuber experiments. Keep editable copies and explicit render evidence.
5. Update skill/capability descriptions with tested recipes, source/version
   requirements and honest unsupported features. Preserve all existing gates.

Fresh Executor read-only evidence is in the authorized media workspace revision
`motion-native.P4O4qw`: installed Snap Lower Third uses ozml version 4.0,
`scenenode` text layers with IDs 10040/10092, direct `text` children, nested
`parameter` elements with name/id/value attributes, style timing attributes and
`curve` elements. These observations are not sufficient to invent new nodes or
change text-run ranges without inspecting them.

The preceding Whisper/fixture verification must be settled before concurrent
Swift source changes. Job 193 passed normal per-file coverage and TSan but failed
ASan because the runtime-generated SOURCE fixture had 29 rather than 30 frames.
No assertion was weakened. A stored synthetic 30-frame fixture replaces only
that setup; the first lossless H.264 encoding was independently decoded by
FFmpeg but unsupported by Apple's reader and is retained as failure evidence.
Job 204 tests a High-profile compatible fixture under ASan. Full gates, normal
coverage restoration and release remain mandatory before deployment.

## Requested outcome

The user explicitly requested completion of Logic sound-pack downloads, UI-free
animation editing of TikTok footage with Motion, and development of missing
capabilities. Preserve source footage, installed templates, previous outputs and
unrelated working-tree changes. Keep private paths, captures and hashes out of Git.

## Machine-first policy

The user explicitly requires continually expanding non-UI capabilities. Prefer
public APIs, typed CLI calls and validated file interchange. Treat repeated UI
work as a candidate for a bounded native adapter, not as the permanent design.
A UI fallback must name its missing machine interface and retain honest limits;
never relabel UI automation or an AVFoundation-only effect as native Motion work.

## Work sequence / acceptance

1. Complete the currently offered Logic sound-pack download, using the existing
   Executor UI integration. Download is explicitly approved; purchases, license
   acceptance, TCC changes and new blanket approval policies are not. Verify the
   actual installed/completed state, not merely a clicked button. Never repeat an
   unconfirmed mutation without observing the current state.
2. Establish a version-matched Motion template and inspect its native project
   structure read-only. Separate template copying/editing from rendering. Motion
   XML is undocumented; keep the experimental-format boundary explicit and never
   modify installed templates or existing user projects.
3. Investigate documented Motion/Compressor interoperability and the official
   Compressor CLI. Apple documents adding Motion project files to Compressor, but
   that alone does not prove that CLI submission can render them unattended. Test
   the actual path before claiming headless support. Do not substitute old Final
   Cut Pro 7 Apple events or call AVFoundation rendering “Motion-made.”
4. Implement missing bounded tools only after the concrete template/renderer
   contract is understood. Keep typed inputs, non-overwriting publication, exact
   source preservation, cancellation/deadlines and explicit compatibility limits.
5. Verify real TikTok footage with both 60s and 90s outputs, including animation
   changes at multiple times and full 1800/2700-frame decode at 30fps. Verify audio
   separately. Do not represent XML well-formedness or job submission as rendering
   success. Apply full Swift rules and per-file coverage/sanitizer gates for code.
6. The supplied 39-second BGM reference is already copied and decoded to stereo
   Float32 WAV in the authorized project downloads directory. Reference alignment,
   separation and removal are still pending and are not supplied by Stem Splitter's
   interface alone. Preserve all intermediate/reference evidence.

## Current observations

- Executor-hosted Screen Recording, Accessibility and Event Synthesizing all pass.
- Motion and Logic launch and expose menus. Motion's project browser exposes
  actionable AX controls after increasing the bounded observation depth/count.
- After one normal restart, Logic's library exposes a complete 231-element AX
  snapshot. The three initial starter packs (Studio Instruments, Electronic and
  Hip-Hop) explicitly show installed. This does not mean the entire optional
  catalog is installed; available local space is approximately 21 GiB.
- Motion's exact empty `<!DOCTYPE ozxmlscene>` prolog is supported. The corresponding
  release passed 193 tests / 35 suites, per-file gates and separate full sanitizers.
  External/internal DTDs, entities, duplicate/misplaced and cross-format declarations
  remain rejected. Later source changes require new release gates.
- An unchanged copy of the installed Snap Lower Third project rendered through
  Compressor CLI without UI interaction. The successful output fully decodes:
  180 frames, 1920×1080, 29.97003 fps, 6.006 seconds, no audio. Sampled early frames
  are black; later samples contain graphics and OCR reads NAME HERE / Description.
  Output hashes match before/after inspection. This proves one unchanged template,
  not edited parameters, transparent export, TikTok composition or general support.
- The initial installed-path submission failed because spaces were URL-encoded.
  A filesystem-path argument fix passed nine focused tests after a compiler
  type-checking failure was resolved. Its full release/deployment remains pending.
- The restart/complete library snapshot supersedes the earlier incomplete/blank
  Logic observations. Never retry the earlier unconfirmed click on stale evidence.

## Additional-video implementation contract (code gates passed; real acceptance pending)

Add optional `additionalVideo` to the existing typed edit recipe, not a new runtime.
Each video-only placement contains a local source, explicit selection/rate, output
start, optional geometry and opacity (default one). At most 16 placements fit
inside the base timeline; later entries appear above earlier entries. This supports
repeated placements of a short Motion-rendered animation. Source alpha participates
in compositing, opaque video covers the base, and layer audio is ignored. Existing
explicit audio layers and base audio remain separate. Global effects/masks/text run
after composition; use a previously processed base when masks must remain beneath
an animation. Existing recipes remain compatible through an absent optional field.

Reuse the operation-owned AVFoundation graph and instruction partitioning. Do not
share mutable assets/compositions across operations or add executors, raw pointers,
unstructured tasks or new unsafe exceptions. Preserve the existing child deadline,
private staging, cancellation checks, saved recipe and non-overwriting publication.

Verification: model/schema rejection cases, round-trip compatibility, native pixel
checks for layer order/opacity/geometry/half-open timing, separate audio decoding,
source preservation and failure cleanup. Then test actual ProRes alpha export from
Motion and both real 60/90-second compositions. An isolated source snapshot passed
230 tests / 46 suites, full separate TSan/ASan, restored coverage for all 37 production
files (minimum 95.238% lines / 96.774% functions), and a release build. Source hashes
matched the original checkout afterward. The managed synthetic ProRes alpha test
also passed; that is not proof of Motion alpha export.

The first Motion-to-ProRes trial with Apple's `ap4h.setting` failed with
`className is null`; no output was produced. Retain the failed job and do not
resubmit it blindly. The earlier Motion-to-H.264 output remains fully verified,
but opaque. An initial scratch-path-only coverage attempt failed because CLI tests
explicitly invoke the package-local `.build/debug` binary; copying the complete
package isolated those subprocesses and restored correct coverage collection.
Dependency checkout emitted two cache-lock warnings on the first isolated run;
subsequent builds and all four test runs completed without compiler diagnostics. The
combined workflow must be described as Motion-generated animation composited by
AVFoundation, not as an entire TikTok timeline rendered inside Motion.

## Deployed diagnostic composition evidence

The canonical release was built after source-hash comparison, the existing Executor
connection refreshed, and its described schema confirmed `additionalVideo`. Both
real diagnostic composites are 720×1280 at 30fps: exactly 60/90 seconds, fully decoded
1800/2700 frames and 960,000/1,440,000 mono 16kHz audio samples. Both audio peaks are
0.964081. Inputs and probed outputs retained matching hashes.

The unchanged Motion H.264 clip is cropped to its lower-third band, retimed from
6.006 to 6 seconds and placed 10/15 times. It is deliberately opaque, not an alpha
substitute. Samples at 0.5/6.5 seconds are black; samples at 3/9 and 57/87 seconds
show graphics plus OCR NAME HERE / Description. Four control samples outside the
band match the corresponding base video within approximately 1/255 mean RGB.
These are selected-region checks, not all-pixel or perceptual audio proof. Private
viewing indexes record exact outputs, recipes, measurement results and hashes.

Remaining: explicit user authorization for experimental Motion copy editing,
verified edited text/animation parameters, working Motion alpha export, finished
60/90-second animation deliverables, optional sound-pack completion and reference
BGM removal. Tmux completion notifications are not that authorization.

## Apple references

- https://support.apple.com/guide/motion/motn189cf6cf/mac
- https://support.apple.com/guide/compressor/cpsr1e359452/mac
- https://support.apple.com/guide/compressor/cpsr9be73312/mac
