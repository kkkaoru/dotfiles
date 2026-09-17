import MCP
import ProAppsCore

extension ToolSpec {
  static let editSelection = object(
    [
      "startSeconds": number, "durationSeconds": number, "rate": number,
    ], ["startSeconds", "durationSeconds", "rate"])

  static let editAudio = object(
    [
      "volume": number, "fadeInSeconds": number, "fadeOutSeconds": number,
    ], ["volume", "fadeInSeconds", "fadeOutSeconds"])

  static let editGeometry = object(
    [
      "rotation": .object([
        "type": .string("integer"), "minimum": .int(0), "maximum": .int(270),
        "enum": .array([0, 90, 180, 270].map(Value.int)),
      ]),
      "crop": object(
        ["x": number, "y": number, "width": number, "height": number],
        ["x", "y", "width", "height"]),
    ], ["rotation"])

  static let editRecipe = object(
    [
      "clips": array(
        object(
          [
            "sourcePath": string(), "selection": editSelection, "audio": editAudio,
            "geometry": editGeometry, "transitionInSeconds": number,
          ], ["sourcePath", "selection"]), maximum: EditPlan.maximumClips),
      "video": object(
        [
          "width": integer(2, EditPlan.maximumCanvasDimension),
          "height": integer(2, EditPlan.maximumCanvasDimension),
          "frameRate": integer(1, EditPlan.maximumFrameRate),
          "resizeMode": string(["fit", "fill"]),
          "titles": array(
            object(
              ["text": string(maximum: 1024), "x": number, "y": number, "fontSize": number],
              ["text", "x", "y", "fontSize"]), maximum: EditPlan.maximumTitles, minimum: 0),
          "masks": array(
            object(
              [
                "region": object(
                  ["x": number, "y": number, "width": number, "height": number],
                  ["x", "y", "width", "height"]),
                "opacity": number, "blurRadius": number,
                "startSeconds": number, "endSeconds": number,
              ], ["region", "opacity"]), maximum: EditPlan.maximumMasks, minimum: 0),
          "captionStyle": object(
            [
              "outlineWidth": number, "backgroundOpacity": number, "fontSize": number,
              "bottomMargin": number, "centerY": number,
            ],
            ["outlineWidth", "backgroundOpacity"]),
          "captions": array(
            object(
              ["text": string(maximum: 1024), "startSeconds": number, "endSeconds": number],
              ["text", "startSeconds", "endSeconds"]), maximum: EditPlan.maximumCaptions, minimum: 0
          ),
          "color": object(
            ["brightness": number, "contrast": number, "saturation": number],
            ["brightness", "contrast", "saturation"]),
        ], ["width", "height", "frameRate", "resizeMode"]),
      "additionalVideo": array(
        object(
          [
            "sourcePath": string(), "selection": editSelection, "offsetSeconds": number,
            "geometry": editGeometry, "opacity": number,
          ], ["sourcePath", "selection", "offsetSeconds"]),
        maximum: EditPlan.maximumVideoLayers, minimum: 0),
      "additionalAudio": array(
        object(
          [
            "sourcePath": string(), "selection": editSelection, "offsetSeconds": number,
            "audio": editAudio,
          ], ["sourcePath", "selection", "offsetSeconds"]), maximum: EditPlan.maximumAudioLayers,
        minimum: 0),
      "muteOriginalAudio": boolean,
    ], ["clips"])

  static let editProperties: [String: Value] = [
    "recipe": editRecipe, "outputDirectory": string(), "outputName": string(maximum: 180),
  ]
  static let editRequired = ["recipe", "outputDirectory", "outputName"]
  static let editRequest = object(editProperties, editRequired)

