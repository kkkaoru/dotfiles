import Foundation
import Testing

@testable import ProAppsCore

/// Synthetic inspector, carrier import and Export XML panel added to FakeFinalCut.
@MainActor
final class FakeEffectsUI {
  let world: FakeFinalCut
  var copied = false
  var pasted: [UIHandle] = []
  var importAppears = true
  var importActivates = false
  var valueAccepts = true
  var checkboxToggles = true
  var tabSwitches = true
  var xmlDirectory: URL?
  var xmlNamesProject = true
  var pasteAttributesEnabled = true
  var attributesDialogOpens = true
  var attributeToggles = true
  var attributesDialogCloses = true
  var libraryCloses = true
  var pastedAttributes: [String] = []
  var maintainTiming = false

  static let carrierLibrary = "Claude-Effect-Carriers"
  static let attributeTitles: [UIHandle: String] = [
    272: "エフェクト", 273: "ガウス", 274: "トランスフォーム", 275: "位置", 276: "回転", 277: "調整",
    278: "アンカー", 279: "クロップ", 280: "合成",
  ]

  static let tabs = [181: "ビデオ", 182: "カラー", 183: "オーディオ", 184: "情報"]

  init(_ world: FakeFinalCut) {
    self.world = world
    let backend = world.backend
    backend.installed[FakeUIBackend.fcp] = URL(fileURLWithPath: "/Applications/FCP.app")
    backend.attributes[100]?[.children] = .elements([110, 120, 130, 140, 160, 180])
    backend.attributes[180] = [
      .role: .string("AXGroup"), .children: .elements([181, 182, 183, 184]),
    ]
    for (handle, title) in Self.tabs {
      backend.attributes[handle] = [
        .role: .string("AXCheckBox"), .title: .string(title),
        .value: .number(handle == 183 ? 1 : 0),
      ]
    }
    backend.attributes[160] = [
      .role: .string("AXScrollArea"), .description: .string("inspector"),
      .children: .elements([161, 162, 163, 164, 165, 166, 167]),
    ]
    backend.attributes[161] = [
      .role: .string("AXTextField"), .description: .string("不透明度スクラバー"), .value: .string("100.0"),
    ]
    backend.attributes[162] = [
      .role: .string("AXCheckBox"), .description: .string("ガウスチェックボックス"), .value: .number(1),
    ]
    backend.attributes[163] = [
      .role: .string("AXPopUpButton"), .description: .string("ブレンドモードポップアップ"),
      .value: .string("標準"),
    ]
    backend.attributes[164] = [.role: .string("AXStaticText"), .value: .string("合成")]
    backend.attributes[165] = [.role: .string("AXTextField"), .description: .string("名前")]
    backend.attributes[166] = [.role: .string("AXTextField"), .description: .string("スクラバー")]
    backend.attributes[167] = [
      .role: .string("AXTextField"), .description: .string("回転スクラバー"), .value: .string("0"),
    ]
    world.menu(["編集", "コピー"])
    world.menu(["編集", "エフェクトをペースト"])
    for path in [
      ["編集", "カラー調整を追加"], ["編集", "カラーボードを追加"], ["編集", "クロスディゾルブを追加"],
      ["編集", "エフェクトを削除"],
    ] {
      world.menu(path)
    }
    world.menu(["ファイル", "XMLを書き出す…"], enabled: false)
    world.menu(["編集", "パラメータをペースト…"])
    world.menu(["ファイル", "ライブラリ“\(Self.carrierLibrary)”を閉じる"])
    world.enabledWhenActive = ["ファイル>XMLを書き出す…"]
    world.extraPress = { [weak self] handle in self?.press(handle) ?? false }
    world.extraWrite = { [weak self] name, value, handle in self?.write(name, value, handle) }
    backend.onOpen = { [weak self] _, document in self?.imported(document) }
  }

