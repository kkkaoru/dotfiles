import Foundation
import MCP
import ProAppsCore
import Synchronization
import Testing

@testable import AppleProApps

struct UIToolsTests {
  static let target = UITarget(app: .finalCutPro)
  static let window = UIWindowLocator(title: "ライブラリを開く")
  static let row = UIElementLocator(role: "AXRow", containsText: "alpha.fcpbundle")
  static let base: [String: Value] = ["app": .string("finalCutPro")]
  static let windowValue: Value = .object(["title": .string("ライブラリを開く")])
  static let rowValue: Value = .object([
    "role": .string("AXRow"), "containsText": .string("alpha.fcpbundle"),
  ])

  static func service(_ ui: @escaping @Sendable (UIRequest, Duration) async throws -> UIResponse)
    -> NativeService
  {
    var interfaces = NativeInterfaces()
    interfaces.ui = ui
    return NativeService(interfaces: interfaces)
  }

  static let response: UIResponse = {
    var response = UIResponse(
      focus: UIFocusEvidence(frontmostBefore: "dev.terminal", frontmostAfter: "dev.terminal"))
    response.dispatched = true
    return response
  }()

  @Test(arguments: [
    ("ui_windows", base, UIRequest.windows(target)),
    (
      "ui_inspect", base.merging(["window": windowValue]) { a, _ in a },
      .inspect(target, window: window, root: nil, maxDepth: 8, maxElements: 200)
    ),
    (
      "ui_inspect",
      base.merging([
        "window": windowValue, "root": rowValue, "maxDepth": .int(2), "maxElements": .int(5),
      ]) { a, _ in a },
      .inspect(target, window: window, root: row, maxDepth: 2, maxElements: 5)
    ),
    ("ui_menu_list", base, .menuList(target, path: [])),
    (
      "ui_menu_select", base.merging(["menuPath": .array([.string("ファイル")])]) { a, _ in a },
      .menuSelect(target, path: ["ファイル"])
    ),
    (
      "ui_perform",
      base.merging(["window": windowValue, "element": rowValue, "action": .string("press")]) {
        a, _ in a
      }, .perform(target, window: window, target: row, action: .press)
    ),
    (
      "ui_set_value",
      base.merging([
        "window": windowValue, "element": rowValue, "attribute": .string("selected"),
        "boolValue": .bool(true),
      ]) { a, _ in a },
      .set(target, window: window, target: row, attribute: .selected, value: .bool(true))
    ),
    (
      "ui_set_value",
      base.merging([
        "window": windowValue, "element": rowValue, "attribute": .string("value"),
        "stringValue": .string("/Volumes/x"),
      ]) { a, _ in a },
      .set(target, window: window, target: row, attribute: .value, value: .string("/Volumes/x"))
    ),
    (
      "ui_wait",
      base.merging([
        "window": windowValue, "condition": .string("absent"), "timeoutSeconds": .double(2),
      ]) { a, _ in a },
      .wait(target, window: window, target: nil, condition: .absent, timeoutSeconds: 2)
    ),
    (
      "ui_capture",
      base.merging(["window": windowValue, "outputPath": .string("/tmp/w.png")]) { a, _ in a },
      .capture(target, window: window, outputPath: "/tmp/w.png")
    ),
    ("input_source_status", [:], .inputSourceStatus),
    (
      "input_source_select", ["asciiCapable": .bool(true)],
      .inputSourceSelect(id: nil, asciiCapable: true)
    ),
    (
      "input_source_select", ["id": .string("abc")],
      .inputSourceSelect(id: "abc", asciiCapable: false)
    ),
    ("app_launch", base, .launch(target, activate: false)),
    ("app_quit", base, .quit(target, force: false)),
    (
      "app_quit",
      base.merging(["force": .bool(true), "discardUnsavedChanges": .bool(true)]) { a, _ in a },
      .quit(target, force: true)
    ),
  ])
  func mapsToolArgumentsToTypedRequests(
    _ name: String, _ arguments: [String: Value], _ expected: UIRequest
  ) async throws {
    let service = Self.service { _, _ in Self.response }
    #expect(try await service.uiRequest(name, .object(arguments)) == expected)
  }

