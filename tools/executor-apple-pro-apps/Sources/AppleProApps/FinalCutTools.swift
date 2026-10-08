import Foundation
import MCP
import ProAppsCore

extension ToolSpec {
  static let finalCutTarget: [String: Value] = ["bundleID": string()]

  static let keyframeList = array(
    object(
      [
        "seconds": number, "value": string(maximum: 512),
        "curve": string(FCPKeyframeCurve.allCases.map(\.rawValue)),
      ], ["seconds", "value"]), maximum: 1000)

  /// A constant `value` or a `keyframes` list.
  static func animated(_ keyframes: Value) -> Value {
    object(["value": string(maximum: 512), "keyframes": keyframes], [])
  }

  static func finalCutTargeted(_ fields: [String: Value]) -> [String: Value] {
    var properties = finalCutTarget
    for (key, value) in fields { properties[key] = value }
    return properties
  }

  static let finalCutNote =
    "Live Final Cut Pro control through Accessibility with the app in the background (no activation, no keyboard/pointer input); reports frontmostChanged. Final Cut Pro autosaves: use disposable projects for experiments."

  static let finalCut: [ToolSpec] = [
    .init(
      name: "fcp_timeline_read",
      description:
        "Read the project open in the Final Cut Pro timeline: name, duration, playhead timecode and a page of clips (index, kind:name description, start, duration, selected). \(finalCutNote)",
      properties: finalCutTargeted([
        "offset": integer(0, 1_000_000), "limit": integer(1, 1000),
        "project": string(maximum: 512),
      ]), required: [], readOnly: true),
    .init(
      name: "fcp_timeline_select",
      description:
        "Select timeline clips by index in the named open project (refused if the timeline shows another project); each description must still match the last read, and the selection is read back. \(finalCutNote)",
      properties: finalCutTargeted([
        "clips": array(
          object(
            ["index": integer(0, 1_000_000), "description": string(maximum: 512)],
            ["index", "description"]), maximum: 500), "project": string(maximum: 512),
      ]), required: ["clips", "project"], readOnly: false),
    .init(
      name: "fcp_project_open",
      description:
        "Open a project in the timeline: selects the event row in the library sidebar and the project in the filmstrip browser, then runs Clip > Open Clip with Final Cut Pro briefly ACTIVE (its real keyboard focus decides what opens) and restores the previous app. If Open Clip opened a timeline clip instead, it navigates back and fails. Name the library when event names repeat.",
      properties: finalCutTargeted([
        "library": string(maximum: 512), "event": string(maximum: 512),
        "project": string(maximum: 512),
      ]), required: ["event", "project"], readOnly: false),
    .init(
      name: "fcp_playhead_move",
      description:
        "Move the playhead with Mark menu commands (start, end, range start/end, previous/next frame, edit or marker), repeating relative moves up to 600 times and reading the timecode back. \(finalCutNote)",
      properties: finalCutTargeted([
        "move": string(FCPPlayheadMove.allCases.map(\.rawValue)), "count": integer(1, 600),
        "project": string(maximum: 512),
      ]), required: ["move"], readOnly: false),
    .init(
      name: "fcp_playhead_seek",
      description:
        "Move the playhead to an exact HH:MM:SS:FF timecode without keyboard input: hops edit points that do not pass the target, then single frames, verifying each step. Fails (with the reached position) when no edit point lies within maximumSteps frames of the target. \(finalCutNote)",
      properties: finalCutTargeted([
        "timecode": string(maximum: 16), "maximumSteps": integer(1, 216_000),
        "project": string(maximum: 512),
      ]), required: ["timecode"], readOnly: false),
    .init(
      name: "fcp_timeline_edit",
      description:
        "Run one timeline command in the named open project (refused if the timeline shows another project): bladeAll (cut every lane at the playhead), delete (remove the selection), deselectAll or setClipRange. bladeAll/delete report whether the clip count or duration changed. Undo is not available from the background. \(finalCutNote)",
      properties: finalCutTargeted([
        "command": string(FCPEditCommand.allCases.map(\.rawValue)),
        "project": string(maximum: 512),
      ]), required: ["command", "project"], readOnly: false),
    .init(
      name: "fcp_export",
      description:
        "Export the project open in the timeline with File > Share > Export File into a NEW file in an existing folder inside the home folder. Final Cut Pro only enables sharing while active, so this ACTIVATES it (allowForeground must be true), sets format/codec and 'Save only', drives the Save panel through Accessibility and restores the previous frontmost app. Rendering continues after this returns: verify the file with media_inspect/media_verify_video.",
      properties: finalCutTargeted([
        "project": string(maximum: 512), "directory": string(), "fileName": string(maximum: 200),
        "format": string(FCPExportFormat.allCases.map(\.rawValue)), "codec": string(maximum: 128),
        "allowForeground": boolean,
      ]), required: ["project", "directory", "fileName", "allowForeground"], readOnly: false),
    .init(
      name: "fcp_effect_catalog",
      description:
        "List Final Cut Pro's installed Motion effect templates (uid for FCPXML, file name, localized name, category), optionally filtered by query. Read-only file scan of the app bundle.",
      properties: finalCutTargeted(["query": string(maximum: 128)]), required: [], readOnly: true
    ),
    .init(
      name: "fcp_inspector_read",
      description:
        "List the selected clip's inspector controls (value fields, enable checkboxes, pop-ups) with their current values, optionally showing the video/color/audio/info/title/text pane first; the previously shown pane is restored afterwards. Values show the playhead position. \(finalCutNote)",
      properties: finalCutTargeted([
        "tab": string(FCPInspectorTab.allCases.map(\.rawValue)), "project": string(maximum: 512),
      ]), required: [], readOnly: false),
    .init(
      name: "fcp_inspector_set",
      description:
        "Set one inspector parameter of the selected clip in the named open project (refused if the timeline shows another project): a value field (`value`, confirmed and read back numerically) or an effect/section enable checkbox (`enabled`). A requested pane is shown only for the change. Use fcp_effects_paste for keyframes. \(finalCutNote)",
      properties: finalCutTargeted([
        "parameter": string(maximum: 256), "value": string(maximum: 128), "enabled": boolean,
        "tab": string(FCPInspectorTab.allCases.map(\.rawValue)), "project": string(maximum: 512),
      ]), required: ["parameter", "project"], readOnly: false),
    .init(
      name: "fcp_effects_paste",
      description:
        "Apply effects, parameter values and keyframes to timeline clips: writes a carrier FCPXML (solid generator with opacity/position/scale/rotation/anchor adjustments and effects by uid, keyframes clip-relative in seconds), imports it in the background into the disposable Claude-Effect-Carriers library inside workDirectory, then with Final Cut Pro briefly ACTIVE copies it (OVERWRITES THE CLIPBOARD) and pastes onto the target clips of library/event/project. mode merge (default) uses Edit > Paste Attributes with only the carrier's attributes checked and keyframe timing maintained, keeping existing effects/attributes; mode replace uses Edit > Paste Effects, which replaces them. The carrier library is closed afterwards unless closeCarrierLibrary is false. Effect parameter keys come from fcp_effect_parameters (or Final Cut Pro's own XML). Verify with fcp_xml_export or rendered pixels.",
      properties: finalCutTargeted([
        "library": string(maximum: 512), "event": string(maximum: 512),
        "project": string(maximum: 512), "workDirectory": string(),
        "targets": array(
          object(
            ["index": integer(0, 1_000_000), "description": string(maximum: 512)],
            ["index", "description"]), maximum: 500),
        "durationSeconds": number, "frameDuration": string(maximum: 32),
        "opacity": animated(keyframeList), "position": animated(keyframeList),
        "scale": animated(keyframeList),
        "rotation": animated(keyframeList), "anchor": animated(keyframeList),
        "effects": array(
          object(
            [
              "uid": string(maximum: 512), "name": string(maximum: 256),
              "parameters": array(
                object(
                  [
                    "name": string(maximum: 256), "key": string(maximum: 512),
                    "value": string(maximum: 512), "keyframes": keyframeList,
                  ], ["name", "key"]), maximum: 64, minimum: 0),
            ], ["uid", "name"]), maximum: 16, minimum: 0),
        "mode": string(FCPPasteMode.allCases.map(\.rawValue)), "closeCarrierLibrary": boolean,
      ]),
      required: ["library", "event", "project", "targets", "workDirectory", "durationSeconds"],
      readOnly: false),
    .init(
      name: "fcp_xml_export",
      description:
        "Export the project open in the timeline as FCPXML (File > Export XML) into a NEW .fcpxmld/.fcpxml inside the home folder. Like fcp_export it ACTIVATES Final Cut Pro (allowForeground must be true) and restores the previous app. Use it to read effect parameter keys and verify keyframes.",
      properties: finalCutTargeted([
        "project": string(maximum: 512), "directory": string(), "fileName": string(maximum: 200),
        "allowForeground": boolean,
      ]), required: ["project", "directory", "fileName", "allowForeground"], readOnly: false),
    .init(
      name: "fcp_effect_parameters",
      description:
        "List the published parameters of one built-in effect template (uid from fcp_effect_catalog) with FCPXML keys derived from the template's object hierarchy. verified marks structures confirmed against Final Cut Pro's own XML (rig widgets, filters of an image node); check other keys with fcp_xml_export. Read-only file scan.",
      properties: finalCutTargeted(["uid": string(maximum: 512)]), required: ["uid"],
      readOnly: true),
    .init(
      name: "fcp_library_close",
      description:
        "Close one open library by its exact sidebar name (File > Close Library “name”), with Final Cut Pro briefly ACTIVE; refuses when the name matches no or several libraries. Closing only removes it from Final Cut Pro, files stay on disk.",
      properties: finalCutTargeted(["library": string(maximum: 512)]), required: ["library"],
      readOnly: false),
  ]

