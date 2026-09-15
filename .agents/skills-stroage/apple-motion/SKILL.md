---
name: apple-motion
description: Automate Apple Motion through Executor with native project-file inspection, XPath queries and non-overwriting copy/patch operations first; use Peekaboo only for UI-only authoring/rendering. Covers motion graphics, keyframes, rigs and Final Cut templates.
---

# Motion — native file integration first

Load **apple-pro-apps** first. Discover native tools in `apple-pro-apps`; the
separate `apple-pro-apps-ui` namespace is fallback, not the primary method.
Use `app_capabilities` and verify `motion` / `com.apple.motionappApp` on this Mac.

## Machine workflow

1. Start from a version-compatible, known-good `.motn`, `.moti`, `.motr`, `.moef`
   or `.mogen` document. Do not guess the internal node/class layout.
2. Use `interchange_inspect` with `kind: "motion"`, then `interchange_query` to
   inspect bounded fragments at specific XPath locations. The expected root is
   `ozml`; well-formedness does **not** establish that Motion can open/render it.
3. For a parameter/text/keyframe value already represented in that document, use
   `interchange_patch` with an explicit new output path and a uniquely selecting
   XPath. It only changes existing attributes or text-only leaf elements. Inspect
   identifiers/units/time representation before choosing values. Motion writes
   require `allowUndocumentedFormat: true` because this format is undocumented.
4. `interchange_write` can create a new file from fully authored XML with the same
   opt-in. It is not a supported schema generator: prefer a known-good template
   over invented project XML. Originals are never overwritten.
5. Use `app_open_document` with `app: "motion"` and the new path for native
   Open Document delivery. Opening is not render validation. Inspect the result
   with the user or the explicitly chosen UI fallback before production use.

## Native rendered-media alternative

If the goal is a new MP4 rather than editing a `.motn`, discover `media_edit`:
typed recipes offer geometry, SDR brightness/contrast/saturation and static
single-line white titles. Color/title effects share an additional native encoding
pass; no Motion project, rig, FxPlug or reusable Motion title template is created.
Use `media_project_read` for the reusable JSON recipe, and full-video/region/audio
measurement tools for bounded export evidence. These do not establish Motion
compatibility, animated text support or HDR preservation.

## Where GUI fallback is still needed

Apple's guide documents project creation, layers, filters, behaviors, tracking,
3D/particles, rigs, template publishing and export through the application UI;
no supported universal parameter API/headless `.motn` renderer was established.
For these gaps, explain the limitation and use the shared Peekaboo procedure.

- New projects: confirm dimensions, frame rate, duration and color space; choose
  Motion Project versus Final Cut Effect/Title/Transition/Generator deliberately.
- Layers/animation: confirm exact layer, playhead and active tool; distinguish
  static changes, keyframes and animation recording. Verify at multiple times.
- Templates: publish selected Inspector parameter controls or rig widgets;
  review Project Inspector → Publishing, save under a new name/category, then
  verify in the corresponding Final Cut browser. Not all items can be published.
- Rendering: inspect Share options, range, codec/alpha and destination; verify the
  exported media. Compressor can encode exported media through its native MCP;
  its CLI is not evidence that it can render arbitrary Motion projects headlessly.

Do not directly alter template bundles shipped by Apple, overwrite user templates,
change custom command sets or assume factory shortcuts. Treat media/fonts/plugins
referenced by XML as part of the version-sensitive dependency set.

## Apple references

- https://support.apple.com/guide/motion/welcome/mac
- https://support.apple.com/guide/motion/motn17691fe6/mac — template types
- https://support.apple.com/guide/motion/motna47583a5/mac — publish controls
- https://support.apple.com/guide/motion/motn13f21017/mac — publish rigs
- https://support.apple.com/guide/motion/motn72925de5/mac — convert project types
