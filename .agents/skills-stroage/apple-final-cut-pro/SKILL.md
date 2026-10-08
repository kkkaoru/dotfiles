---
name: apple-final-cut-pro
description: Integrate Final Cut Pro through Executor using FCPXML inspection, generation, copy/patch and native Open Document import first, plus typed live timeline tools (`fcp_*`) for reading, selecting, frame-exact playhead seeking, blade/delete, inspector parameters, effects and keyframes (carrier paste), FCPXML export and Share export. Covers limits of live editing/export; native background `ui_*` tools before an auxiliary Peekaboo fallback.
---

# Final Cut Pro — FCPXML first

Load **apple-pro-apps**. Discover the actual `apple-pro-apps` tool schemas before
calling them. This Mac uses `finalCutPro` / `com.apple.FinalCutApp` (Creator Studio).
Never target the standalone edition implicitly or edit a live `.fcpbundle` database.

## Mechanical timeline/media workflow

1. Obtain a user-approved FCPXML export or build FCPXML following Apple's current
   reference. `interchange_inspect` and `interchange_query` with `kind: "fcpxml"`
   inspect structure, assets, resource IDs, timelines and metadata without the UI.
   Queries return bounded/truncated fragments; narrow the XPath rather than
   assuming a truncated node is the full XML.
2. `interchange_write` creates a **new** `.fcpxml`; `interchange_patch` creates a
   copy while changing existing uniquely selected leaf/attribute values. Use the
   former for structural edits, the latter for precise values. No overwrite,
   arbitrary entity declarations or external DTD loading is allowed.
3. Keep resource references consistent, media URLs correctly escaped, lanes and
   parent-relative offsets intentional, and time values in FCPXML rational seconds
   (for example, `1001/30000s`). Do not substitute decimal frame approximations.
   Match source media formats and the intended sequence rate/resolution/color space.
4. `interchange_inspect` checks UTF-8, well-formed XML and the `fcpxml` root only.
   Separately discover/describe `fcpxml_validate`: it uses the matching DTD from
   the selected installed Final Cut edition, private snapshots and network/catalog-
   disabled system validation. External schema references are refused. DTD success
   **does not resolve media, prove effect fidelity or establish importability**.
   Use version-matched Apple documentation and an isolated library for import tests.
5. Deliver the saved file with `app_open_document`, `app: "finalCutPro"`. This uses
   the documented Open Document path, not UI clicking. FCP may ask for an event,
   library, missing media or license; tool acceptance does not verify the timeline.
   Don't repeat an indeterminate import—it may create duplicate events/projects.
6. Final Cut Pro shows a library chooser for **every** FCPXML Open Document, even
   when `<library location>` names an existing library; a missing library is not
   created. Drive the chooser in the background: `ui_wait` for the chooser window,
   `ui_set_value` `selected: true` on the `AXRow` whose `containsText` is the exact
   intended `.fcpbundle` name, `ui_perform` `press` on the confirm button (read its
   localized title), then `ui_wait` for the window to be absent. Never select a
   library you were not asked to use; Final Cut Pro may also reopen previously used
   libraries at launch — do not act on them.
7. Creating a brand-new library goes through a Save panel whose folder change needs
   the keyboard-only Go to Folder sheet. Prefer an existing, explicitly approved
   verification library. Final Cut Pro's scripting dictionary is read-only (`get`);
   it cannot create libraries or import. No native import-verification tool exists
   yet; confirm the imported event/project by observation (for example `ui_inspect`
   of the browser) and report that as observation, not database proof.
8. `interchange_patch` output currently omits `<!DOCTYPE fcpxml>`, which makes
   `fcpxml_validate` fail. Write FCPXML that must validate with `interchange_write`.

Current exports can also be `.fcpxmld` bundles containing `Info.fcpxml`. The native
open tool accepts that bundle, but XML tools operate on its explicit regular
`Info.fcpxml` file. Write a new standalone `.fcpxml` for experiments; never mutate
a live bundle or assume every bundled asset can be omitted.

## Automatic renders still export FCPXML

Native `media_edit` / `media_edit_batch` video renders write `<outputName>.fcpxml`
next to each output and DTD-validate it. Report `render.fcpxml.unrepresented`
(for example masks recorded only as markers, the caption outer rim) and import it
with the library-chooser procedure above when review in Final Cut Pro is needed.
Title positions use FCPXML transform percentages and are approximate; the
rendered MP4 is the delivered look.

## Offline edits before import

For a new rendered asset rather than a live FCP timeline change, use
`media_edit_plan` → `media_edit` → `media_project_read`. Recipes support trim,
reorder, speed, crop/fit/fill/rotation, audio mixing/fades, SDR color controls and
static titles (describe the deployed schema). This produces MP4/M4A plus reusable
JSON, not an FCPXML timeline or an imported FCP project. Preserve the source and
use the Movies verification root for viewing tests. Check `media_verify_video`,
`audio_measure` and `video_frame_measure` for bounded decoded evidence; none
proves FCP import or all-channel/HDR fidelity.