  func press(_ handle: UIHandle) -> Bool {
    let backend = world.backend
    if let title = Self.attributeTitles[handle] {
      guard attributeToggles, case .number(let state)? = backend.attributes[handle]?[.value] else {
        return true
      }
      backend.attributes[handle]?[.value] = .number(state == 0 ? 1 : 0)
      _ = title
      return true
    }
    switch handle {
    case world.item("編集>パラメータをペースト…"): openAttributesDialog()
    case world.item("ファイル>ライブラリ“\(Self.carrierLibrary)”を閉じる"):
      if libraryCloses {
        let rows = UIAutomation<FakeUIBackend>.handles(
          backend.attributes[130]?[.children] ?? .missing)
        backend.attributes[130]?[.children] = .elements(rows.filter { $0 != 138 && $0 != 139 })
      }
    case 282:
      backend.attributes[282]?[.value] = .number(1)
      backend.attributes[283]?[.value] = .number(0)
    case 284:
      maintainTiming = backend.attributes[282]?[.value] == .number(1)
      pastedAttributes = Self.attributeTitles.keys.sorted().filter {
        backend.attributes[$0]?[.value] == .number(1)
      }.compactMap { Self.attributeTitles[$0] }
      pasted = world.clips.indices.map(world.clipHandle).filter {
        backend.attributes[$0]?[.selected] == .bool(true)
      }
      if attributesDialogCloses { backend.attributes[1]?[.windows] = .elements([5, 100]) }
    case world.item("編集>コピー"): copied = true
    case world.item("編集>エフェクトをペースト"):
      pasted = world.clips.indices.map(world.clipHandle).filter {
        backend.attributes[$0]?[.selected] == .bool(true)
      }
    case world.item("ファイル>XMLを書き出す…"): openXMLPanel()
    case 181...184:
      guard tabSwitches else { return true }
      for tab in Self.tabs.keys {
        backend.attributes[tab]?[.value] = .number(tab == handle ? 1 : 0)
      }
    case 162:
      guard checkboxToggles, case .number(let state)? = backend.attributes[162]?[.value] else {
        return true
      }
      backend.attributes[162]?[.value] = .number(state == 0 ? 1 : 0)
    case 161, 167: return true
    case 315:
      // Save: write the XML document, then let the shared Save panel model close.
      if let xmlDirectory, let name = FakeFinalCut.text(backend.attributes[312]?[.value]) {
        let bundle = xmlDirectory.appendingPathComponent(name)
        do {
          try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
          try Data("<fcpxml/>".utf8).write(to: bundle.appendingPathComponent("Info.fcpxml"))
        } catch {
          Issue.record(error)
        }
      }
      return false
    default: return false
    }
    return true
  }

  func write(_ name: UIAttributeName, _ value: UISettableValue, _ handle: UIHandle) {
    guard name == .value, [161, 167].contains(handle), case .string(let text) = value else {
      return
    }
    let shown = valueAccepts ? Double(text).map { String(format: "%.1f", $0) } : "100.0"
    world.backend.attributes[handle]?[.value] = .string(shown ?? text)
  }

  func openAttributesDialog() {
    guard attributesDialogOpens else { return }
    let backend = world.backend
    backend.attributes[1]?[.windows] = .elements([5, 270, 100])
    backend.attributes[270] = [
      .role: .string("AXWindow"), .subrole: .string("AXDialog"),
      .children: .elements([271, 281, 284, 285, 286]),
    ]
    backend.attributes[271] = [
      .role: .string("AXScrollArea"), .children: .elements(Self.attributeTitles.keys.sorted()),
    ]
    let initiallyOn: Set<UIHandle> = [276, 279]
    for (handle, title) in Self.attributeTitles {
      backend.attributes[handle] = [
        .role: .string("AXCheckBox"), .title: .string(title),
        .value: .number(initiallyOn.contains(handle) ? 1 : 0),
      ]
    }
    backend.attributes[272]?[.title] = .missing
    backend.attributes[281] = [.role: .string("AXRadioGroup"), .children: .elements([282, 283])]
    backend.attributes[282] = [
      .role: .string("AXRadioButton"), .title: .string("保持"), .value: .number(0),
    ]
    backend.attributes[283] = [
      .role: .string("AXRadioButton"), .title: .string("伸ばして合わせる"), .value: .number(1),
    ]
    backend.attributes[284] = [.role: .string("AXButton"), .title: .string("ペースト")]
    backend.attributes[285] = [.role: .string("AXButton"), .title: .string("キャンセル")]
    backend.attributes[286] = [.role: .string("AXCheckBox"), .title: .string("ビデオパラメータ")]
  }