  static let editing: [ToolSpec] = [
    .init(
      name: "speech_cut_plan",
      description:
        "Build a cut plan from explicit ordered source-time spans, not automatic speech detection. Pad each span by paddingSeconds (0–2), clamp to source duration (up to 21600 seconds), merge overlaps and interior gaps no larger than minimumRemovedGapSeconds (0–5). Leading/trailing gaps outside padding are removed independently. Returns retained source/output intervals and removed spans, with speechVerified:false. Empty input is refused rather than deleting the entire video. At most 30000 spans. No file access, render, frame quantization or source writes; clients must map captions and quantize cuts before export.",
      properties: [
        "sourceDurationSeconds": number, "paddingSeconds": number,
        "minimumRemovedGapSeconds": number,
        "spans": array(
          object(
            ["startSeconds": number, "endSeconds": number],
            ["startSeconds", "endSeconds"]), maximum: 30_000),
      ],
      required: ["sourceDurationSeconds", "paddingSeconds", "minimumRemovedGapSeconds", "spans"],
      readOnly: true),
    .init(
      name: "media_edit_plan",
      description:
        "Validate an editing recipe and calculate ordered output-time spans. Supports source ranges, concat/reordering, optional per-clip transitionInSeconds (0–5, at most half either adjacent output clip; absent on first), rate 0.25–4, gain/fades, timed additional audio, mute/replacement, canvas fit/fill/crop/quarter-turns. No source loading or rendering; this is shape/timeline validation only. At most 60 clips, 16 audio layers, 16 additional video layers, 600 output seconds. Additional video requires a canvas and fits within the base timeline; later layers appear above earlier ones.",
      properties: ["recipe": editRecipe], required: ["recipe"], readOnly: true),
    .init(
      name: "media_edit",
      description:
        "Render a typed video/audio recipe with native AVFoundation. With video settings, produce MP4; without them, audio-only M4A. Optional video.color clamps input working RGB to SDR 0–1 (not HDR-preserving) and applies brightness -1–1, contrast 0–4 and saturation 0–2 in an additional native encoding pass (omission keeps one pass). Optional video.titles adds up to eight static single-line white bold system titles in top-left output pixels, fontSize 8–128; overflowing text is refused, not truncated. Optional video.captions supplies up to 120 ordered, nonoverlapping output-time cues (text/startSeconds/endSeconds), 120 characters/1024 UTF-8 bytes each, with half-open timing, automatic wrapping and white bold text on a dark bottom box. Optional video.captionStyle controls a black expanded-alpha outlineWidth (0–6 output pixels) and caption box backgroundOpacity (0–1); optional fontSize (16–64 pixels) and bottomMargin (pixels from output bottom) position the text above a source mask. Alternatively centerY sets the vertical center of each caption block in top-left output pixels, independent of wrapping; it cannot be combined with bottomMargin. Text must still fit the canvas. Omission preserves automatic size/position, no outline and 0.7 box opacity. These are supplied captions, not automatic recognition. Canvas must be at least 160x90; oversized text is refused. All text bitmaps share a 16-megapixel budget. Optional video.masks supplies up to eight regions (top-left output pixels, opacity 0–1), applied before new text. Optional blurRadius (1–64 output pixels) blends Gaussian blur instead of black concealment. Optional startSeconds/endSeconds must be supplied together and define a half-open output-time interval; omission means the whole output. Full-opacity black masks conceal burned-in subtitles; blur and partial opacity do not guarantee unreadability, and neither reconstructs the hidden scene. Masks, titles, captions and color share one extra encoding pass. Optional clip.transitionInSeconds overlaps adjacent clips with video cross-dissolve and linear audio crossfade; it shortens the timeline. Transition and explicit audio fades use the longer duration, rejecting overlap within a clip. Optional additionalVideo supplies up to 16 output-timed video-only layers above base clips: sourcePath, selection, offsetSeconds, optional geometry and opacity (0–1, default 1). Later entries are on top. Source alpha is honored; opaque sources cover the base. Layer audio is ignored. Global effects/masks/titles run after video composition. Repeated placements can reuse an animation clip; all intervals must fit the base timeline. Gain is linear 0–1; fades and audio offsets use output seconds. Preserves every source; creates a private edit-UUID subdirectory and reusable edit-request.json. Processing has a 300-second child deadline; failure can leave partial staging but never replaces existing media. No playback or proprietary editor timeline mutation. Use an absolute outputDirectory under ~/Movies/Apple-Pro-Apps-Verification for this user's tests.",
      properties: editProperties, required: editRequired, readOnly: false),
    .init(
      name: "media_project_read",
      description:
        "Read a saved bounded edit-request.json, enforcing the editing schema and timeline limits. Returns the reusable request and plan for revision/re-rendering; does not resolve missing sources or render.",
      properties: ["path": string()], required: ["path"], readOnly: true),
  ]
}
