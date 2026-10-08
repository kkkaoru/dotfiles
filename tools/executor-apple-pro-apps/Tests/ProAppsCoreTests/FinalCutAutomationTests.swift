import Foundation
import Testing

@testable import ProAppsCore

/// A synthetic Final Cut Pro: a 30 fps timeline with clips and a playhead, the
/// menus used by the live tools, a library sidebar, a filmstrip browser, and the
/// share dialog with its Save panel. Menu presses mutate the model through hooks.
@MainActor
final class FakeFinalCut {
  static let fps = 30
  let backend = FakeUIBackend()
  let homeName: String
  var homeNameOverride: String?
  var clips: [(start: Int, duration: Int)] = [(0, 300), (300, 600)]
  var playhead = 0
  var projectName = "Live60"
  var menuItems: [String: UIHandle] = [:]
  var nextMenuHandle = 1000
  var browserPath: [String]? = nil
  var tree: [[String]] = [["ムービー", "Other"], ["out"], []]
  var shareOpened = false
  var savedName: String?
  var dialogCancelled = false
  /// Extension points for additional synthetic UI (inspector, carriers, XML export).
  var extraPress: ((UIHandle) -> Bool)?
  var extraWrite: ((UIAttributeName, UISettableValue, UIHandle) -> Void)?
  var enabledWhenActive: [String] = []

  // Behaviour switches for failure paths.
  var playheadFrozen = false
  var openClipWorks = true
  var selectionApplies = true
  var saveCloses = true
  var nextOpensSheet = true
  var popupMenuOpens = true
  var shareEnabledWhenActive = true
  var openClipOpensClip = false
  var openedClip: String?
  var disclosureShowsBrowser = true
  var nameFieldAccepts = true

  static let previousPID: Int32 = 9
  static let finalCutPID: Int32 = 42

  init(homeName: String) {
    self.homeName = homeName
    backend.processes[FakeUIBackend.fcp] = UIRunningProcess(
      pid: Self.finalCutPID, bundleID: FakeUIBackend.fcp)
    backend.frontmostPID = Self.previousPID
    backend.appElements = [Self.finalCutPID: 1, Self.previousPID: 2]
    let prefix = "editor/timelineContainer/toolbar/"
    backend.attributes = [
      1: [.windows: .elements([5, 100]), .menuBar: .element(30), .frontmost: .bool(false)],
      2: [.frontmost: .bool(true)],
      5: [.role: .string("AXWindow"), .subrole: .string("AXUnknown")],
      100: [
        .role: .string("AXWindow"), .subrole: .string("AXStandardWindow"), .main: .bool(true),
        .children: .elements([110, 120, 130, 140]),
      ],
      110: [.role: .string("AXGroup"), .children: .elements([111, 112, 113, 115])],
      115: [
        .role: .string("AXButton"), .identifier: .string(prefix + "timelineNavigationBackButton"),
      ],
      111: [
        .role: .string("AXMenuButton"), .identifier: .string(prefix + "projectNamePopUpButton"),
      ],
      112: [.role: .string("AXStaticText"), .identifier: .string(prefix + "projectInfo")],
      113: [.role: .string("AXTable"), .children: .elements([114])],
      114: [.role: .string("AXRow")],
      120: [.role: .string("AXLayoutArea"), .subrole: .string("AXTimeline")],
      129: [.role: .string("AXValueIndicator")],
      130: [.role: .string("AXOutline"), .children: .elements([131, 132, 133, 134, 135, 136])],
      131: [.role: .string("AXRow"), .disclosureLevel: .number(0), .description: .string("Lib")],
      132: [.role: .string("AXRow"), .disclosureLevel: .number(1), .description: .string("Ev")],
      133: [.role: .string("AXRow"), .disclosureLevel: .number(0), .description: .string("Other")],
      134: [.role: .string("AXRow"), .disclosureLevel: .number(1), .children: .elements([137])],
      135: [.role: .string("AXRow"), .description: .string("Ev")],
      136: [.role: .string("AXStaticText"), .value: .string("not a row")],
      137: [.role: .string("AXCell"), .value: .string("Ev")],
      140: [.role: .string("AXGroup"), .children: .elements([141])],
      141: [
        .role: .string("AXGroup"), .description: .string("Live90"), .children: .elements([142]),
      ],
      142: [.role: .string("AXTextField"), .value: .string("Live90")],
    ]
    menu(["ファイル", "共有", "ファイルを書き出す（デフォルト）…"], enabled: false)
    menu(["クリップ", "クリップを開く"])
    for path in [
      ["マーク", "移動", "開始"], ["マーク", "移動", "終了"], ["マーク", "移動", "範囲開始点"],
      ["マーク", "移動", "範囲終了点"], ["マーク", "前へ", "フレーム"], ["マーク", "次へ", "フレーム"],
      ["マーク", "前へ", "編集"], ["マーク", "次へ", "編集"], ["マーク", "前へ", "マーカー"],
      ["マーク", "次へ", "マーカー"], ["マーク", "クリップ範囲を設定"], ["トリム", "すべてをブレード"],
      ["編集", "削除"], ["編集", "すべてを選択解除"],
    ] {
      menu(path)
    }
    // Weak captures: the backend is owned by this model and must not retain it back.
    backend.onPerform = { [weak self] _, _, handle in self?.pressed(handle) }
    backend.onSet = { [weak self] _, name, value, handle in self?.wrote(name, value, handle) }
    backend.onActivate = { [weak self] _, pid in self?.activated(pid) }
    refresh()
  }