  func imported(_ document: URL) {
    guard importAppears else { return }
    let backend = world.backend
    let carrier = document.deletingPathExtension().lastPathComponent
    let rows = UIAutomation<FakeUIBackend>.handles(backend.attributes[130]?[.children] ?? .missing)
    backend.attributes[130]?[.children] = .elements(
      rows.filter { $0 != 138 && $0 != 139 } + [138, 139])
    backend.attributes[138] = [
      .role: .string("AXRow"), .disclosureLevel: .number(0),
      .description: .string("Claude-Effect-Carriers"),
    ]
    backend.attributes[139] = [
      .role: .string("AXRow"), .disclosureLevel: .number(1), .description: .string("Carriers"),
    ]
    backend.attributes[140]?[.children] = .elements([141, 143])
    backend.attributes[143] = [
      .role: .string("AXGroup"), .description: .string(carrier), .children: .elements([144]),
    ]
    backend.attributes[144] = [.role: .string("AXTextField"), .value: .string(carrier)]
    backend.attributes[world.item("編集>パラメータをペースト…")]?[.enabled] = .bool(
      pasteAttributesEnabled)
    if importActivates { world.activated(FakeFinalCut.finalCutPID) }
  }

  func openXMLPanel() {
    world.openSheet()
    let backend = world.backend
    backend.attributes[1]?[.windows] = .elements([5, 260, 100])
    backend.attributes[260] = [
      .role: .string("AXWindow"), .subrole: .string("AXDialog"), .children: .elements([261, 301]),
    ]
    backend.attributes[261] = [
      .role: .string("AXStaticText"),
      .value: .string(xmlNamesProject ? world.projectName : "Other"),
    ]
    backend.attributes[262] = [.role: .string("AXButton"), .title: .string("キャンセル")]
  }
}

@MainActor
struct FinalCutEffectsAutomationTests {
  let target = UITarget(app: .finalCutPro)

  func run(_ world: FakeFinalCut, _ request: FCPRequest, home: URL? = nil) async throws
    -> FCPResult
  {
    let automation = UIAutomation(
      backend: world.backend, home: home ?? URL(fileURLWithPath: "/nonexistent-home"),
      folderLocalizations: URL(fileURLWithPath: "/nonexistent-tables"))
    return try #require(try await automation.run(.finalCut(target, request)).finalCut)
  }

  func fixture() -> (FakeFinalCut, FakeEffectsUI) {
    let world = FakeFinalCut(homeName: "user")
    return (world, FakeEffectsUI(world))
  }

  // MARK: Inspector