## Live timeline control (`fcp_*` tools)

**Safety first.** Always pass `project` to timeline tools (required for
`fcp_timeline_select`, `fcp_timeline_edit`, `fcp_inspector_set`); they refuse when
the timeline shows another project. Another session or the user may switch the
timeline at any time — never chain mutations after a failed call, and re-read
before every mutation. `fcp_project_open` briefly activates Final Cut Pro and
navigates back if Open Clip opened a timeline clip instead.

The native MCP drives the project open in the timeline through Accessibility.
Final Cut Pro autosaves and `編集 > 取り消し` is unavailable from the background,
so experiment only in a disposable library/project. Everything except export
keeps Final Cut Pro in the background (`frontmostChanged: false`).

- `fcp_timeline_read`: project name/duration, playhead, and a page of clips
  (`index`, `kind:name` description, start, duration, selected). Read first.
- `fcp_project_open {library?, event, project}`: selects the event row in the
  library sidebar and the filmstrip item, focuses the browser and runs
  クリップ > クリップを開く; waits until the timeline shows the project. Name the
  library when event names repeat. The browser must be in filmstrip view.
- `fcp_timeline_select {clips: [{index, description}]}`: writes the timeline's
  `AXSelectedChildren`; each description must still match (stale reads fail).
- `fcp_playhead_move {move, count}` and `fcp_playhead_seek {timecode}`: Mark menu
  moves read back after each step. Seek hops edit points that do not pass the
  target, then single frames (about 1 ms per frame measured), so any frame is
  reachable without keyboard input; it fails with the reached position at the
  timeline end or the `maximumSteps` budget.
- `fcp_timeline_edit {command}`: `bladeAll` (blade every lane at the playhead —
  frame exact), `delete` (selected clips, ripples the primary storyline),
  `deselectAll`, `setClipRange`. `changed` reports clip count/duration change.
  トリム > ブレード and トリム開始点/終了点 stay disabled while FCP is inactive.
- `fcp_export {project, directory, fileName, format?, codec?, allowForeground: true}`:
  Final Cut Pro enables 共有 only while active, so this is the one tool that
  **activates** it (user-approved scope 2026-10-08). It focuses the timeline,
  opens ファイル > 共有 > ファイルを書き出す（デフォルト）…, checks the dialog names
  `project`, sets format/codec and 操作 = 保存のみ, drives the Save panel (sidebar
  home → column browser, localized system folder names such as ムービー), names the
  file and saves, then restores the previous frontmost app. On any failure it
  cancels the panel/dialog and restores focus. The destination must be an
  existing visible folder inside the home folder and the file must not exist.
  Return is dispatch evidence (`completionVerified: false`): wait for the file to
  stop growing, then `media_inspect`, a full `media_verify_video` decode and an
  audio decode before calling the export finished.

### Effects, parameters and keyframes

- `fcp_effect_catalog {query?}`: installed Motion effect templates with FCPXML
  `uid`, file name, Japanese name and category (for example ガウス =
  `.../Effects.localized/Blur.localized/Gaussian.localized/Gaussian.moef`).
- `fcp_inspector_read {tab?}` / `fcp_inspector_set {parameter, value | enabled, tab?}`:
  background read/write of the selected clip's inspector (value fields such as
  不透明度, 位置 x, 回転, effect parameters like `horizontal`, and enable
  checkboxes such as ガウス). Values are confirmed and read back numerically.
  Values reflect the playhead; set a constant only on parameters without keyframes.
- `fcp_effects_paste {library, event, project, targets, workDirectory,
  durationSeconds, frameDuration?, opacity?, position?, scale?, rotation?, anchor?,
  effects?}`: each attribute is `{value}` or `{keyframes: [{seconds, value,
  curve?}]}` (opacity 0–1; position/anchor "x y" in percent of frame height;
  scale "x y"; rotation degrees; curve linear by default). Effects are
  `{uid, name, parameters: [{name, key, value | keyframes}]}`. It imports a
  carrier into `Claude-Effect-Carriers.fcpbundle` inside `workDirectory` (no
  activation), copies it — **overwriting the clipboard** (user-approved) — and
  pastes onto the targets. `mode: merge` (default) uses Paste Attributes with only
  the carrier's attributes checked and timing Maintain, keeping existing effects
  and keyframes (verified); `mode: replace` uses Paste Effects. The carrier library
  is closed afterwards. Keyframes stay relative to each target clip;
  durationSeconds should cover the longest target.
  **Paste Effects replaces** the targets' existing effects and animated
  attributes (verified: earlier opacity keyframes were removed). Put everything
  the clip should keep into the same carrier, or read the clip first with
  `fcp_xml_export` and reproduce it.