  // MARK: Menus

  /// Add a menu item at `path`, creating intermediate menus, and return its handle.
  @discardableResult
  func menu(_ path: [String], enabled: Bool = true) -> UIHandle {
    var container: UIHandle = 30
    for (offset, title) in path.enumerated() {
      let existing = UIAutomation<FakeUIBackend>.handles(
        backend.attributes[container]?[.children] ?? .missing
      ).first { backend.attributes[$0]?[.title] == .string(title) }
      let item = existing ?? newHandle()
      if existing == nil {
        let siblings = UIAutomation<FakeUIBackend>.handles(
          backend.attributes[container]?[.children] ?? .missing)
        backend.attributes[container, default: [:]][.children] = .elements(siblings + [item])
        backend.attributes[item] = [.title: .string(title), .enabled: .bool(true)]
      }
      guard offset < path.count - 1 else {
        backend.attributes[item]?[.enabled] = .bool(enabled)
        menuItems[path.joined(separator: ">")] = item
        return item
      }
      if case .elements(let submenus)? = backend.attributes[item]?[.children],
        let submenu = submenus.first
      {
        container = submenu
      } else {
        let submenu = newHandle()
        backend.attributes[item]?[.children] = .elements([submenu])
        backend.attributes[submenu] = [.children: .elements([])]
        container = submenu
      }
    }
    return container
  }

  func newHandle() -> UIHandle {
    nextMenuHandle += 1
    return nextMenuHandle
  }

  func item(_ path: String) -> UIHandle { menuItems[path] ?? -1 }

  // MARK: Model rendering

  static func timecode(_ frame: Int) -> String {
    let seconds = frame / fps
    return String(
      format: "%02d:%02d:%02d:%02d", seconds / 3600, seconds / 60 % 60, seconds % 60, frame % fps)
  }

  var total: Int { clips.map(\.duration).reduce(0, +) }

  func clipHandle(_ index: Int) -> UIHandle { 150 + index }

  func refresh() {
    let handles = clips.indices.map(clipHandle)
    for (index, clip) in clips.enumerated() {
      let selected = backend.attributes[clipHandle(index)]?[.selected] ?? .bool(false)
      backend.attributes[clipHandle(index)] = [
        .role: .string("AXLayoutItem"), .subrole: .string("AXTimeline"),
        .description: .string("AVクリップ:clip"), .value: .string(Self.timecode(clip.duration)),
        .valueDescription: .string(Self.timecode(clip.start)), .selected: selected,
      ]
    }
    backend.attributes[120]?[.children] = .elements(handles + [129, 149])
    backend.attributes[149] = [.role: .string("AXLayoutItem")]
    backend.attributes[129]?[.value] = .string(Self.timecode(playhead))
    backend.attributes[111]?[.title] = .string(projectName)
    backend.attributes[111]?[.value] = openedClip.map(UIAttributeValue.string) ?? .missing
    backend.attributes[112]?[.value] = .string(Self.timecode(total))
  }

  // MARK: Hooks

  func activated(_ pid: Int32) {
    backend.attributes[1]?[.frontmost] = .bool(pid == Self.finalCutPID)
    backend.attributes[2]?[.frontmost] = .bool(pid == Self.previousPID)
    backend.attributes[item("ファイル>共有>ファイルを書き出す（デフォルト）…")]?[.enabled] = .bool(
      pid == Self.finalCutPID && shareEnabledWhenActive)
    for path in enabledWhenActive {
      backend.attributes[item(path)]?[.enabled] = .bool(pid == Self.finalCutPID)
    }
  }

  /// Description of the project selected in the filmstrip browser.
  func selectedBrowserProject() -> String? {
    UIAutomation<FakeUIBackend>.handles(backend.attributes[140]?[.selectedChildren] ?? .missing)
      .first.flatMap { Self.text(backend.attributes[$0]?[.description]) }
  }

  func pressed(_ handle: UIHandle) {
    let boundaries = clips.map(\.start) + [total]
    switch handle {
    case item("マーク>移動>開始"): move(to: 0)
    case item("マーク>移動>終了"): move(to: total)
    case item("マーク>次へ>フレーム"): move(to: min(total, playhead + 1))
    case item("マーク>前へ>フレーム"): move(to: max(0, playhead - 1))
    case item("マーク>次へ>編集"): move(to: boundaries.first { $0 > playhead } ?? playhead)
    case item("マーク>前へ>編集"): move(to: boundaries.last { $0 < playhead } ?? playhead)
    case item("トリム>すべてをブレード"): blade()
    case item("編集>削除"): deleteSelection()
    case item("クリップ>クリップを開く"):
      if openClipWorks {
        projectName = selectedBrowserProject() ?? "Live90"
      } else if openClipOpensClip {
        openedClip = "Cut 1"
      }
    case 115: openedClip = nil
    case item("ファイル>共有>ファイルを書き出す（デフォルト）…"): openShare()
    default:
      if extraPress?(handle) != true { pressedDialog(handle) }
    }
    refresh()
  }

