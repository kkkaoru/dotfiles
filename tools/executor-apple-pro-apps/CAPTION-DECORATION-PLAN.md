# Caption decoration, source-subtitle concealment and 60/90-second verification

## Required deliverables

Produce both 60-second and 90-second variants from the existing recording, without
replacing source media or earlier outputs. Neither length alone completes acceptance.
Add white text with a black outline, a restrained sound at each caption onset,
and explicit source-subtitle concealment. Preserve unreviewed ASR and evaluation
artifacts separately from any supported corrections.

## Implementation sequence and contracts

1. Add bounded output-pixel black masks with explicit opacity to video recipes.
   Masks are applied for the whole output, after color adjustment but before new
   titles/captions. Full opacity conceals pixels; partial opacity leaves them
   visible. Neither reconstructs the underlying scene. Validate count, finite
   geometry, canvas containment and opacity before export; retain old recipes.
2. Implement a real expanded-alpha black text outline, not a claim that a drop
   shadow is an outline. Keep text layout bounded and preserve the shared overlay
   pixel budget. Add raster and native composition regressions.
3. Inspect source frames and text locations using local native decoding/OCR as
   needed, with private bounded still outputs. Choose mask regions from actual
   evidence, not guessed coordinates. No cloud video/audio upload.
4. Produce bounded local SE assets and place them through existing timed audio
   mixing. Avoid an audio track per cue if that exceeds the 16-layer budget: use
   a precomposed cue track. Verify onset alignment, off-onset silence and source
   voice preservation rather than merely detecting any audio.
5. Transcribe 60/90-second material in bounded <=60-second audio chunks. Retain
   source offsets and inspect boundary duplicates/omissions. Evaluate using local
   audio recognition evidence (and source-caption OCR as corroboration); agreement
   between recognizers or burned-in captions is not ground truth. Report ambiguity,
   unreviewed names and unsupported corrections; never invent an accuracy score.
6. Run mandatory 60/90-second fixtures and real exports: full 1800/2700 frames at
   30fps, whole audio, caption/mask/outline samples at early/middle/late times and
   boundaries, SE onset/control windows, source hashes, transcript and cue records.

## Ownership and gates

Keep AVFoundation/Core Image and file I/O on bounded operation-owned executors;
only SwiftUI rasterization uses MainActor. Do not add unsafe interoperability,
unstructured detached tasks, dependencies, model downloads or permission bypasses.
Existing Japanese reservation approval applies; any new model installation needs
separate consent. Mutating setup remains distinct from read-only transcription.

For each logical change run focused tests, then full strict formatting, warnings-as-
errors, per-production-file >=95% line/function gates, separate sanitizers and
restored normal coverage. Discover/describe deployed Executor tools before real
calls. Reuse no completion claims from the previous 14-second subtitle video.

## Status

Masks, expanded-alpha outlines, bounded single-track SE synthesis and local Vision
OCR have passed 170 tests, per-file gates and full sanitizers and are deployed.
Real source OCR found the burned-in subtitle area separately from upper UI text.

The follow-up release adds caption size/vertical placement and clamps OCR boxes
to their actual image bounds. A real 30-second ASR result exposed an approximately
18-microsecond native-clock/container-end discrepancy: only <=1 native clock tick
is now normalized, preserving the original duration and unchanged text. Strict
transcript validation still rejects the unnormalized or larger overrun. The private
failing-input reproduction and subsequent Executor transcription both passed.

Combined 60/90-second synthetic video tests now verify exact 1800/2700 frames,
positioned outlined captions over masks, source voice and cue-onset audio, control
windows and source preservation. All 179 tests / 32 suites, 31 per-file gates and
full separate sanitizers passed, with normal instrumentation restored.

Both real decorated outputs were rendered at 720×1280/30fps and fully decoded:
60s/1800 frames and 90s/2700 frames, with 960,000/1,440,000 mono samples. All 5/8
caption midpoints were sampled by local OCR; selected adjacent boundary frames,
SE onset/off-onset windows, black mask regions, source/control comparisons and
artifact/source hashes were checked. Both videos, recipes, SRT, raw ASR and
contextual-review results are indexed under the private Movies verification home.
Evaluation explicitly leaves uncertain names/phrases uncorrected and makes no
speech-accuracy percentage claim. See CAPTION-DECORATION-VERIFICATION.md for scope.
The final unchanged-limit source/schema consolidation audit also passed all 179
tests, both full sanitizers, restored coverage, release refresh and final hashes.