  @Test func readsInspectorControlsAfterSwitchingTabs() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let result = try await run(world, .inspectorRead(tab: .video))
    #expect(world.backend.performed.contains("AXPress@181"))
    // The previously shown pane (オーディオ) is shown again afterwards.
    #expect(world.backend.attributes[183]?[.value] == .number(1))
    #expect(world.backend.attributes[181]?[.value] == .number(0))
    #expect(
      result.parameters == [
        FCPInspectorParameter(name: "不透明度", kind: .value, value: "100.0"),
        FCPInspectorParameter(name: "ガウス", kind: .checkbox, value: "1.0"),
        FCPInspectorParameter(name: "ブレンドモード", kind: .popup, value: "標準"),
        FCPInspectorParameter(name: "回転", kind: .value, value: "0"),
      ])
    let unchanged = try await run(world, .inspectorRead(tab: nil))
    #expect(unchanged.parameters?.count == 4)
  }

  @Test func reportsMissingInspectorAndStuckTabs() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    ui.tabSwitches = false
    await #expect(throws: ProAppsError.unavailable("The inspector did not switch to カラー")) {
      try await run(world, .inspectorRead(tab: .color))
    }
    _ = try await run(world, .inspectorRead(tab: .audio))
    world.backend.attributes[160]?[.description] = .string("other")
    await #expect(
      throws: ProAppsError.unavailable("The inspector is not shown; select a clip first")
    ) {
      try await run(world, .inspectorRead(tab: nil))
    }
  }

  @Test func setsValuesAndCheckboxesWithReadBack() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let value = try await run(
      world, .inspectorSet(FCPInspectorChange(parameter: "不透明度", value: "50", tab: .video)))
    #expect(value.parameters == [FCPInspectorParameter(name: "不透明度", kind: .value, value: "50.0")])
    #expect(value.changed == true)
    #expect(world.backend.performed.contains("AXConfirm@161"))
    let off = try await run(
      world, .inspectorSet(FCPInspectorChange(parameter: "ガウス", enabled: false)))
    #expect(off.parameters?.first?.value == "0.0")
    let same = try await run(
      world, .inspectorSet(FCPInspectorChange(parameter: "ガウス", enabled: false)))
    #expect(same.parameters?.first?.value == "0.0")
  }

  @Test(arguments: [
    (
      FCPInspectorChange(parameter: "不透明度", value: "1", enabled: true),
      "Give exactly one of value or enabled"
    ),
    (FCPInspectorChange(parameter: "不透明度"), "Give exactly one of value or enabled"),
    (
      FCPInspectorChange(parameter: "absent", value: "1"),
      "Parameter absent matched 0 inspector controls"
    ),
    (
      FCPInspectorChange(parameter: "不透明度", enabled: true),
      "Parameter 不透明度 matched 0 inspector controls"
    ),
  ])
  func rejectsInvalidInspectorChanges(_ change: FCPInspectorChange, _ reason: String) async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    await #expect(throws: ProAppsError.invalid(reason)) {
      try await run(world, .inspectorSet(change))
    }
  }

  @Test func reportsRejectedValuesAmbiguityAndStuckCheckboxes() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    ui.valueAccepts = false
    await #expect(throws: ProAppsError.unavailable("Final Cut Pro did not accept 50 for 不透明度")) {
      try await run(world, .inspectorSet(FCPInspectorChange(parameter: "不透明度", value: "50")))
    }
    ui.checkboxToggles = false
    await #expect(throws: ProAppsError.unavailable("ガウス did not change to false")) {
      try await run(world, .inspectorSet(FCPInspectorChange(parameter: "ガウス", enabled: false)))
    }
    world.backend.attributes[160]?[.children] = .elements([161, 167, 167])
    await #expect(throws: ProAppsError.invalid("Parameter 回転 matched 2 inspector controls")) {
      try await run(world, .inspectorSet(FCPInspectorChange(parameter: "回転", value: "5")))
    }
  }

  @Test func reportsAControlThatDisappearsAfterTheWrite() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    world.extraWrite = { [world] _, _, handle in
      if handle == 167 { world.backend.attributes[160]?[.children] = .elements([161]) }
    }
    await #expect(throws: ProAppsError.self) {
      try await run(world, .inspectorSet(FCPInspectorChange(parameter: "回転", value: "5")))
    }
  }

  // MARK: Carrier paste

  func work() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent(
      "carriers-\(UUID().uuidString)"
    ).resolvingSymlinksInPath()
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
  }

  func paste(
    _ work: URL, targets: [FCPClipReference]? = nil, directory: String? = nil,
    mode: FCPPasteMode = .merge, closeCarrier: Bool = true, effects: [FCPEffectSpec] = []
  ) -> FCPRequest {
    .pasteEffects(
      FCPPasteRequest(
        library: "Lib", event: "Ev", project: "Live90",
        targets: targets ?? [FCPClipReference(index: 1, description: "AVクリップ:clip")],
        carrier: FCPCarrierSpec(
          durationSeconds: 20,
          opacity: FCPAnimatedValue(keyframes: [
            FCPKeyframe(seconds: 1, value: "1"), FCPKeyframe(seconds: 2, value: "0"),
          ]), effects: effects), workDirectory: directory ?? work.path, mode: mode,
        closeCarrierLibrary: closeCarrier))
  }

  @Test func pastesACarrierOntoTheTargets() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let folder = try work()
    let result = try await run(world, paste(folder))
    let evidence = try #require(result.paste)
    #expect(evidence.carrierLibrary == "Claude-Effect-Carriers")
    #expect(evidence.carrierProject.hasPrefix("carrier-"))
    #expect(evidence.pastedClips.map(\.index) == [1])
    #expect(evidence.foregroundRestored)
    #expect(ui.copied)
    #expect(ui.pasted == [world.clipHandle(1)])
    #expect(world.projectName == "Live90")
    #expect(evidence.mode == .merge)
    #expect(evidence.carrierLibraryClosed)
    #expect(ui.pastedAttributes == ["合成"])
    #expect(ui.maintainTiming)
    let rowsAfter = UIAutomation<FakeUIBackend>.handles(
      world.backend.attributes[130]?[.children] ?? .missing)
    #expect(!rowsAfter.contains(138))
    #expect(world.backend.attributes[world.clipHandle(1)]?[.selected] == .bool(true))
    let xml = try String(contentsOfFile: evidence.carrierPath, encoding: .utf8)
    #expect(xml.contains(#"<keyframe time="30/30s" value="1" curve="linear"/>"#))
    #expect(world.backend.openedDocuments.first?.1 == URL(fileURLWithPath: "/Applications/FCP.app"))
    try FileManager.default.removeItem(at: folder)
  }

  @Test func restoresFocusWhenTheImportActivatesFinalCut() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    ui.importActivates = true
    let folder = try work()
    let result = try await run(world, paste(folder))
    #expect(result.paste?.foregroundRestored == true)
    #expect(world.backend.activated.last == FakeFinalCut.previousPID)
    try FileManager.default.removeItem(at: folder)
  }

  @Test func rejectsInvalidPasteRequests() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let folder = try work()
    await #expect(throws: ProAppsError.invalid("workDirectory must be an existing absolute folder"))
    {
      try await run(world, paste(folder, directory: "relative"))
    }
    await #expect(throws: ProAppsError.invalid("workDirectory must be an existing absolute folder"))
    {
      try await run(world, paste(folder, directory: folder.appendingPathComponent("missing").path))
    }
    await #expect(throws: ProAppsError.invalid("Name at least one target")) {
      try await run(world, paste(folder, targets: []))
    }
    world.backend.installed = [:]
    await #expect(throws: ProAppsError.unavailable("Selected app edition is not installed")) {
      try await run(world, paste(folder))
    }
    try FileManager.default.removeItem(at: folder)
  }

  @Test func reportsImportAndCarrierFailures() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let folder = try work()
    ui.importAppears = false
    await #expect(
      throws: ProAppsError.unavailable("Final Cut Pro did not import the carrier library")
    ) {
      try await run(world, paste(folder))
    }
    world.backend.openFailure = .unavailable("refused")
    await #expect(throws: ProAppsError.unavailable("refused")) {
      try await run(world, paste(folder))
    }
    world.backend.openFailure = nil
    ui.importAppears = true
    world.backend.onOpen = { [world, ui] _, document in
      ui.imported(document)
      world.clips = []
      world.refresh()
    }
    do {
      _ = try await run(world, paste(folder))
      Issue.record("paste without a carrier clip succeeded")
    } catch ProAppsError.unavailable(let reason) {
      #expect(reason.hasPrefix("The carrier project has no clip"))
    }
    try FileManager.default.removeItem(at: folder)
  }

  @Test func pastesEffectsInReplaceModeAndKeepsTheCarrierOpen() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let folder = try work()
    let effects = [
      FCPEffectSpec(uid: "FxPlug:9C13F991-BC99-4DC8-B150-381D7CCE183B", name: "Shadow")
    ]
    let merged = try await run(world, paste(folder, effects: effects))
    #expect(merged.paste?.mode == .merge)
    #expect(ui.pastedAttributes == ["ガウス", "合成"])
    let replaced = try await run(world, paste(folder, mode: .replace, closeCarrier: false))
    #expect(replaced.paste?.mode == .replace)
    #expect(replaced.paste?.carrierLibraryClosed == false)
    let rowsKept = UIAutomation<FakeUIBackend>.handles(
      world.backend.attributes[130]?[.children] ?? .missing)
    #expect(rowsKept.contains(138))
    try FileManager.default.removeItem(at: folder)
  }

  enum MergeFailure: CaseIterable {
    case disabled, noDialog, stuckToggle, staysOpen, libraryStaysOpen

    var reason: String {
      switch self {
      case .disabled: return "Paste Attributes is disabled; the clipboard changed"
      case .noDialog: return "The Paste Attributes dialog did not open"
      case .stuckToggle: return "Paste Attributes did not toggle 回転"
      case .staysOpen: return "The Paste Attributes dialog stayed open"
      case .libraryStaysOpen: return "Final Cut Pro kept the library Claude-Effect-Carriers open"
      }
    }
  }

  @Test(arguments: MergeFailure.allCases)
  func mergeFailuresCleanUp(_ failure: MergeFailure) async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    switch failure {
    case .disabled: ui.pasteAttributesEnabled = false
    case .noDialog: ui.attributesDialogOpens = false
    case .stuckToggle: ui.attributeToggles = false
    case .staysOpen: ui.attributesDialogCloses = false
    case .libraryStaysOpen: ui.libraryCloses = false
    }
    let folder = try work()
    do {
      _ = try await run(world, paste(folder))
      Issue.record("paste unexpectedly succeeded")
    } catch ProAppsError.invalid(let reason), ProAppsError.unavailable(let reason) {
      #expect(reason.hasPrefix(failure.reason), "\(reason)")
      #expect(reason.contains("previous app restored: true"))
    }
    try FileManager.default.removeItem(at: folder)
  }

  @Test func closesALibraryByItsExactName() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    ui.imported(URL(fileURLWithPath: "/w/carrier-1.fcpxml"))
    let result = try await run(world, .closeLibrary("Claude-Effect-Carriers"))
    #expect(result.libraryClosed == true)
    #expect(world.backend.activated == [FakeFinalCut.finalCutPID, FakeFinalCut.previousPID])
    do {
      _ = try await run(world, .closeLibrary("Lib"))
      Issue.record("closing an unoffered library succeeded")
    } catch ProAppsError.unavailable(let reason) {
      #expect(reason.hasPrefix("Final Cut Pro does not offer to close Lib"))
    }
    do {
      _ = try await run(world, .closeLibrary("Absent"))
      Issue.record("closing a missing library succeeded")
    } catch ProAppsError.invalid(let reason) {
      #expect(reason.hasPrefix("Library Absent matched 0 sidebar rows"))
    }
  }

  @Test func restoresTheInspectorPaneAfterFailures() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    world.backend.attributes[160]?[.description] = .string("other")
    await #expect(throws: ProAppsError.self) { try await run(world, .inspectorRead(tab: .video)) }
    #expect(world.backend.attributes[183]?[.value] == .number(1))
    ui.tabSwitches = false
    world.backend.attributes[183]?[.value] = .number(0)
    world.backend.attributes[181]?[.value] = .number(1)
    let switching = world.backend.onPerform
    world.backend.onPerform = { backend, action, handle in
      switching(backend, action, handle)
      if handle == 182 {
        backend.attributes[182]?[.value] = .number(1)
        backend.attributes[181]?[.value] = .number(0)
      }
    }
    do {
      _ = try await run(world, .inspectorRead(tab: .color))
      Issue.record("read unexpectedly succeeded")
    } catch ProAppsError.unavailable(let reason) {
      #expect(reason.contains("restoring the inspector pane failed"))
    }
  }

  @Test func derivesEffectParameterKeysOfTheInstalledEdition() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let app = FileManager.default.temporaryDirectory.appendingPathComponent(
      "params-\(UUID().uuidString)/FCP.app")
    let template = app.appendingPathComponent(
      FCPEffectCatalog.resources + "/PETemplates.localized/Effects.localized/Blur.localized/G.moef")
    try FileManager.default.createDirectory(
      at: template.deletingLastPathComponent(), withIntermediateDirectories: true)
    try Data(FinalCutEffectsTests.template.utf8).write(to: template)
    world.backend.installed[FakeUIBackend.fcp] = app
    let result = try await run(
      world, .effectParameters(uid: ".../Effects.localized/Blur.localized/G.moef"))
    #expect(result.effectParameters?.first?.key == "9999/10/100/11/2/100")
    world.backend.installed = [:]
    await #expect(throws: ProAppsError.unavailable("Selected app edition is not installed")) {
      try await run(world, .effectParameters(uid: ".../Effects.localized/Blur.localized/G.moef"))
    }
    try FileManager.default.removeItem(at: app.deletingLastPathComponent())
  }

  // MARK: Export XML

  func xmlRequest(_ directory: URL, project: String = "Live60", name: String = "check.fcpxmld")
    -> FCPRequest
  {
    .xmlExport(
      FCPExportRequest(
        project: project, directory: directory.path, fileName: name, allowForeground: true))
  }

  func xmlHome() throws -> URL {
    let home = FileManager.default.temporaryDirectory.appendingPathComponent(
      "xml-\(UUID().uuidString)/user"
    ).resolvingSymlinksInPath()
    try FileManager.default.createDirectory(
      at: home.appendingPathComponent("out"), withIntermediateDirectories: true)
    return home
  }

  @Test func exportsXMLAndWaitsForTheDocument() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let home = try xmlHome()
    world.tree = [["out"], []]
    ui.xmlDirectory = home.appendingPathComponent("out")
    let result = try await run(
      world, xmlRequest(home.appendingPathComponent("out")), home: home)
    let evidence = try #require(result.xmlExport)
    #expect(evidence.written)
    #expect(evidence.foregroundRestored)
    #expect(evidence.outputPath == home.appendingPathComponent("out/check.fcpxmld").path)
    ui.xmlDirectory = nil
    let unwritten = try await run(
      world, xmlRequest(home.appendingPathComponent("out"), name: "plain.fcpxml"), home: home)
    #expect(unwritten.xmlExport?.written == false)
    try FileManager.default.removeItem(at: home.deletingLastPathComponent())
  }

  @Test func xmlExportRejectsOtherProjectsAndMedia() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let home = try xmlHome()
    await #expect(throws: ProAppsError.self) {
      try await run(
        world, xmlRequest(home.appendingPathComponent("out"), project: "Other"), home: home)
    }
    await #expect(throws: ProAppsError.self) {
      try await run(
        world, xmlRequest(home.appendingPathComponent("out"), name: "a.mov"), home: home)
    }
    #expect(world.backend.activated.isEmpty)
    try FileManager.default.removeItem(at: home.deletingLastPathComponent())
  }

  enum XMLFailure: CaseIterable {
    case menuDisabled, panelMissing, otherProject, staysOpen

    var reason: String {
      switch self {
      case .menuDisabled: return "Final Cut Pro kept Export XML disabled while active"
      case .panelMissing: return "The Export XML panel did not open"
      case .otherProject: return "The Export XML panel names a different project"
      case .staysOpen: return "The Export XML panel stayed open"
      }
    }
  }

  @Test(arguments: XMLFailure.allCases)
  func xmlExportFailuresRestoreFocus(_ failure: XMLFailure) async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let home = try xmlHome()
    world.tree = [["out"], []]
    switch failure {
    case .menuDisabled: world.enabledWhenActive = []
    case .panelMissing:
      world.extraPress = { [world, ui] handle in
        handle == world.item("ファイル>XMLを書き出す…") ? true : ui.press(handle)
      }
    case .otherProject: ui.xmlNamesProject = false
    case .staysOpen: world.saveCloses = false
    }
    do {
      _ = try await run(world, xmlRequest(home.appendingPathComponent("out")), home: home)
      Issue.record("export unexpectedly succeeded")
    } catch ProAppsError.invalid(let reason), ProAppsError.unavailable(let reason) {
      #expect(reason.hasPrefix(failure.reason))
      #expect(reason.contains("previous app restored: true"))
    }
    try FileManager.default.removeItem(at: home.deletingLastPathComponent())
  }

  // MARK: Menu effects and catalog

  @Test(arguments: [
    FCPEditCommand.addColorAdjustments, .addColorBoard, .addCrossDissolve, .removeEffects,
  ])
  func runsMenuEffectCommands(_ command: FCPEditCommand) async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let result = try await run(world, .edit(command))
    #expect(result.changed == nil)
    #expect(result.clipCount == 2)
  }

  @Test func catalogsEffectsOfTheInstalledEdition() async throws {
    let (world, ui) = fixture()
    defer { withExtendedLifetime(ui) {} }
    let app = FileManager.default.temporaryDirectory.appendingPathComponent(
      "fcp-\(UUID().uuidString)/FCP.app")
    let template = app.appendingPathComponent(
      FCPEffectCatalog.resources + "/PETemplates.localized/Effects.localized/Blur.localized/G.moef")
    try FileManager.default.createDirectory(
      at: template.deletingLastPathComponent(), withIntermediateDirectories: true)
    try Data("<ozml/>".utf8).write(to: template)
    world.backend.installed[FakeUIBackend.fcp] = app
    let result = try await run(world, .effectCatalog(query: nil))
    #expect(result.effects?.map(\.uid) == [".../Effects.localized/Blur.localized/G.moef"])
    world.backend.installed = [:]
    await #expect(throws: ProAppsError.unavailable("Selected app edition is not installed")) {
      try await run(world, .effectCatalog(query: "x"))
    }
    try FileManager.default.removeItem(at: app.deletingLastPathComponent())
  }
}