  func move(to frame: Int) {
    if !playheadFrozen { playhead = frame }
  }

  func blade() {
    guard
      let index = clips.firstIndex(where: {
        $0.start < playhead && playhead < $0.start + $0.duration
      })
    else { return }
    let clip = clips[index]
    clips.replaceSubrange(
      index...index,
      with: [
        (clip.start, playhead - clip.start), (playhead, clip.start + clip.duration - playhead),
      ])
  }

  func deleteSelection() {
    let kept = clips.indices.filter {
      backend.attributes[clipHandle($0)]?[.selected] != .bool(true)
    }
    var start = 0
    clips = kept.map { index in
      defer { start += clips[index].duration }
      return (start, clips[index].duration)
    }
    for index in 0..<10 { backend.attributes[clipHandle(index)]?[.selected] = .bool(false) }
  }

  func wrote(_ name: UIAttributeName, _ value: UISettableValue, _ handle: UIHandle) {
    extraWrite?(name, value, handle)
    if handle == 312, !nameFieldAccepts { backend.attributes[312]?[.value] = .string("other.mov") }
    guard case .elements(let handles) = value else { return }
    switch (name, handle) {
    case (.selectedChildren, 120) where selectionApplies:
      for index in clips.indices {
        backend.attributes[clipHandle(index)]?[.selected] = .bool(
          handles.contains(clipHandle(index)))
      }
    case (.selectedRows, 302):
      browserPath = handles == [304] ? [] : nil
      backend.attributes[304]?[.selected] = .bool(handles == [304])
      renderBrowser()
    case (.selectedChildren, _):
      selectInColumn(list: handle, items: handles)
    default: break
    }
  }

  // MARK: Share dialog and Save panel

  func openShare() {
    shareOpened = true
    backend.attributes[1]?[.windows] = .elements([5, 200, 100])
    backend.attributes[200] = [
      .role: .string("AXWindow"), .subrole: .string("AXDialog"),
      .children: .elements([201, 202, 203, 204, 218, 216, 217]),
    ]
    backend.attributes[201] = [.role: .string("AXTextField"), .value: .string(projectName)]
    backend.attributes[202] = [
      .role: .string("AXStaticText"), .description: .string("継続時間"),
      .value: .string(Self.timecode(total)),
    ]
    backend.attributes[218] = [.role: .string("AXStaticText"), .description: .string("種類")]
    backend.attributes[203] = [
      .role: .string("AXTabGroup"), .children: .elements([205, 206, 207, 210, 211, 214, 215]),
    ]
    backend.attributes[204] = [.role: .string("AXStaticText"), .value: .string("unlabelled")]
    backend.attributes[205] = [.role: .string("AXRadioButton"), .title: .string("設定")]
    backend.attributes[206] = [.role: .string("AXStaticText"), .value: .string("フォーマット: ")]
    backend.attributes[207] = [.role: .string("AXPopUpButton"), .value: .string("ビデオのみ")]
    backend.attributes[210] = [.role: .string("AXStaticText"), .value: .string("ビデオコーデック: ")]
    backend.attributes[211] = [
      .role: .string("AXPopUpButton"), .value: .string("Apple ProRes 4444"),
    ]
    backend.attributes[214] = [.role: .string("AXStaticText"), .value: .string("操作: ")]
    backend.attributes[215] = [.role: .string("AXPopUpButton"), .value: .string("QuickTimeで開く")]
    backend.attributes[216] = [.role: .string("AXButton"), .title: .string("次へ…")]
    backend.attributes[217] = [.role: .string("AXButton"), .title: .string("キャンセル")]
  }

  static let popupMenus: [UIHandle: (menu: UIHandle, items: [(UIHandle, String)])] = [
    207: (208, [(209, "ビデオとオーディオ"), (230, "ビデオのみ")]),
    211: (212, [(213, "H.264"), (231, "Apple ProRes 4444")]),
    215: (219, [(220, "保存のみ")]),
  ]

  func pressedDialog(_ handle: UIHandle) {
    if let popup = Self.popupMenus[handle], popupMenuOpens {
      backend.attributes[popup.menu] = [
        .role: .string("AXMenu"), .children: .elements(popup.items.map(\.0)),
      ]
      for (item, title) in popup.items { backend.attributes[item] = [.title: .string(title)] }
      backend.attributes[handle]?[.children] = .elements([popup.menu])
      return
    }
    for (popup, entry) in Self.popupMenus {
      if let (_, title) = entry.items.first(where: { $0.0 == handle }) {
        backend.attributes[popup]?[.value] = .string(title)
        backend.attributes[popup]?[.children] = .elements([])
        return
      }
    }
    switch handle {
    case 216 where nextOpensSheet: openSheet()
    case 217:
      dialogCancelled = true
      backend.attributes[1]?[.windows] = .elements([5, 100])
    case 320 where disclosureShowsBrowser:
      backend.attributes[301]?[.children] = .elements([302, 305, 320, 311, 312, 313, 314, 315])
      renderBrowser()
    case 314: backend.attributes[200]?[.children] = .elements([201, 216, 217])
    case 315:
      savedName = Self.text(backend.attributes[312]?[.value])
      if saveCloses { backend.attributes[1]?[.windows] = .elements([5, 100]) }
    default: break
    }
  }

