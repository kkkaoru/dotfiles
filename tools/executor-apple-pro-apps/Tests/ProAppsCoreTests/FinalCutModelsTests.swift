import Foundation
import Testing

@testable import ProAppsCore

struct FinalCutModelsTests {
  @Test(
    arguments: [
      ("00:00:27:10", "00:00:27:10"), ("1:02:03;04", "01:02:03:04"), ("10:59:59:100", nil),
      ("00:00:27", nil), ("00:60:00:00", nil), ("00:00:60:00", nil), ("00:00:00:120", nil),
      ("+1:00:00:00", nil), ("00::00:00", nil), ("０0:00:00:00", nil),
    ] as [(String, String?)])
  func parsesTimecodes(_ text: String, _ expected: String?) {
    #expect(FCPTimecode(text)?.description == expected)
  }

  @Test func ordersTimecodesFieldByField() throws {
    let early = try #require(FCPTimecode("00:00:59:29"))
    let late = try #require(FCPTimecode("00:01:00:00"))
    #expect(early < late)
    #expect(!(late < early))
    #expect(late == FCPTimecode("00:01:00:00"))
  }

  @Test func classifiesMovesAndCommands() {
    #expect(
      FCPPlayheadMove.allCases.filter(\.isAbsolute) == [.start, .end, .rangeStart, .rangeEnd])
    #expect(FCPEditCommand.allCases.filter(\.changesTimeline) == [.bladeAll, .delete])
  }

  @Test func detectsVocabulariesAndResolvesEveryPath() throws {
    #expect(FCPVocabulary.detect(menuBarTitles: ["Apple", "ファイル"]) == .japanese)
    #expect(FCPVocabulary.detect(menuBarTitles: ["File"])?.verified == true)
    #expect(FCPVocabulary.detect(menuBarTitles: ["Datei"]) == nil)
    for vocabulary in [FCPVocabulary.japanese, .english] {
      for move in FCPPlayheadMove.allCases { #expect(try vocabulary.path(move).count >= 2) }
      for command in FCPEditCommand.allCases { #expect(try vocabulary.path(command).count == 2) }
      for format in FCPExportFormat.allCases { #expect(!(try vocabulary.title(format)).isEmpty) }
      for tab in FCPInspectorTab.allCases { #expect(!(try vocabulary.title(tab)).isEmpty) }
    }
    #expect(try FCPVocabulary.japanese.path(.nextEdit) == ["マーク", "次へ", "編集"])
  }

  @Test func rejectsIncompleteVocabularies() {
    let empty = FCPVocabulary(
      language: "xx", verified: false, menuBarKey: "X", share: [], defaultDestinationMarker: "",
      openClip: [], moves: [:], edits: [:], settingsTab: "", next: "", cancel: "", save: "",
      formatLabel: "", codecLabel: "", actionLabel: "", formats: [:], saveOnly: "", copy: [],
      pasteEffects: [], xmlExport: [], tabs: [:], valueSuffix: "", checkboxSuffix: "",
      popupSuffix: "", pasteAttributes: [], closeLibraryPrefix: "", closeLibrarySuffix: "",
      attributeLabels: FCPVocabulary.english.attributeLabels)
    #expect(throws: ProAppsError.invalid("Unsupported move")) { try empty.path(.start) }
    #expect(throws: ProAppsError.invalid("Unsupported command")) { try empty.path(.delete) }
    #expect(throws: ProAppsError.invalid("Unsupported format")) { try empty.title(.videoOnly) }
    #expect(throws: ProAppsError.invalid("Unsupported tab")) { try empty.title(.video) }
  }

  @Test func encodesRequestsAndEvidence() throws {
    let request = UIRequest.finalCut(
      UITarget(app: .finalCutPro),
      .export(
        FCPExportRequest(
          project: "P", directory: "/d", fileName: "f.mov", format: .videoOnly, codec: "H.264",
          allowForeground: true)))
    let data = try JSONEncoder().encode(request)
    #expect(try JSONDecoder().decode(UIRequest.self, from: data) == request)
    let evidence = FCPExportEvidence(
      outputPath: "/d/f.mov", dialogInfo: ["k": "v"], format: nil, codec: "H.264",
      action: "Save only", foregroundRestored: true)
    let encoded = try JSONEncoder().encode(evidence)
    let fields = try JSONDecoder().decode(EvidenceFields.self, from: encoded)
    #expect(fields.completionVerified == false)
    #expect(fields.format == nil)
    #expect(try JSONDecoder().decode(FCPExportEvidence.self, from: encoded) == evidence)
  }

  @Test func elementHandlesAreNotEncodable() {
    let request = UIRequest.set(
      UITarget(app: .finalCutPro), window: UIWindowLocator(index: 0),
      target: UIElementLocator(path: [0]), attribute: .selected, value: .elements([1]))
    #expect(throws: EncodingError.self) { try JSONEncoder().encode(request) }
  }

  /// The encoded evidence fields these tests compare.
  struct EvidenceFields: Decodable {
    let completionVerified: Bool
    let format: String?
  }

  // MARK: Export plans

  struct Sandbox {
    let root: URL
    let home: URL
    let tables: URL
  }

  func sandbox() throws -> Sandbox {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "fcp-plan-\(UUID().uuidString)"
    ).resolvingSymlinksInPath()
    let home = root.appendingPathComponent("home")
    try FileManager.default.createDirectory(
      at: home.appendingPathComponent("Movies/out"), withIntermediateDirectories: true)
    try FileManager.default.createDirectory(
      at: home.appendingPathComponent(".hidden"), withIntermediateDirectories: true)
    try Data().write(to: home.appendingPathComponent("Movies/.localized"))
    try Data().write(to: home.appendingPathComponent("Movies/out/taken.mov"))
    try Data().write(to: home.appendingPathComponent("file.txt"))
    try FileManager.default.createSymbolicLink(
      at: home.appendingPathComponent("link"),
      withDestinationURL: home.appendingPathComponent("Movies"))
    let tables = root.appendingPathComponent("tables")
    let lproj = tables.appendingPathComponent("ja.lproj")
    try FileManager.default.createDirectory(at: lproj, withIntermediateDirectories: true)
    try PropertyListEncoder().encode(["Movies": "ムービー"]).write(
      to: lproj.appendingPathComponent("SystemFolderLocalizations.strings"))
    let bad = tables.appendingPathComponent("xx.lproj")
    try FileManager.default.createDirectory(at: bad, withIntermediateDirectories: true)
    try Data("not a plist".utf8).write(
      to: bad.appendingPathComponent("SystemFolderLocalizations.strings"))
    return Sandbox(root: root, home: home, tables: tables)
  }

  func request(_ directory: String, _ name: String = "new.mov", foreground: Bool = true)
    -> FCPExportRequest
  {
    FCPExportRequest(
      project: "P", directory: directory, fileName: name, allowForeground: foreground)
  }

  @Test func plansLocalizedSavePanelRoutes() throws {
    let box = try sandbox()
    let path = box.home.appendingPathComponent("Movies/out").path
    let japanese = try FCPExportPlan.make(
      request(path), home: box.home, language: "ja", localizations: box.tables)
    #expect(japanese.components == ["ムービー", "out"])
    #expect(japanese.homeDisplayName == "home")
    #expect(japanese.outputPath == path + "/new.mov")
    let english = try FCPExportPlan.make(
      request(path), home: box.home, language: "en", localizations: box.tables)
    #expect(english.components == ["Movies", "out"])
    let home = try FCPExportPlan.make(
      request(box.home.path), home: box.home, language: "ja", localizations: box.tables)
    #expect(home.components.isEmpty)
    try FileManager.default.removeItem(at: box.root)
  }

  @Test func rejectsUnsafeExportRequests() throws {
    let box = try sandbox()
    let out = box.home.appendingPathComponent("Movies/out").path
    let cases: [FCPExportRequest] = [
      request(out, foreground: false), request(out, "a/b.mov"), request(out, "a:b.mov"),
      request(out, ".hidden.mov"), request(out, "clip.avi"), request(out, ""),
      request(out, String(repeating: "a", count: 201) + ".mov"), request("relative/out"),
      request(out + "/../out"), request(box.home.appendingPathComponent("link").path),
      request(box.root.path), request(box.home.appendingPathComponent(".hidden").path),
      request(box.home.appendingPathComponent("missing").path),
      request(box.home.appendingPathComponent("file.txt").path), request(out, "taken.mov"),
    ]
    for invalid in cases {
      #expect(throws: ProAppsError.self) {
        try FCPExportPlan.make(invalid, home: box.home, language: "ja", localizations: box.tables)
      }
    }
    #expect(throws: ProAppsError.unavailable("The system folder localization table is malformed")) {
      try FCPExportPlan.make(
        request(out), home: box.home, language: "xx", localizations: box.tables)
    }
    try FileManager.default.removeItem(at: box.root)
  }
}