- Effect parameter **keys** (`9999/...`): use `fcp_effect_parameters {uid}`; keys
  are derived from the template hierarchy and `verified` marks the 22 structures
  confirmed against Final Cut Pro's XML (1731 of 1736 parameters). For FxPlug
  filters (`FxPlug:<UUID>` in the catalog) or unverified keys, set the parameter
  once with `fcp_inspector_set`, run `fcp_xml_export` and read the key.
- `fcp_library_close {library}` closes a library by exact name (never another one).
- `fcp_xml_export {project, directory, fileName (.fcpxmld/.fcpxml), allowForeground: true}`:
  Final Cut Pro's own FCPXML (activates like export). Use it to verify keyframes.
- `fcp_timeline_edit` adds `addColorAdjustments`, `addColorBoard`,
  `addCrossDissolve`, `removeEffects` (background menu commands).
- Inspector keyframe buttons/parameter menus ignore Accessibility presses even
  when FCP is active, and browser effects need a double-click; use the carrier
  route instead. Audio effects (Audio Units, Logic, third-party) are not carried. Opacity midpoints render brighter than 50% because Final Cut Pro
  blends in linear light — measure against that, not a gamma-space midpoint.

Coarse trims via clip-edge `AXPosition` writes are pixel based (≈1.3 frames per
pixel with hysteresis) and are deliberately not exposed; use seek + `bladeAll`
+ select + `delete` for exact cuts. Japanese menu titles are verified; English
titles were also exercised (FCP relaunched with `-AppleLanguages (en)`).

## Remaining export and live-operation limits

FCPXML interchange and the `fcp_*` tools are not a universal live timeline API.
Apple documents Custom Share Destinations (media/FCPXML delivery by Apple events),
Workflow Extensions (in-app timeline integration, including `movePlayhead`) and
FxPlug (effects). These require additional app/extension implementations and are
**not** provided by this MCP. The installed scripting dictionary is read-only
library inspection. Effects, color, keyframes, trimming by drag and other
destinations still need the generic `ui_*` tools or a human.

For other UI-only work (browser organization, effects/color, keyframe editing,
other share destinations), first try the native background `ui_*` tools (menus, buttons,
rows, text values). Use `apple-pro-apps-ui` (Peekaboo) only for an explained
remaining gap via the shared safe procedure. Export to a new destination and verify actual media. Use
Compressor's native MCP for subsequent encoding, not blind repeated Share actions.

## Edit repeated text objects within one title

For templates with separate face/rim text objects, changing one text field does
not imply the other follows. Prefer Apple's **Edit > Find and Replace Title
Text** over repeatedly trying an unverified layer-selector click:

1. Verify the intended title selection. Open the dialog once; a floating panel
   may be hidden while FCP is inactive. Observe before any redispatch, and use
   authorized foreground focus only if necessary to expose it.
2. Set **Search in: Selected Title** and read it back before replacing anything.
   The default can be **All Titles In Project**; never leave that scope for a
   single-title edit. Read back the exact find and replacement strings.
3. Use **Replace All** within that selected title. This replaces matching text
   in its separate objects; it does not establish a persistent text binding.
4. Verify both objects in FCP's own newly exported XML, plus unchanged other
   titles, cuts, clocks and keyframes. For a reversible test, inverse-replace
   within the same verified scope and compare the restored sequence, then close
   dialogs and restore focus. Never overwrite earlier evidence exports.

Official procedure: https://support.apple.com/guide/final-cut-pro/find-and-replace-text-verc5d34470/mac

## Recovering a stuck instance without logout

A huge import can leave FCP in state `E` (exiting, no threads) for a long time.
LaunchServices then still lists it, `app_launch` returns the dead pid, and
`open` fails with -600. Do not log out or reboot:

1. Stop the import first with a normal quit; if it is refused (-128), check the
   open libraries (`lsof -p PID | grep -o '/[^ ]*\.fcpbundle'`), then `kill -TERM`.
2. `lsappinfo list` shows AppKit XPC services labelled `（Final Cut Pro）`
   (Open and Save Panel, ThemeWidgetControlViewService). They serve only the dead
   instance; stop them (`kill -TERM`, then `-KILL`).
3. Start the binary directly so LaunchServices is bypassed:
   `nohup "/Applications/Final Cut Pro Creator Studio.app/Contents/MacOS/Final Cut Pro" >LOG 2>&1 &`.
   The log reports each library restore; confirm with `ui_windows` that the new
   pid responds and focus did not change.

Avoid the cause: a timeline with tens of thousands of titles or clips. Use one
title per caption (custom template) and masked filters on the original clips.

## Apple references

- https://developer.apple.com/documentation/professional-video-applications/importing-fcpxml-data
- https://developer.apple.com/documentation/professional-video-applications/sending-data-programmatically-to-final-cut-pro
- https://developer.apple.com/documentation/professional-video-applications/receiving-media-and-data-through-a-custom-share-destination
- https://developer.apple.com/documentation/professional-video-applications/workflow-extensions
- https://support.apple.com/guide/final-cut-pro/welcome/mac
