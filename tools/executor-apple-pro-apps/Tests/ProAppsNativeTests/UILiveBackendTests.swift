import ApplicationServices
import Foundation
import Testing

@testable import ProAppsCore

/// Exercises the real Accessibility / TIS / ScreenCaptureKit boundary against the
/// synthetic accessory fixture only. No Apple production app is launched and the
/// keyboard input source is only re-selected to its current value (a no-op).
@MainActor
struct UILiveBackendTests {
  struct Fixture {
    let backend: LiveUIBackend
    let bundleID: String
    let bundle: URL
    let pid: Int32
    let app: UIHandle
    let window: UIHandle
  }

  /// Minimal accessory (LSUIElement) bundle metadata for the synthetic fixture.
  struct BundleInfo: Encodable {
    let identifier: String
    let executable = "UIFixture"
    let packageType = "APPL"
    let accessory = true

    enum CodingKeys: String, CodingKey {
      case identifier = "CFBundleIdentifier"
      case executable = "CFBundleExecutable"
      case packageType = "CFBundlePackageType"
      case accessory = "LSUIElement"
    }
  }

  static let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    .deletingLastPathComponent().deletingLastPathComponent()

  func launchFixture() async throws -> Fixture {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ui-fixture-\(UUID().uuidString)")
    let bundle = root.appendingPathComponent("UIFixture.app")
    let macOS = bundle.appendingPathComponent("Contents/MacOS")
    try FileManager.default.createDirectory(at: macOS, withIntermediateDirectories: true)
    try FileManager.default.copyItem(
      at: Self.package.appendingPathComponent(".build/debug/apple-pro-apps-ui-fixture"),
      to: macOS.appendingPathComponent("UIFixture"))
    let bundleID = "dev.apple-pro-apps.ui-fixture.\(UUID().uuidString.lowercased())"
    let encoder = PropertyListEncoder()
    encoder.outputFormat = .xml
    try encoder.encode(BundleInfo(identifier: bundleID))
      .write(to: bundle.appendingPathComponent("Contents/Info.plist"))
    let backend = LiveUIBackend()
    let pid = try await backend.launch(applicationAt: bundle, activate: false)
    let app = backend.applicationElement(pid: pid)
    var window: UIHandle?
    for _ in 0..<50 where window == nil {
      do {
        if case .elements(let windows) = try backend.attribute(.windows, of: app) {
          window = try windows.first {
            try backend.attribute(.title, of: $0) == .string("UI Fixture")
          }
        }
      } catch UIBackendError.accessibility(AXError.cannotComplete.rawValue) {
        // The fixture is still launching and cannot answer Accessibility yet.
      }
      if window == nil { try await backend.pause(seconds: 0.1) }
    }
    return Fixture(
      backend: backend, bundleID: bundleID, bundle: root, pid: pid, app: app,
      window: try #require(window))
  }

  func stop(_ fixture: Fixture, force: Bool = false) async throws {
    #expect(fixture.backend.terminate(pid: fixture.pid, force: force))
    for _ in 0..<50 where fixture.backend.runningProcess(bundleID: fixture.bundleID) != nil {
      try await fixture.backend.pause(seconds: 0.1)
    }
    #expect(fixture.backend.runningProcess(bundleID: fixture.bundleID) == nil)
    try FileManager.default.removeItem(at: fixture.bundle)
  }

  func element(_ fixture: Fixture, identifier: String) throws -> UIHandle {
    var queue = [fixture.window]
    while !queue.isEmpty {
      let node = queue.removeFirst()
      if try fixture.backend.attribute(.identifier, of: node) == .string(identifier) {
        return node
      }
      if case .elements(let children) = try fixture.backend.attribute(.children, of: node) {
        queue += children
      }
    }
    throw UIBackendError.unavailable("fixture element missing: \(identifier)")
  }

  func waitForStatus(_ fixture: Fixture, _ expected: String) async throws -> Bool {
    let status = try element(fixture, identifier: "fixture-status")
    for _ in 0..<50 {
      if try fixture.backend.attribute(.value, of: status) == .string(expected) { return true }
      try await fixture.backend.pause(seconds: 0.1)
    }
    return false
  }

  @Test func readsTypedAttributesAndPerformsBackgroundActions() async throws {
    let fixture = try await launchFixture()
    let backend = fixture.backend
    #expect(backend.runningProcess(bundleID: fixture.bundleID)?.pid == fixture.pid)
    #expect(backend.runningProcess(bundleID: "dev.apple-pro-apps.absent") == nil)
    #expect(try backend.attribute(.role, of: fixture.window) == .string("AXWindow"))
    guard case .point = try backend.attribute(.position, of: fixture.window) else {
      Issue.record("expected a point")
      return
    }
    guard case .size = try backend.attribute(.size, of: fixture.window) else {
      Issue.record("expected a size")
      return
    }
    guard case .element = try backend.attribute(.menuBar, of: fixture.app) else {
      Issue.record("expected a menu bar element")
      return
    }
    let button = try element(fixture, identifier: "fixture-button")
    #expect(try backend.attribute(.enabled, of: button) == .bool(true))
    #expect(try backend.attribute(.selected, of: button) == .missing)
    #expect(try backend.actions(of: button).contains("AXPress"))
    #expect(try backend.isSettable(.value, of: button) == false)
    try backend.perform("AXPress", on: button)
    #expect(try await waitForStatus(fixture, "button-pressed"))
    #expect(try backend.attribute(.title, of: button) == .string("Pressed"))
    let checkbox = try element(fixture, identifier: "fixture-checkbox")
    #expect(try backend.attribute(.value, of: checkbox) == .number(0))
    let field = try element(fixture, identifier: "fixture-field")
    #expect(try backend.isSettable(.value, of: field))
    try backend.set(.value, to: .string("/Volumes/ascii/パス"), on: field)
    #expect(try backend.attribute(.value, of: field) == .string("/Volumes/ascii/パス"))
    #expect(throws: UIBackendError.self) { try backend.perform("AXPress", on: fixture.window) }
    #expect(throws: UIBackendError.self) {
      try backend.set(.selected, to: .bool(true), on: fixture.window)
    }
    #expect(throws: UIBackendError.unavailable("Unknown accessibility handle")) {
      try backend.attribute(.role, of: 9_999_999)
    }
    #expect(try backend.actions(of: fixture.app).isEmpty)
    try await stop(fixture)
  }

  @Test func selectsTableRowsAndMenuItemsWithoutActivation() async throws {
    let fixture = try await launchFixture()
    let backend = fixture.backend
    let table = try element(fixture, identifier: "fixture-table")
    guard case .elements(let rows) = try backend.attribute(.children, of: table),
      let row = rows.first
    else {
      Issue.record("expected table rows")
      return
    }
    #expect(try backend.isSettable(.selected, of: row))
    try backend.set(.selected, to: .bool(true), on: row)
    #expect(try await waitForStatus(fixture, "selected-alpha.fcpbundle"))
    // Element lists are written as one CFArray (Final Cut Pro's selection model).
    #expect(try backend.isSettable(.selectedRows, of: table))
    try backend.set(.selectedRows, to: .elements([rows[1]]), on: table)
    #expect(try await waitForStatus(fixture, "selected-beta.fcpbundle"))
    #expect(throws: UIBackendError.unavailable("Unknown accessibility handle")) {
      try backend.set(.selectedRows, to: .elements([-1]), on: table)
    }
    guard case .element(let bar) = try backend.attribute(.menuBar, of: fixture.app),
      case .elements(let barItems) = try backend.attribute(.children, of: bar)
    else {
      Issue.record("expected menu bar items")
      return
    }
    let file = try #require(
      try barItems.first { try backend.attribute(.title, of: $0) == .string("File") })
    guard case .elements(let fileMenus) = try backend.attribute(.children, of: file),
      let fileMenu = fileMenus.first,
      case .elements(let fileItems) = try backend.attribute(.children, of: fileMenu)
    else {
      Issue.record("expected File menu")
      return
    }
    let new = try #require(
      try fileItems.first { try backend.attribute(.title, of: $0) == .string("New") })
    guard case .elements(let newMenus) = try backend.attribute(.children, of: new),
      let newMenu = newMenus.first,
      case .elements(let newItems) = try backend.attribute(.children, of: newMenu)
    else {
      Issue.record("expected New submenu")
      return
    }
    let thing = try #require(
      try newItems.first { try backend.attribute(.title, of: $0) == .string("Thing") })
    try backend.perform("AXPress", on: thing)
    #expect(try await waitForStatus(fixture, "menu-selected"))
    try await stop(fixture, force: true)
  }

  @Test func capturesABackgroundWindowAsPNG() async throws {
    let fixture = try await launchFixture()
    let backend = fixture.backend
    guard case .point(let origin) = try backend.attribute(.position, of: fixture.window),
      case .size(let size) = try backend.attribute(.size, of: fixture.window)
    else {
      Issue.record("expected geometry")
      return
    }
    let image = try await backend.captureWindow(
      pid: fixture.pid, title: "UI Fixture",
      frame: UIFrame(x: origin.x, y: origin.y, width: size.width, height: size.height))
    #expect(image.width > 0 && image.height > 0)
    #expect(image.png.prefix(4) == Data([0x89, 0x50, 0x4E, 0x47]))
    await #expect(throws: UIBackendError.self) {
      try await backend.captureWindow(pid: 1, title: "absent", frame: nil)
    }
    try await stop(fixture)
  }

  @Test func readsAndReselectsTheCurrentInputSourceOnly() throws {
    let backend = LiveUIBackend()
    let current = try backend.currentInputSource()
    #expect(!current.id.isEmpty)
    #expect(try backend.asciiCapableInputSource().asciiCapable)
    #expect(try backend.inputSources().contains { $0.id == current.id })
    try backend.selectInputSource(id: current.id)
    #expect(try backend.currentInputSource().id == current.id)
    #expect(throws: UIBackendError.unavailable("Input source is not installed")) {
      try backend.selectInputSource(id: "dev.apple-pro-apps.absent-input-source")
    }
  }

  @Test func reportsProcessClockAndLaunchServicesState() async throws {
    let backend = LiveUIBackend()
    #expect(backend.applicationURL(bundleID: "dev.apple-pro-apps.absent") == nil)
    #expect(backend.terminate(pid: -1, force: false) == false)
    // Activation is only exercised on a process that cannot exist, so tests never
    // move the user's focus; the real flow is covered by Final Cut Pro acceptance.
    #expect(backend.activate(pid: -1) == false)
    _ = backend.frontmostProcessID()
    _ = backend.frontmostBundleID()
    let start = backend.monotonicSeconds()
    try await backend.pause(seconds: 0.01)
    #expect(backend.monotonicSeconds() >= start)
  }

  @Test func convertsBoundaryValuesConservatively() {
    let backend = LiveUIBackend()
    #expect(backend.convert("text" as CFString) == .string("text"))
    #expect(backend.convert(kCFBooleanTrue) == .bool(true))
    #expect(backend.convert(NSNumber(value: 2.5)) == .number(2.5))
    #expect(backend.convert(Data([1]) as CFData) == .unsupported)
    #expect(backend.convert(["mixed"] as CFArray) == .unsupported)
    #expect(backend.convert([] as CFArray) == .elements([]))
    var range = CFRange(location: 1, length: 2)
    let rangeValue = AXValueCreate(.cfRange, &range)
    #expect(rangeValue.map(LiveUIBackend.geometry) == .unsupported)
    #expect(LiveUIBackend.error(.apiDisabled) == .notPermitted)
    #expect(LiveUIBackend.error(.cannotComplete) == .accessibility(AXError.cannotComplete.rawValue))
  }

  @Test func matchesCaptureWindowsWithoutGuessing() {
    let a = LiveUIBackend.WindowCandidate(
      title: "Main", frame: CGRect(x: 0, y: 0, width: 10, height: 10))
    let b = LiveUIBackend.WindowCandidate(
      title: "Main", frame: CGRect(x: 50, y: 0, width: 10, height: 10))
    let c = LiveUIBackend.WindowCandidate(
      title: "Other", frame: CGRect(x: 50, y: 0, width: 10, height: 10))
    let frame = UIFrame(x: 50, y: 1, width: 10, height: 10)
    #expect(LiveUIBackend.matchWindow([a, b, c], title: "Main", frame: frame) == 1)
    #expect(LiveUIBackend.matchWindow([a, b], title: nil, frame: frame) == 1)
    #expect(LiveUIBackend.matchWindow([a, c], title: "Other", frame: nil) == 1)
    #expect(LiveUIBackend.matchWindow([a, b], title: "Main", frame: nil) == nil)
    #expect(LiveUIBackend.matchWindow([b, c], title: nil, frame: frame) == nil)
  }
}
