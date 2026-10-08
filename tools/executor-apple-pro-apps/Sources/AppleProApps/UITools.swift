import Foundation
import MCP
import ProAppsCore

extension ToolSpec {
  static let windowLocator = object(
    ["index": integer(0, 63), "title": string(maximum: 512), "subrole": string(maximum: 128)], [])
  static let elementLocator = object(
    [
      "path": array(integer(0, 4096), maximum: 32, minimum: 0),
      "role": string(maximum: 128), "subrole": string(maximum: 128),
      "identifier": string(maximum: 256), "title": string(maximum: 512),
      "description": string(maximum: 512), "value": string(maximum: 512),
      "containsText": string(maximum: 512), "index": integer(0, 999),
    ], [])
  static let menuPath = array(string(maximum: 256), maximum: 6, minimum: 0)
  static let uiTarget: [String: Value] = ["app": app, "bundleID": string()]

  /// Tool properties for one selected app edition plus tool-specific fields.
  static func targeted(_ fields: [String: Value]) -> [String: Value] {
    var properties = uiTarget
    for (key, value) in fields { properties[key] = value }
    return properties
  }

  static let backgroundNote =
    "Background Accessibility only: never activates the app or synthesizes keyboard/pointer input; reports frontmostChanged."

  static let ui: [ToolSpec] = [
    .init(
      name: "ui_windows",
      description:
        "List windows, dialogs and sheets of one running supported Pro app edition through Accessibility. \(backgroundNote)",
      properties: uiTarget, required: ["app"], readOnly: true),
    .init(
      name: "ui_inspect",
      description:
        "Read a bounded Accessibility subtree of one window (optionally from a located root). Returns child-index paths, roles, identifiers, titles, values, actions and frames. \(backgroundNote)",
      properties: targeted([
        "window": windowLocator, "root": elementLocator, "maxDepth": integer(1, 20),
        "maxElements": integer(1, 500),
      ]), required: ["app", "window"], readOnly: true),
    .init(
      name: "ui_menu_list",
      description:
        "List menu-bar items, or items of the submenu at an exact title path, without opening menus. \(backgroundNote)",
      properties: targeted(["menuPath": menuPath]),
      required: ["app"], readOnly: true),
    .init(
      name: "ui_menu_select",
      description:
        "Press one enabled menu item at an exact unique title path (for example File > New > Library). Effects such as dialogs are not verified; observe afterwards. \(backgroundNote)",
      properties: targeted(["menuPath": menuPath]),
      required: ["app", "menuPath"], readOnly: false),
    .init(
      name: "ui_perform",
      description:
        "Perform one typed Accessibility action on exactly one located element and read the element back. The locator must match uniquely unless index is given. \(backgroundNote)",
      properties: targeted([
        "window": windowLocator, "element": elementLocator,
        "action": string(UIAction.allCases.map(\.rawValue)),
      ]), required: ["app", "window", "element", "action"],
      readOnly: false),
    .init(
      name: "ui_set_value",
      description:
        "Set AXValue (text) or AXSelected (row/cell selection) on exactly one located element and verify by reading it back. Text is written directly, so keyboard layouts and input methods do not affect it. \(backgroundNote)",
      properties: targeted([
        "window": windowLocator, "element": elementLocator,
        "attribute": string(UISettableAttribute.allCases.map(\.rawValue)),
        "stringValue": string(maximum: 4096), "boolValue": boolean,
      ]), required: ["app", "window", "element", "attribute"],
      readOnly: false),
    .init(
      name: "ui_wait",
      description:
        "Poll until a window and/or element exists or is absent, up to 30 seconds. Use instead of sleeping in Executor code. \(backgroundNote)",
      properties: targeted([
        "window": windowLocator, "element": elementLocator,
        "condition": string(["exists", "absent"]), "timeoutSeconds": number,
      ]), required: ["app", "condition", "timeoutSeconds"],
      readOnly: true),
    .init(
      name: "ui_capture",
      description:
        "Capture one window of a supported Pro app (including background/occluded windows) with ScreenCaptureKit into a NEW private PNG. Requires Screen Recording for the Executor host. No activation or cursor.",
      properties: targeted(["window": windowLocator, "outputPath": string()]),
      required: ["app", "window", "outputPath"], readOnly: false),
    .init(
      name: "input_source_status",
      description:
        "Read the current keyboard input source ID and whether it is ASCII-capable. Synthetic typing under a Japanese input method can turn '/' into '・'.",
      properties: [:], required: [], readOnly: true),
    .init(
      name: "input_source_select",
      description:
        "Select an installed keyboard input source by exact ID, or the system ASCII-capable source. Global user state: returns previousInputSource; restore it with this tool when finished.",
      properties: ["id": string(maximum: 256), "asciiCapable": boolean], required: [],
      readOnly: false),
    .init(
      name: "app_launch",
      description:
        "Launch the exact supported app edition if it is not running. activate defaults to false (background launch); a no-op returns the existing PID. Launch can show license or first-run UI.",
      properties: targeted(["activate": boolean]),
      required: ["app"], readOnly: false),
    .init(
      name: "app_quit",
      description:
        "Request normal termination of the exact running app edition and wait up to 10 seconds. The app may refuse or ask about unsaved changes. force requires discardUnsavedChanges: true and loses unsaved work.",
      properties: targeted(["force": boolean, "discardUnsavedChanges": boolean]), required: ["app"],
      readOnly: false),
  ]