  static var finalCutNames: Set<String> { Set(finalCut.map(\.name)) }
}

extension NativeService {
  private struct FinalCutInput: Decodable {
    let bundleID: String?
    let offset: Int?
    let limit: Int?
    let clips: [FCPClipReference]?
    let library: String?
    let event: String?
    let project: String?
    let move: FCPPlayheadMove?
    let count: Int?
    let timecode: String?
    let maximumSteps: Int?
    let command: FCPEditCommand?
    let directory: String?
    let fileName: String?
    let format: FCPExportFormat?
    let codec: String?
    let allowForeground: Bool?
    let query: String?
    let tab: FCPInspectorTab?
    let parameter: String?
    let value: String?
    let enabled: Bool?
    let workDirectory: String?
    let targets: [FCPClipReference]?
    let durationSeconds: Double?
    let frameDuration: String?
    let opacity: FCPAnimatedValue?
    let position: FCPAnimatedValue?
    let scale: FCPAnimatedValue?
    let rotation: FCPAnimatedValue?
    let anchor: FCPAnimatedValue?
    let effects: [EffectInput]?
    let mode: FCPPasteMode?
    let closeCarrierLibrary: Bool?
    let uid: String?
  }

  /// MCP shape of one effect; parameters carry either value or keyframes.
  private struct EffectInput: Decodable {
    let uid: String
    let name: String
    let parameters: [ParameterInput]?
  }