  static func text(_ value: UIAttributeValue?) -> String? {
    guard case .string(let text)? = value else { return nil }
    return text
  }

  func openSheet() {
    backend.attributes[200]?[.children] = .elements([201, 202, 203, 204, 218, 216, 217, 300])
    backend.attributes[300] = [.role: .string("AXSheet"), .children: .elements([301])]
    backend.attributes[301] = [
      .role: .string("AXSplitGroup"), .children: .elements([320, 311, 312, 313, 314, 315]),
    ]
    backend.attributes[320] = [.role: .string("AXDisclosureTriangle")]
    backend.attributes[302] = [.role: .string("AXOutline"), .children: .elements([303, 304, 306])]
    backend.attributes[303] = [.role: .string("AXRow"), .description: .string("デスクトップ")]
    backend.attributes[304] = [
      .role: .string("AXRow"), .description: .string(homeNameOverride ?? homeName),
      .selected: .bool(browserPath != nil),
    ]
    backend.attributes[306] = [.role: .string("AXColumn")]
    backend.attributes[305] = [.role: .string("AXBrowser")]
    backend.attributes[311] = [.role: .string("AXPopUpButton"), .value: .string("デスクトップ")]
    backend.attributes[312] = [.role: .string("AXTextField"), .value: .string("Live60.mov")]
    backend.attributes[313] = [.role: .string("AXTextField"), .subrole: .string("AXSearchField")]
    backend.attributes[314] = [.role: .string("AXButton"), .title: .string("キャンセル")]
    backend.attributes[315] = [.role: .string("AXButton"), .title: .string("保存")]
  }

  func listHandle(_ level: Int) -> UIHandle { 401 + level * 10 }

  func renderBrowser() {
    guard let path = browserPath else {
      backend.attributes[305]?[.children] = .elements([])
      backend.attributes[311]?[.value] = .string("デスクトップ")
      return
    }
    let levels = Array(0...min(path.count, tree.count - 1))
    backend.attributes[305]?[.children] = .elements([399])
    backend.attributes[399] = [
      .role: .string("AXScrollArea"), .children: .elements(levels.map { 400 + $0 * 10 }),
    ]
    for level in levels {
      let names = tree[level]
      let items = names.indices.map { listHandle(level) + 1 + $0 * 2 }
      backend.attributes[400 + level * 10] = [
        .role: .string("AXScrollArea"), .children: .elements([listHandle(level)]),
      ]
      let selected =
        level < path.count
        ? items.filter { item in
          names[(item - listHandle(level) - 1) / 2] == path[level]
        } : []
      backend.attributes[listHandle(level)] = [
        .role: .string("AXList"), .children: .elements(items),
        .selectedChildren: .elements(selected),
      ]
      for (offset, item) in items.enumerated() {
        backend.attributes[item] = [.role: .string("AXGroup"), .children: .elements([item + 1])]
        backend.attributes[item + 1] = [
          .role: .string("AXTextField"), .value: .string(names[offset]),
        ]
      }
    }
    backend.attributes[311]?[.value] = .string(path.last ?? homeName)
  }

  func selectInColumn(list: UIHandle, items: [UIHandle]) {
    guard let path = browserPath, (list - 401) % 10 == 0 else { return }
    let level = (list - 401) / 10
    let names = items.compactMap { item -> String? in
      Self.text(backend.attributes[item + 1]?[.value])
    }
    browserPath = Array(path.prefix(level)) + names
    renderBrowser()
  }
}

@MainActor
struct FinalCutAutomationTests {
  let target = UITarget(app: .finalCutPro)

  func run(_ world: FakeFinalCut, _ request: FCPRequest, home: URL? = nil, tables: URL? = nil)
    async throws -> FCPResult
  {
    let automation = UIAutomation(
      backend: world.backend, home: home ?? URL(fileURLWithPath: "/nonexistent-home"),
      folderLocalizations: tables ?? URL(fileURLWithPath: "/nonexistent-tables"))
    let response = try await automation.run(.finalCut(target, request))
    return try #require(response.finalCut)
  }