  static var uiNames: Set<String> { Set(ui.map(\.name)) }
}

extension NativeService {
  private struct UIInput: Decodable {
    let app: ProApp?
    let bundleID: String?
    let window: UIWindowLocator?
    let root: UIElementLocator?
    let element: UIElementLocator?
    let maxDepth: Int?
    let maxElements: Int?
    let menuPath: [String]?
    let action: UIAction?
    let attribute: UISettableAttribute?
    let stringValue: String?
    let boolValue: Bool?
    let condition: UIWaitCondition?
    let timeoutSeconds: Double?
    let outputPath: String?
    let id: String?
    let asciiCapable: Bool?
    let activate: Bool?
    let force: Bool?
    let discardUnsavedChanges: Bool?
  }

  static let defaultInspectDepth = 8
  static let defaultInspectElements = 200

  /// Translate validated MCP arguments into one typed child request.
  func uiRequest(_ name: String, _ arguments: Value) throws -> UIRequest {
    let input = try decode(UIInput.self, arguments)
    func target() throws -> UITarget {
      guard let app = input.app else { throw ProAppsError.invalid("Missing app") }
      return UITarget(app: app, bundleID: input.bundleID)
    }
    func window() throws -> UIWindowLocator {
      guard let window = input.window else { throw ProAppsError.invalid("Missing window") }
      return window
    }
    func element() throws -> UIElementLocator {
      guard let element = input.element else { throw ProAppsError.invalid("Missing element") }
      return element
    }
    switch name {
    case "ui_windows": return .windows(try target())
    case "ui_inspect":
      return .inspect(
        try target(), window: try window(), root: input.root,
        maxDepth: input.maxDepth ?? Self.defaultInspectDepth,
        maxElements: input.maxElements ?? Self.defaultInspectElements)
    case "ui_menu_list": return .menuList(try target(), path: input.menuPath ?? [])
    case "ui_menu_select":
      guard let path = input.menuPath, !path.isEmpty else {
        throw ProAppsError.invalid("menuPath must name at least one item")
      }
      return .menuSelect(try target(), path: path)
    case "ui_perform":
      guard let action = input.action else { throw ProAppsError.invalid("Missing action") }
      return .perform(try target(), window: try window(), target: try element(), action: action)
    case "ui_set_value":
      guard let attribute = input.attribute else { throw ProAppsError.invalid("Missing attribute") }
      let value: UISettableValue
      switch (attribute, input.stringValue, input.boolValue) {
      case (.value, .some(let text), .none): value = .string(text)
      case (.selected, .none, .some(let flag)): value = .bool(flag)
      default:
        throw ProAppsError.invalid("value needs stringValue; selected needs boolValue")
      }
      return .set(
        try target(), window: try window(), target: try element(), attribute: attribute,
        value: value)
    case "ui_wait":
      guard let condition = input.condition, let timeout = input.timeoutSeconds else {
        throw ProAppsError.invalid("Missing condition or timeout")
      }
      return .wait(
        try target(), window: input.window, target: input.element, condition: condition,
        timeoutSeconds: timeout)
    case "ui_capture":
      guard let output = input.outputPath else { throw ProAppsError.invalid("Missing outputPath") }
      return .capture(try target(), window: try window(), outputPath: output)
    case "input_source_status": return .inputSourceStatus
    case "input_source_select":
      return .inputSourceSelect(id: input.id, asciiCapable: input.asciiCapable ?? false)
    case "app_launch": return .launch(try target(), activate: input.activate ?? false)
    case "app_quit":
      let force = input.force ?? false
      guard !force || input.discardUnsavedChanges == true else {
        throw ProAppsError.invalid("force requires discardUnsavedChanges: true")
      }
      return .quit(try target(), force: force)
    default: throw ProAppsError.invalid("Unknown tool")
    }
  }

  static func uiTimeout(_ request: UIRequest) -> Duration {
    switch request {
    case .wait(_, _, _, _, let seconds): return .seconds(Int(seconds.rounded(.up)) + 15)
    case .launch: return .seconds(90)
    default: return .seconds(45)
    }
  }

  func uiTool(_ name: String, _ arguments: Value) async throws -> CallTool.Result {
    let request = try uiRequest(name, arguments)
    let response = try await interfaces.ui(request, Self.uiTimeout(request))
    guard var fields = try Value(response).objectValue else {
      throw ProAppsError.unavailable("UI result did not encode as an object")
    }
    fields["retrySafe"] = .bool(false)
    return self.response(fields)
  }
}