  private struct ParameterInput: Decodable {
    let name: String
    let key: String
    let value: String?
    let keyframes: [FCPKeyframe]?
  }

  static let defaultTimelinePage = 200
  static let defaultSeekSteps = 54_000
  static let defaultFrameDuration = "1/30s"

  /// Translate validated MCP arguments into one typed Final Cut Pro request.
  func finalCutRequest(_ name: String, _ arguments: Value) throws -> UIRequest {
    let input = try decode(FinalCutInput.self, arguments)
    let target = UITarget(app: .finalCutPro, bundleID: input.bundleID)
    func required<T>(_ value: T?, _ field: String) throws -> T {
      guard let value else { throw ProAppsError.invalid("Missing \(field)") }
      return value
    }
    let request: FCPRequest
    switch name {
    case "fcp_timeline_read":
      request = .timeline(offset: input.offset ?? 0, limit: input.limit ?? Self.defaultTimelinePage)
    case "fcp_timeline_select": request = .select(try required(input.clips, "clips"))
    case "fcp_project_open":
      request = .openProject(
        library: input.library, event: try required(input.event, "event"),
        project: try required(input.project, "project"))
    case "fcp_playhead_move":
      request = .move(try required(input.move, "move"), count: input.count ?? 1)
    case "fcp_playhead_seek":
      request = .seek(
        timecode: try required(input.timecode, "timecode"),
        maximumSteps: input.maximumSteps ?? Self.defaultSeekSteps)
    case "fcp_timeline_edit": request = .edit(try required(input.command, "command"))
    case "fcp_export":
      request = .export(
        FCPExportRequest(
          project: try required(input.project, "project"),
          directory: try required(input.directory, "directory"),
          fileName: try required(input.fileName, "fileName"), format: input.format,
          codec: input.codec,
          allowForeground: try required(input.allowForeground, "allowForeground")))
    case "fcp_effect_catalog": request = .effectCatalog(query: input.query)
    case "fcp_inspector_read": request = .inspectorRead(tab: input.tab)
    case "fcp_inspector_set":
      request = .inspectorSet(
        FCPInspectorChange(
          parameter: try required(input.parameter, "parameter"), value: input.value,
          enabled: input.enabled, tab: input.tab))
    case "fcp_effects_paste":
      let effects = (input.effects ?? []).map { effect in
        FCPEffectSpec(
          uid: effect.uid, name: effect.name,
          parameters: (effect.parameters ?? []).map {
            FCPEffectParameter(
              name: $0.name, key: $0.key,
              animation: FCPAnimatedValue(value: $0.value, keyframes: $0.keyframes))
          })
      }
      request = .pasteEffects(
        FCPPasteRequest(
          library: try required(input.library, "library"),
          event: try required(input.event, "event"),
          project: try required(input.project, "project"),
          targets: try required(input.targets, "targets"),
          carrier: FCPCarrierSpec(
            durationSeconds: try required(input.durationSeconds, "durationSeconds"),
            frameDuration: input.frameDuration ?? Self.defaultFrameDuration,
            opacity: input.opacity, position: input.position, scale: input.scale,
            rotation: input.rotation, anchor: input.anchor, effects: effects),
          workDirectory: try required(input.workDirectory, "workDirectory"),
          mode: input.mode ?? .merge, closeCarrierLibrary: input.closeCarrierLibrary ?? true))
    case "fcp_xml_export":
      request = .xmlExport(
        FCPExportRequest(
          project: try required(input.project, "project"),
          directory: try required(input.directory, "directory"),
          fileName: try required(input.fileName, "fileName"),
          allowForeground: try required(input.allowForeground, "allowForeground")))
    case "fcp_effect_parameters": request = .effectParameters(uid: try required(input.uid, "uid"))
    case "fcp_library_close": request = .closeLibrary(try required(input.library, "library"))
    default: throw ProAppsError.invalid("Unknown tool")
    }
    // Tools that act on the open timeline run only inside the named project.
    let guarded: Set<String> = [
      "fcp_timeline_read", "fcp_timeline_select", "fcp_playhead_move", "fcp_playhead_seek",
      "fcp_timeline_edit", "fcp_inspector_read", "fcp_inspector_set",
    ]
    if guarded.contains(name), let project = input.project {
      return .finalCut(target, .inProject(project, request))
    }
    return .finalCut(target, request)
  }

  static func finalCutTimeout(_ request: UIRequest) -> Duration {
    switch request {
    case .finalCut(_, .export), .finalCut(_, .seek), .finalCut(_, .xmlExport),
      .finalCut(_, .pasteEffects), .finalCut(_, .inProject(_, .seek)):
      return .seconds(600)
    default: return .seconds(90)
    }
  }

  func finalCutTool(_ name: String, _ arguments: Value) async throws -> CallTool.Result {
    let request = try finalCutRequest(name, arguments)
    let response = try await interfaces.ui(request, Self.finalCutTimeout(request))
    guard var fields = try Value(response).objectValue else {
      throw ProAppsError.unavailable("Final Cut Pro result did not encode as an object")
    }
    fields["retrySafe"] = .bool(
      ["fcp_timeline_read", "fcp_effect_catalog", "fcp_effect_parameters"].contains(name))
    return self.response(fields)
  }
}
