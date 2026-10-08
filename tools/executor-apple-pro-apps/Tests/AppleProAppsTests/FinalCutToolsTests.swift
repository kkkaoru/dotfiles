import Foundation
import MCP
import ProAppsCore
import Synchronization
import Testing

@testable import AppleProApps

struct FinalCutToolsTests {
  static let target = UITarget(app: .finalCutPro)

  static func response(_ result: FCPResult?) -> UIResponse {
    var response = UIResponse(
      focus: UIFocusEvidence(frontmostBefore: "dev.terminal", frontmostAfter: "dev.terminal"))
    response.finalCut = result
    return response
  }

  @Test(arguments: [
    ("fcp_timeline_read", [String: Value](), FCPRequest.timeline(offset: 0, limit: 200)),
    (
      "fcp_timeline_read", ["offset": .int(5), "limit": .int(10)],
      .timeline(offset: 5, limit: 10)
    ),
    (
      "fcp_timeline_select",
      ["clips": .array([.object(["index": .int(2), "description": .string("AVクリップ:a")])])],
      .select([FCPClipReference(index: 2, description: "AVクリップ:a")])
    ),
    (
      "fcp_project_open", ["event": .string("Ev"), "project": .string("P")],
      .openProject(library: nil, event: "Ev", project: "P")
    ),
    (
      "fcp_project_open",
      ["library": .string("Lib"), "event": .string("Ev"), "project": .string("P")],
      .openProject(library: "Lib", event: "Ev", project: "P")
    ),
    ("fcp_playhead_move", ["move": .string("nextEdit")], .move(.nextEdit, count: 1)),
    (
      "fcp_playhead_move", ["move": .string("previousFrame"), "count": .int(7)],
      .move(.previousFrame, count: 7)
    ),
    (
      "fcp_playhead_seek", ["timecode": .string("00:00:01:02")],
      .seek(timecode: "00:00:01:02", maximumSteps: 54_000)
    ),
    (
      "fcp_playhead_seek", ["timecode": .string("00:00:01:02"), "maximumSteps": .int(9)],
      .seek(timecode: "00:00:01:02", maximumSteps: 9)
    ),
    ("fcp_timeline_edit", ["command": .string("bladeAll")], .edit(.bladeAll)),
    (
      "fcp_timeline_edit", ["command": .string("delete"), "project": .string("P")],
      .inProject("P", .edit(.delete))
    ),
    (
      "fcp_inspector_read", ["project": .string("P")], .inProject("P", .inspectorRead(tab: nil))
    ),
    (
      "fcp_effect_parameters", ["uid": .string(".../Effects.localized/G.moef")],
      .effectParameters(uid: ".../Effects.localized/G.moef")
    ),
    ("fcp_library_close", ["library": .string("Lib")], .closeLibrary("Lib")),
    (
      "fcp_effects_paste",
      [
        "library": .string("L"), "event": .string("E"), "project": .string("P"),
        "workDirectory": .string("/w"), "durationSeconds": .double(5),
        "targets": .array([.object(["index": .int(0), "description": .string("a")])]),
        "mode": .string("replace"), "closeCarrierLibrary": .bool(false),
      ],
      .pasteEffects(
        FCPPasteRequest(
          library: "L", event: "E", project: "P",
          targets: [FCPClipReference(index: 0, description: "a")],
          carrier: FCPCarrierSpec(durationSeconds: 5), workDirectory: "/w", mode: .replace,
          closeCarrierLibrary: false))
    ),
    (
      "fcp_export",
      [
        "project": .string("P"), "directory": .string("/Users/u/out"),
        "fileName": .string("p.mov"), "format": .string("videoOnly"), "codec": .string("H.264"),
        "allowForeground": .bool(true),
      ],
      .export(
        FCPExportRequest(
          project: "P", directory: "/Users/u/out", fileName: "p.mov", format: .videoOnly,
          codec: "H.264", allowForeground: true))
    ),
    ("fcp_effect_catalog", [:], .effectCatalog(query: nil)),
    ("fcp_effect_catalog", ["query": .string("ブラー")], .effectCatalog(query: "ブラー")),
    ("fcp_inspector_read", [:], .inspectorRead(tab: nil)),
    ("fcp_inspector_read", ["tab": .string("color")], .inspectorRead(tab: .color)),
    (
      "fcp_inspector_set", ["parameter": .string("不透明度"), "value": .string("50")],
      .inspectorSet(FCPInspectorChange(parameter: "不透明度", value: "50"))
    ),
    (
      "fcp_inspector_set",
      ["parameter": .string("ガウス"), "enabled": .bool(false), "tab": .string("video")],
      .inspectorSet(FCPInspectorChange(parameter: "ガウス", enabled: false, tab: .video))
    ),
    (
      "fcp_xml_export",
      [
        "project": .string("P"), "directory": .string("/Users/u/out"),
        "fileName": .string("p.fcpxmld"), "allowForeground": .bool(true),
      ],
      .xmlExport(
        FCPExportRequest(
          project: "P", directory: "/Users/u/out", fileName: "p.fcpxmld", allowForeground: true))
    ),
    (
      "fcp_effects_paste",
      [
        "library": .string("L"), "event": .string("E"), "project": .string("P"),
        "workDirectory": .string("/w"), "durationSeconds": .double(30),
        "targets": .array([.object(["index": .int(0), "description": .string("AVクリップ:a")])]),
        "opacity": .object([
          "keyframes": .array([
            .object(["seconds": .double(1), "value": .string("1"), "curve": .string("smooth")])
          ])
        ]),
        "rotation": .object(["value": .string("15")]),
        "effects": .array([
          .object([
            "uid": .string(".../G.moef"), "name": .string("G"),
            "parameters": .array([
              .object(["name": .string("Amount"), "key": .string("9999/1"), "value": .string("1")])
            ]),
          ]),
          .object(["uid": .string("FFBoard"), "name": .string("B")]),
        ]),
      ],
      .pasteEffects(
        FCPPasteRequest(
          library: "L", event: "E", project: "P",
          targets: [FCPClipReference(index: 0, description: "AVクリップ:a")],
          carrier: FCPCarrierSpec(
            durationSeconds: 30,
            opacity: FCPAnimatedValue(keyframes: [
              FCPKeyframe(seconds: 1, value: "1", curve: .smooth)
            ]),
            rotation: FCPAnimatedValue(value: "15"),
            effects: [
              FCPEffectSpec(
                uid: ".../G.moef", name: "G",
                parameters: [
                  FCPEffectParameter(
                    name: "Amount", key: "9999/1", animation: FCPAnimatedValue(value: "1"))
                ]),
              FCPEffectSpec(uid: "FFBoard", name: "B"),
            ]), workDirectory: "/w"))
    ),
  ])
  func mapsToolArgumentsToTypedRequests(
    _ name: String, _ arguments: [String: Value], _ expected: FCPRequest
  ) async throws {
    let service = UIToolsTests.service { _, _ in Self.response(nil) }
    #expect(
      try await service.finalCutRequest(name, .object(arguments))
        == .finalCut(Self.target, expected))
  }

  @Test(arguments: [
    ("fcp_timeline_select", [String: Value](), "Missing clips"),
    ("fcp_project_open", ["project": .string("P")], "Missing event"),
    ("fcp_project_open", ["event": .string("E")], "Missing project"),
    ("fcp_playhead_move", [:], "Missing move"),
    ("fcp_playhead_seek", [:], "Missing timecode"),
    ("fcp_timeline_edit", [:], "Missing command"),
    (
      "fcp_export", ["directory": .string("/d"), "fileName": .string("f.mov")],
      "Missing project"
    ),
    ("fcp_export", ["project": .string("P"), "fileName": .string("f.mov")], "Missing directory"),
    ("fcp_export", ["project": .string("P"), "directory": .string("/d")], "Missing fileName"),
    (
      "fcp_export",
      ["project": .string("P"), "directory": .string("/d"), "fileName": .string("f.mov")],
      "Missing allowForeground"
    ),
    ("fcp_inspector_set", [:], "Missing parameter"),
    ("fcp_effect_parameters", [:], "Missing uid"),
    ("fcp_library_close", [:], "Missing library"),
    ("fcp_effects_paste", [:], "Missing library"),
    (
      "fcp_effects_paste",
      [
        "library": .string("L"), "event": .string("E"), "project": .string("P"),
        "targets": .array([]), "workDirectory": .string("/w"),
      ], "Missing durationSeconds"
    ),
    ("fcp_unknown", [:], "Unknown tool"),
  ])
  func rejectsIncompleteToolArguments(
    _ name: String, _ arguments: [String: Value], _ reason: String
  ) async throws {
    let service = UIToolsTests.service { _, _ in Self.response(nil) }
    await #expect(throws: ProAppsError.invalid(reason)) {
      try await service.finalCutRequest(name, .object(arguments))
    }
  }

  @Test func boundsChildTimeoutsByRequest() {
    let export = FCPExportRequest(
      project: "P", directory: "/d", fileName: "f.mov", allowForeground: true)
    #expect(NativeService.finalCutTimeout(.finalCut(Self.target, .export(export))) == .seconds(600))
    #expect(
      NativeService.finalCutTimeout(.finalCut(Self.target, .seek(timecode: "x", maximumSteps: 1)))
        == .seconds(600))
    #expect(NativeService.finalCutTimeout(.finalCut(Self.target, .edit(.delete))) == .seconds(90))
    #expect(
      NativeService.finalCutTimeout(.finalCut(Self.target, .xmlExport(export))) == .seconds(600))
    let paste = FCPPasteRequest(
      library: "L", event: "E", project: "P", targets: [],
      carrier: FCPCarrierSpec(durationSeconds: 1), workDirectory: "/w")
    #expect(
      NativeService.finalCutTimeout(.finalCut(Self.target, .pasteEffects(paste))) == .seconds(600))
  }

  @Test func routesFinalCutToolsThroughTheChildInterface() async throws {
    let seen = Mutex<[UIRequest]>([])
    let result = FCPResult(
      project: FCPProjectState(name: "P", duration: "01:00:00", playhead: "00:00:00:00"),
      language: "ja", vocabularyVerified: true)
    let service = UIToolsTests.service { request, _ in
      seen.withLock { $0.append(request) }
      return Self.response(result)
    }
    let read = await service.call(.init(name: "fcp_timeline_read", arguments: [:]))
    #expect(read.isError == false)
    let fields = try #require(read.structuredContent?.objectValue)
    #expect(fields["retrySafe"] == .bool(true))
    #expect(fields["finalCut"]?.objectValue?["language"] == .string("ja"))
    let edit = await service.call(
      .init(
        name: "fcp_timeline_edit",
        arguments: ["command": .string("delete"), "project": .string("P")]))
    #expect(edit.structuredContent?.objectValue?["retrySafe"] == .bool(false))
    #expect(seen.withLock { $0.count } == 2)
  }

  @Test func rejectsSchemaViolationsAndReportsChildFailures() async throws {
    let service = UIToolsTests.service { _, _ in Self.response(nil) }
    let badMove = await service.call(
      .init(name: "fcp_playhead_move", arguments: ["move": .string("jump")]))
    #expect(badMove.isError == true)
    let failing = UIToolsTests.service { _, _ in throw ProAppsError.unavailable("not running") }
    let failed = await failing.call(.init(name: "fcp_timeline_read", arguments: [:]))
    #expect(failed.isError == true)
  }

  @Test func advertisesTheFinalCutTools() {
    #expect(ToolSpec.finalCutNames.count == 14)
    #expect(ToolSpec.all.filter { ToolSpec.finalCutNames.contains($0.name) }.count == 14)
    #expect(
      ToolSpec.finalCut.filter(\.readOnly).map(\.name)
        == ["fcp_timeline_read", "fcp_effect_catalog", "fcp_effect_parameters"])
    let export = ToolSpec.finalCut.first { $0.name == "fcp_export" }
    #expect(export?.required.contains("allowForeground") == true)
  }
}