  @Test(arguments: [
    ("ui_windows", [String: Value](), "Missing app"),
    ("ui_inspect", base, "Missing window"),
    (
      "ui_perform",
      base.merging(["window": windowValue, "action": .string("press")]) { a, _ in a },
      "Missing element"
    ),
    (
      "ui_perform", base.merging(["window": windowValue, "element": rowValue]) { a, _ in a },
      "Missing action"
    ),
    (
      "ui_set_value", base.merging(["window": windowValue, "element": rowValue]) { a, _ in a },
      "Missing attribute"
    ),
    (
      "ui_set_value",
      base.merging([
        "window": windowValue, "element": rowValue, "attribute": .string("selected"),
        "stringValue": .string("x"),
      ]) { a, _ in a }, "value needs stringValue; selected needs boolValue"
    ),
    (
      "ui_menu_select", base.merging(["menuPath": .array([])]) { a, _ in a },
      "menuPath must name at least one item"
    ),
    ("ui_menu_select", base, "menuPath must name at least one item"),
    ("ui_wait", base, "Missing condition or timeout"),
    ("ui_capture", base.merging(["window": windowValue]) { a, _ in a }, "Missing outputPath"),
    (
      "app_quit", base.merging(["force": .bool(true)]) { a, _ in a },
      "force requires discardUnsavedChanges: true"
    ),
    ("ui_unknown", base, "Unknown tool"),
  ])
  func rejectsIncompleteToolArguments(
    _ name: String, _ arguments: [String: Value], _ reason: String
  )
    async throws
  {
    let service = Self.service { _, _ in Self.response }
    await #expect(throws: ProAppsError.invalid(reason)) {
      try await service.uiRequest(name, .object(arguments))
    }
  }

  @Test func boundsChildTimeoutsByRequest() {
    #expect(
      NativeService.uiTimeout(
        .wait(Self.target, window: nil, target: nil, condition: .exists, timeoutSeconds: 2.2))
        == .seconds(18))
    #expect(NativeService.uiTimeout(.launch(Self.target, activate: false)) == .seconds(90))
    #expect(NativeService.uiTimeout(.inputSourceStatus) == .seconds(45))
  }

  @Test func routesUIToolsThroughTheChildInterface() async throws {
    let seen = Mutex<[UIRequest]>([])
    let service = Self.service { request, _ in
      seen.withLock { $0.append(request) }
      return Self.response
    }
    let result = await service.call(
      .init(
        name: "ui_set_value",
        arguments: Self.base.merging([
          "window": Self.windowValue, "element": Self.rowValue, "attribute": .string("selected"),
          "boolValue": .bool(true),
        ]) { a, _ in a }))
    #expect(result.isError == false)
    let fields = try #require(result.structuredContent?.objectValue)
    #expect(fields["retrySafe"] == .bool(false))
    #expect(fields["dispatched"] == .bool(true))
    #expect(fields["focus"]?.objectValue?["frontmostChanged"] == .bool(false))
    #expect(seen.withLock { $0.count } == 1)
  }

  @Test func reportsChildFailuresAndSchemaViolations() async throws {
    let failing = Self.service { _, _ in throw ProAppsError.unavailable("No matching window") }
    let failed = await failing.call(.init(name: "ui_windows", arguments: Self.base))
    #expect(failed.isError == true)
    let strict = Self.service { _, _ in Self.response }
    let unknownKey = await strict.call(
      .init(
        name: "ui_inspect",
        arguments: Self.base.merging(["window": .object(["bogus": .int(1)])]) { a, _ in a }))
    #expect(unknownKey.isError == true)
    let badAction = await strict.call(
      .init(
        name: "ui_perform",
        arguments: Self.base.merging([
          "window": Self.windowValue, "element": Self.rowValue, "action": .string("typeText"),
        ]) { a, _ in a }))
    #expect(badAction.isError == true)
  }

  @Test func advertisesBackgroundAccessibilityTools() async throws {
    #expect(ToolSpec.uiNames.contains("ui_capture"))
    #expect(ToolSpec.all.filter { ToolSpec.uiNames.contains($0.name) }.count == ToolSpec.ui.count)
    #expect(ToolSpec.ui.first { $0.name == "ui_inspect" }?.readOnly == true)
    #expect(ToolSpec.ui.first { $0.name == "ui_perform" }?.readOnly == false)
    var interfaces = NativeInterfaces()
    interfaces.inventory = { [] }
    let result = await NativeService(interfaces: interfaces).call(
      .init(name: "app_capabilities", arguments: [:]))
    #expect(result.structuredContent?.objectValue?["backgroundAccessibility"] == .bool(true))
  }

  @Test func defaultChildInterfaceFailsClosedOutsideTheExecutable() async throws {
    await #expect(throws: (any Error).self) {
      try await NativeInterfaces().ui(.inputSourceStatus, .seconds(10))
    }
  }

  @Test(arguments: [
    (["ui-native", #"{"operation":"inputSourceStatus"}"#], true),
    (["ui-native"], false),
    (["ui-native", "{}", "{}"], false),
    (["ui-native", String(repeating: "x", count: Command.maximumUIRequestBytes + 1)], false),
  ])
  func parsesBoundedChildRequests(_ arguments: [String], _ valid: Bool) throws {
    if valid {
      #expect(try Command.parse(arguments) == .uiNative(arguments[1]))
    } else {
      #expect(throws: ProAppsError.invalid("ui-native requires exactly one bounded JSON request")) {
        try Command.parse(arguments)
      }
    }
  }

  @Test func programRoutesChildRequestsToTheRuntime() async throws {
    var runtime = ProgramRuntime(
      binary: URL(fileURLWithPath: "/tmp/native"), environment: [:], serve: {},
      inventory: { "[]" }, ui: { _ in }, execute: { _, _, _ in "" })
    runtime.uiNative = { request in "handled:\(request)" }
    #expect(try await Program(runtime: runtime).run(.uiNative("{}")) == "handled:{}")
  }
}