  @Test func readsTheTimelinePageAndProjectState() async throws {
    let world = FakeFinalCut(homeName: "user")
    let result = try await run(world, .timeline(offset: 1, limit: 5))
    #expect(result.language == "ja")
    #expect(result.vocabularyVerified)
    #expect(
      result.project
        == FCPProjectState(name: "Live60", duration: "00:00:30:00", playhead: "00:00:00:00"))
    #expect(result.clipCount == 2)
    #expect(
      result.clips == [
        FCPClip(
          index: 1, description: "AVクリップ:clip", start: "00:00:10:00", duration: "00:00:20:00",
          selected: false)
      ])
  }

  @Test(arguments: [(-1, 5), (0, 0), (0, 1001)])
  func rejectsInvalidPages(_ offset: Int, _ limit: Int) async throws {
    let world = FakeFinalCut(homeName: "user")
    await #expect(throws: ProAppsError.self) {
      try await run(world, .timeline(offset: offset, limit: limit))
    }
  }

  @Test func reportsAMissingTimelineAndUnsupportedLanguage() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.backend.attributes[100]?[.children] = .elements([110])
    await #expect(
      throws: ProAppsError.unavailable("No project is open in the Final Cut Pro timeline")
    ) {
      try await run(world, .timeline(offset: 0, limit: 1))
    }
    world.backend.attributes[world.item("ファイル>共有>ファイルを書き出す（デフォルト）…")] = [:]
    let fileMenu = world.backend.attributes[30]?[.children]
    world.backend.attributes[30]?[.children] = .elements([])
    await #expect(throws: ProAppsError.unavailable("Unsupported Final Cut Pro UI language")) {
      try await run(world, .timeline(offset: 0, limit: 1))
    }
    world.backend.attributes[30]?[.children] = fileMenu
  }

  @Test func requiresOneMainWindow() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.backend.attributes[1]?[.windows] = .elements([5])
    await #expect(throws: ProAppsError.unavailable("Final Cut Pro has no unique main window")) {
      try await run(world, .timeline(offset: 0, limit: 1))
    }
    world.backend.attributes[101] = [
      .role: .string("AXWindow"), .subrole: .string("AXStandardWindow"),
    ]
    world.backend.attributes[1]?[.windows] = .elements([101, 100])
    let result = try await run(world, .timeline(offset: 0, limit: 1))
    #expect(result.clipCount == 2)
  }

  @Test func selectsClipsAndReadsTheSelectionBack() async throws {
    let world = FakeFinalCut(homeName: "user")
    let result = try await run(
      world, .select([FCPClipReference(index: 1, description: "AVクリップ:clip")]))
    #expect(result.clips?.map(\.index) == [1])
    #expect(result.clips?.first?.selected == true)
    #expect(result.changed == true)
  }

  @Test(arguments: [
    [FCPClipReference](),
    [
      FCPClipReference(index: 0, description: "AVクリップ:clip"),
      FCPClipReference(index: 0, description: "AVクリップ:clip"),
    ],
    [FCPClipReference(index: 7, description: "AVクリップ:clip")],
    [FCPClipReference(index: 0, description: "moved")],
  ])
  func rejectsStaleOrInvalidSelections(_ references: [FCPClipReference]) async throws {
    let world = FakeFinalCut(homeName: "user")
    await #expect(throws: ProAppsError.self) { try await run(world, .select(references)) }
  }

  @Test func reportsASelectionThatWasNotApplied() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.selectionApplies = false
    await #expect(
      throws: ProAppsError.unavailable("Final Cut Pro did not apply the requested selection")
    ) {
      try await run(world, .select([FCPClipReference(index: 0, description: "AVクリップ:clip")]))
    }
  }

  @Test func opensAProjectFromALibraryEvent() async throws {
    let world = FakeFinalCut(homeName: "user")
    let result = try await run(world, .openProject(library: "Lib", event: "Ev", project: "Live90"))
    #expect(result.project.name == "Live90")
    #expect(result.changed == true)
    #expect(world.backend.attributes[130]?[.selectedRows] == .elements([132]))
    #expect(world.backend.attributes[140]?[.selectedChildren] == .elements([141]))
    #expect(world.backend.attributes[140]?[.focused] == .bool(true))
  }

  /// Expect a failure whose reason starts with `prefix` (foreground operations
  /// append their cleanup outcome to the reason).
  func expectFailure(
    _ prefix: String, _ world: FakeFinalCut, _ request: FCPRequest
  ) async {
    do {
      _ = try await run(world, request)
      Issue.record("expected failure: \(prefix)")
    } catch ProAppsError.invalid(let reason), ProAppsError.unavailable(let reason) {
      #expect(reason.hasPrefix(prefix), "\(reason)")
    } catch {
      Issue.record(error)
    }
  }

  @Test func rejectsAmbiguousEventsAndMissingProjects() async throws {
    let world = FakeFinalCut(homeName: "user")
    await expectFailure(
      "Event Ev matched 2 library rows; name the library", world,
      .openProject(library: nil, event: "Ev", project: "Live90"))
    await expectFailure(
      "The project is not shown in the browser filmstrip", world,
      .openProject(library: "Other", event: "Ev", project: "Absent"))
    world.backend.attributes[140]?[.children] = .elements([141, 141])
    await expectFailure(
      "More than one browser item is named Live90", world,
      .openProject(library: "Lib", event: "Ev", project: "Live90"))
    #expect(world.backend.attributes[2]?[.frontmost] == .bool(true))
  }

  @Test func reportsAProjectThatDidNotOpen() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.openClipWorks = false
    await expectFailure(
      "Final Cut Pro did not open the project in the timeline", world,
      .openProject(library: "Lib", event: "Ev", project: "Live90"))
    world.backend.attributes[world.item("クリップ>クリップを開く")]?[.enabled] = .bool(false)
    await #expect(throws: ProAppsError.self) {
      try await run(world, .openProject(library: "Lib", event: "Ev", project: "Live90"))
    }
  }

  @Test func navigatesBackWhenOpenClipOpensATimelineClip() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.openClipWorks = false
    world.openClipOpensClip = true
    await expectFailure(
      "Open Clip opened a clip of the shown project instead; navigated back", world,
      .openProject(library: "Lib", event: "Ev", project: "Live90"))
    #expect(world.openedClip == nil)
    #expect(world.backend.activated == [FakeFinalCut.finalCutPID, FakeFinalCut.previousPID])
  }

  @Test func guardsRequestsByTheOpenProject() async throws {
    let world = FakeFinalCut(homeName: "user")
    let read = try await run(world, .inProject("Live60", .timeline(offset: 0, limit: 1)))
    #expect(read.clipCount == 2)
    await expectFailure(
      "The timeline shows Live60, not Other; nothing was changed", world,
      .inProject("Other", .edit(.bladeAll)))
    await expectFailure(
      "Nested project guards", world, .inProject("Live60", .inProject("Live60", .edit(.bladeAll))))
    world.backend.attributes[100]?[.children] = .elements([120])
    await expectFailure(
      "The timeline shows no project, not Live60", world, .inProject("Live60", .edit(.bladeAll)))
  }

  @Test func waitsWhileFinalCutIsBusy() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.backend.namedFailures["30:AXChildren"] = .accessibility(-25204)
    world.backend.onPause = { backend in backend.namedFailures = [:] }
    let result = try await run(world, .timeline(offset: 0, limit: 1))
    #expect(result.clipCount == 2)
    world.backend.namedFailures["30:AXChildren"] = .accessibility(-25204)
    world.backend.onPause = { _ in }
    await #expect(throws: ProAppsError.self) {
      try await run(world, .timeline(offset: 0, limit: 1))
    }
  }

  @Test func movesThePlayheadAndStopsAtTheEnd() async throws {
    let world = FakeFinalCut(homeName: "user")
    let forward = try await run(world, .move(.nextFrame, count: 3))
    #expect(forward.steps == 3)
    #expect(forward.project.playhead == "00:00:00:03")
    let end = try await run(world, .move(.end, count: 1))
    #expect(end.project.playhead == "00:00:30:00")
    let stuck = try await run(world, .move(.nextFrame, count: 5))
    #expect(stuck.steps == 0)
  }

  @Test(arguments: [(FCPPlayheadMove.start, 2), (.nextFrame, 0), (.nextFrame, 601)])
  func rejectsInvalidMoves(_ move: FCPPlayheadMove, _ count: Int) async throws {
    let world = FakeFinalCut(homeName: "user")
    await #expect(throws: ProAppsError.self) { try await run(world, .move(move, count: count)) }
  }

  @Test func seeksByEditPointsThenFrames() async throws {
    let world = FakeFinalCut(homeName: "user")
    let ahead = try await run(world, .seek(timecode: "00:00:12:05", maximumSteps: 500))
    #expect(ahead.project.playhead == "00:00:12:05")
    #expect(ahead.steps == 1 + 1 + 1 + 65)
    let back = try await run(world, .seek(timecode: "00:00:10:00", maximumSteps: 500))
    #expect(back.project.playhead == "00:00:10:00")
    #expect(back.steps == 2)
    let exact = try await run(world, .seek(timecode: "00:00:10:00", maximumSteps: 5))
    #expect(exact.steps == 0)
  }

  @Test func seekFailsWithTheReachedPosition() async throws {
    let world = FakeFinalCut(homeName: "user")
    await #expect(throws: ProAppsError.self) {
      try await run(world, .seek(timecode: "00:00:20:00", maximumSteps: 3))
    }
    #expect(world.playhead == 300)
    world.playheadFrozen = true
    await #expect(throws: ProAppsError.self) {
      try await run(world, .seek(timecode: "00:00:00:00", maximumSteps: 3))
    }
  }

  @Test(arguments: [("1:2:3", 10), ("00:00:00:00", 0), ("00:00:00:00", 216_001)])
  func rejectsInvalidSeeks(_ timecode: String, _ steps: Int) async throws {
    let world = FakeFinalCut(homeName: "user")
    await #expect(throws: ProAppsError.self) {
      try await run(world, .seek(timecode: timecode, maximumSteps: steps))
    }
  }

  @Test func requiresAReadablePlayhead() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.backend.attributes[129]?[.value] = .string("--")
    await #expect(throws: ProAppsError.unavailable("The playhead timecode is not readable")) {
      try await run(world, .seek(timecode: "00:00:01:00", maximumSteps: 5))
    }
    world.backend.attributes[120]?[.children] = .elements([150])
    await #expect(throws: ProAppsError.unavailable("The timeline exposes no playhead")) {
      try await run(world, .move(.start, count: 1))
    }
  }

  @Test func bladesAndDeletesWithChangeEvidence() async throws {
    let world = FakeFinalCut(homeName: "user")
    world.playhead = 450
    world.refresh()
    let blade = try await run(world, .edit(.bladeAll))
    #expect(blade.changed == true)
    #expect(blade.clipCount == 3)
    _ = try await run(world, .select([FCPClipReference(index: 1, description: "AVクリップ:clip")]))
    let delete = try await run(world, .edit(.delete))
    #expect(delete.changed == true)
    #expect(delete.clipCount == 2)
    #expect(delete.project.duration == "00:00:25:00")
    let deselect = try await run(world, .edit(.deselectAll))
    #expect(deselect.changed == nil)
  }

  @Test func reportsAnUnchangedTimelineAndDisabledCommands() async throws {
    let world = FakeFinalCut(homeName: "user")
    let blade = try await run(world, .edit(.bladeAll))
    #expect(blade.changed == false)
    world.backend.attributes[world.item("編集>削除")]?[.enabled] = .bool(false)
    await #expect(
      throws: ProAppsError.unavailable(
        "Final Cut Pro disabled 編集 > 削除; check the selection and focused panel")
    ) {
      try await run(world, .edit(.delete))
    }
  }

  // MARK: Export

  struct ExportHome {
    let home: URL
    let tables: URL
    let output: URL
  }

  /// home/Movies(.localized)/out plus a ja table mapping Movies → ムービー.
  func exportHome() throws -> ExportHome {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "fcp-export-\(UUID().uuidString)")
    let home = root.appendingPathComponent("user")
    let output = home.appendingPathComponent("Movies/out")
    try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
    try Data().write(to: home.appendingPathComponent("Movies/.localized"))
    let tables = root.appendingPathComponent("tables")
    let lproj = tables.appendingPathComponent("ja.lproj")
    try FileManager.default.createDirectory(at: lproj, withIntermediateDirectories: true)
    let encoder = PropertyListEncoder()
    encoder.outputFormat = .binary
    try encoder.encode(["Movies": "ムービー"]).write(
      to: lproj.appendingPathComponent("SystemFolderLocalizations.strings"))
    return ExportHome(
      home: home.resolvingSymlinksInPath(), tables: tables,
      output: output.resolvingSymlinksInPath())
  }

  func exportRequest(_ paths: ExportHome, project: String = "Live60", codec: String? = "H.264")
    -> FCPRequest
  {
    .export(
      FCPExportRequest(
        project: project, directory: paths.output.path, fileName: "Live60-h264.mov",
        format: .videoAndAudio, codec: codec, allowForeground: true))
  }

  @Test func exportsThroughTheSharePanelAndRestoresFocus() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    let result = try await run(world, exportRequest(paths), home: paths.home, tables: paths.tables)
    let export = try #require(result.export)
    #expect(export.outputPath == paths.output.appendingPathComponent("Live60-h264.mov").path)
    #expect(export.dialogInfo == ["継続時間": "00:00:30:00"])
    #expect(export.format == "ビデオとオーディオ")
    #expect(export.codec == "H.264")
    #expect(export.action == "保存のみ")
    #expect(export.foregroundRestored)
    #expect(export.completionVerified == false)
    #expect(world.savedName == "Live60-h264.mov")
    #expect(world.browserPath == ["ムービー", "out"])
    #expect(world.backend.activated == [FakeFinalCut.finalCutPID, FakeFinalCut.previousPID])
    #expect(world.backend.attributes[120]?[.focused] == .bool(true))
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func exportReusesSelectedColumnsAndClearsDeeperSelections() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    world.tree = [["ムービー"], ["out"], ["deeper"], []]
    world.browserPath = ["ムービー", "out", "deeper"]
    let result = try await run(
      world, exportRequest(paths, codec: nil), home: paths.home, tables: paths.tables)
    #expect(result.export?.codec == "Apple ProRes 4444")
    #expect(world.browserPath == ["ムービー", "out"])
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func exportsDirectlyIntoTheHomeFolder() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    let request = FCPRequest.export(
      FCPExportRequest(
        project: "Live60", directory: paths.home.path, fileName: "home.mov",
        allowForeground: true))
    let result = try await run(world, request, home: paths.home, tables: paths.tables)
    #expect(result.export?.outputPath == paths.home.appendingPathComponent("home.mov").path)
    #expect(world.browserPath == [])
    #expect(world.savedName == "home.mov")
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func exportRequiresTheOpenProject() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    await #expect(throws: ProAppsError.self) {
      try await run(
        world, exportRequest(paths, project: "Other"), home: paths.home, tables: paths.tables)
    }
    #expect(world.backend.activated.isEmpty)
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func exportRestoresFocusWhenActivationFails() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    world.backend.activateResult = false
    await #expect(
      throws: ProAppsError.unavailable(
        "Final Cut Pro did not become active (previous app restored: false)")
    ) {
      try await run(world, exportRequest(paths), home: paths.home, tables: paths.tables)
    }
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  enum ExportFailure: String, CaseIterable {
    case shareDisabled, dialogMissing, differentProject, popupMissing, menuClosed
    case duplicateCodec, sheetMissing, browserMissing, homeMissing, folderMissing
    case nameRejected, saveStaysOpen

    var reason: String {
      switch self {
      case .shareDisabled: return "Final Cut Pro kept Share disabled while active"
      case .dialogMissing: return "The share dialog did not open"
      case .differentProject: return "The share dialog names a different item"
      case .popupMissing: return "The share settings have no ビデオコーデック: control"
      case .menuClosed: return "The フォーマット: menu did not open"
      case .duplicateCodec: return "ビデオコーデック: offers 2 items titled H.264"
      case .sheetMissing: return "The Save panel did not open"
      case .browserMissing: return "The Save panel did not show its folder browser"
      case .homeMissing: return "The Save panel sidebar has no unique home item"
      case .folderMissing: return "The Save panel shows 0 folders named ムービー"
      case .nameRejected: return "The Save panel did not accept the file name"
      case .saveStaysOpen: return "The Save panel stayed open; a confirmation may be waiting"
      }
    }
  }

  /// Arrange one failure in the share flow.
  func arrange(_ failure: ExportFailure, in world: FakeFinalCut) {
    let share = world.item("ファイル>共有>ファイルを書き出す（デフォルト）…")
    let pressed = world.backend.onPerform
    switch failure {
    case .shareDisabled: world.shareEnabledWhenActive = false
    case .dialogMissing:
      world.backend.onPerform = { backend, action, handle in
        if handle != share { pressed(backend, action, handle) }
      }
    case .differentProject:
      world.backend.onPerform = { backend, action, handle in
        pressed(backend, action, handle)
        if handle == share { backend.attributes[201]?[.value] = .string("Other") }
      }
    case .popupMissing:
      world.backend.onPerform = { backend, action, handle in
        pressed(backend, action, handle)
        if handle == share {
          backend.attributes[203]?[.children] = .elements([205, 206, 207, 214, 215])
        }
      }
    case .menuClosed: world.popupMenuOpens = false
    case .duplicateCodec:
      world.backend.onPerform = { backend, action, handle in
        pressed(backend, action, handle)
        if handle == 211 { backend.attributes[231]?[.title] = .string("H.264") }
      }
    case .sheetMissing: world.nextOpensSheet = false
    case .browserMissing: world.disclosureShowsBrowser = false
    case .homeMissing: world.homeNameOverride = "someone"
    case .folderMissing: world.tree = [["Other"], [], []]
    case .nameRejected: world.nameFieldAccepts = false
    case .saveStaysOpen: world.saveCloses = false
    }
  }

  /// Each failure closes the share UI and restores the previous app.
  @Test(arguments: ExportFailure.allCases)
  func exportFailuresCleanUp(_ failure: ExportFailure) async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    arrange(failure, in: world)
    do {
      _ = try await run(world, exportRequest(paths), home: paths.home, tables: paths.tables)
      Issue.record("export unexpectedly succeeded")
    } catch ProAppsError.invalid(let reason), ProAppsError.unavailable(let reason) {
      #expect(reason.hasPrefix(failure.reason))
      #expect(reason.contains("previous app restored: true"))
    }
    #expect(world.backend.activated.last == FakeFinalCut.previousPID)
    #expect(world.backend.attributes[2]?[.frontmost] == .bool(true))
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func exportCleanupReportsItsOwnFailure() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    world.nextOpensSheet = false
    let pressed = world.backend.onPerform
    world.backend.onPerform = { backend, action, handle in
      pressed(backend, action, handle)
      backend.attributes[217]?[.title] = .string("gone")
    }
    do {
      _ = try await run(world, exportRequest(paths), home: paths.home, tables: paths.tables)
      Issue.record("export unexpectedly succeeded")
    } catch ProAppsError.unavailable(let reason) {
      #expect(reason.contains("closing the export UI failed"))
      #expect(reason.contains("previous app restored: true"))
    }
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func cancellationStillClosesTheExportUIAndRestoresFocus() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    world.nextOpensSheet = false
    let automation = UIAutomation(
      backend: world.backend, home: paths.home, folderLocalizations: paths.tables)
    let task = Task { @MainActor in
      try await automation.run(.finalCut(target, exportRequest(paths)))
    }
    world.backend.onPause = { _ in task.cancel() }
    await #expect(throws: CancellationError.self) { try await task.value }
    #expect(world.dialogCancelled)
    #expect(world.backend.activated.last == FakeFinalCut.previousPID)
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func restoreIsANoOpWhenFinalCutWasFrontmost() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    world.backend.frontmostPID = FakeFinalCut.finalCutPID
    let result = try await run(world, exportRequest(paths), home: paths.home, tables: paths.tables)
    #expect(result.export?.foregroundRestored == true)
    #expect(world.backend.activated == [FakeFinalCut.finalCutPID])
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }

  @Test func restoreReportsARefusedActivation() async throws {
    let paths = try exportHome()
    let world = FakeFinalCut(homeName: "user")
    let activated = world.backend.onActivate
    world.backend.onActivate = { backend, pid in
      activated(backend, pid)
      backend.activateResult = pid != FakeFinalCut.previousPID
    }
    let result = try await run(world, exportRequest(paths), home: paths.home, tables: paths.tables)
    #expect(result.export?.foregroundRestored == false)
    try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent())
  }
}
