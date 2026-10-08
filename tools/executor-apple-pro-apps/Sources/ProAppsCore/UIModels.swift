import Foundation

/// Accessibility attributes the native UI adapter reads or writes. Keeping the
/// vocabulary closed prevents arbitrary attribute probing of other processes.
public enum UIAttributeName: String, Codable, CaseIterable, Sendable {
  case role = "AXRole"
  case subrole = "AXSubrole"
  case identifier = "AXIdentifier"
  case title = "AXTitle"
  case description = "AXDescription"
  case value = "AXValue"
  case enabled = "AXEnabled"
  case selected = "AXSelected"
  case focused = "AXFocused"
  case main = "AXMain"
  case minimized = "AXMinimized"
  case modal = "AXModal"
  case children = "AXChildren"
  case windows = "AXWindows"
  case menuBar = "AXMenuBar"
  case position = "AXPosition"
  case size = "AXSize"
  case selectedChildren = "AXSelectedChildren"
  case selectedRows = "AXSelectedRows"
  case valueDescription = "AXValueDescription"
  case disclosureLevel = "AXDisclosureLevel"
  case frontmost = "AXFrontmost"
}

/// Accessibility actions exposed by the typed tools. The adapter never forwards
/// an arbitrary action name supplied by a caller.
public enum UIAction: String, Codable, CaseIterable, Sendable {
  case press, confirm, cancel, increment, decrement, showMenu, pick, raise

  public var axName: String {
    switch self {
    case .press: return "AXPress"
    case .confirm: return "AXConfirm"
    case .cancel: return "AXCancel"
    case .increment: return "AXIncrement"
    case .decrement: return "AXDecrement"
    case .showMenu: return "AXShowMenu"
    case .pick: return "AXPick"
    case .raise: return "AXRaise"
    }
  }
}

/// Settable attributes. Focus is excluded because it can move keyboard focus and
/// bring UI forward; selection/value changes stay background operations.
public enum UISettableAttribute: String, Codable, CaseIterable, Sendable {
  case value, selected

  public var attribute: UIAttributeName {
    switch self {
    case .value: return .value
    case .selected: return .selected
    }
  }
}

public struct UIPoint: Codable, Equatable, Sendable {
  public let x: Double
  public let y: Double
  public init(x: Double, y: Double) {
    self.x = x
    self.y = y
  }
}

public struct UISize: Codable, Equatable, Sendable {
  public let width: Double
  public let height: Double
  public init(width: Double, height: Double) {
    self.width = width
    self.height = height
  }
}

public struct UIFrame: Codable, Equatable, Sendable {
  public let x: Double
  public let y: Double
  public let width: Double
  public let height: Double
  public init(x: Double, y: Double, width: Double, height: Double) {
    self.x = x
    self.y = y
    self.width = width
    self.height = height
  }
}

/// Values written through accessibility. Strings are bounded by the tool schema;
/// element lists only contain handles issued by the same backend instance.
public enum UISettableValue: Equatable, Sendable {
  case string(String)
  case bool(Bool)
  case elements([UIHandle])
}

/// Type-checked values read at the accessibility boundary.
public enum UIAttributeValue: Equatable, Sendable {
  case missing
  case string(String)
  case bool(Bool)
  case number(Double)
  case point(UIPoint)
  case size(UISize)
  case element(UIHandle)
  case elements([UIHandle])
  case unsupported
}

/// Opaque per-process handle issued by a backend for one accessibility element.
public typealias UIHandle = Int

/// Selects one window (or a dialog/sheet window) of the target application.
public struct UIWindowLocator: Codable, Equatable, Sendable {
  public let index: Int?
  public let title: String?
  public let subrole: String?
  public init(index: Int? = nil, title: String? = nil, subrole: String? = nil) {
    self.index = index
    self.title = title
    self.subrole = subrole
  }
}

/// Selects one element inside a window. Supplied attributes must all match; a
/// `path` is a child-index path from the window and is still guarded by them.
public struct UIElementLocator: Codable, Equatable, Sendable {
  public let path: [Int]?
  public let role: String?
  public let subrole: String?
  public let identifier: String?
  public let title: String?
  public let description: String?
  public let value: String?
  public let containsText: String?
  public let index: Int?

  public init(
    path: [Int]? = nil, role: String? = nil, subrole: String? = nil, identifier: String? = nil,
    title: String? = nil, description: String? = nil, value: String? = nil,
    containsText: String? = nil, index: Int? = nil
  ) {
    self.path = path
    self.role = role
    self.subrole = subrole
    self.identifier = identifier
    self.title = title
    self.description = description
    self.value = value
    self.containsText = containsText
    self.index = index
  }

  var hasCriteria: Bool {
    path != nil || role != nil || subrole != nil || identifier != nil || title != nil
      || description != nil || value != nil || containsText != nil
  }
}

/// Exact selected app edition for a UI operation. Only the five supported apps'
/// documented bundle identifiers are accepted.
public struct UITarget: Codable, Equatable, Sendable {
  public let app: ProApp
  public let bundleID: String?
  public init(app: ProApp, bundleID: String? = nil) {
    self.app = app
    self.bundleID = bundleID
  }

  public func resolvedBundleID() throws -> String {
    let id = bundleID ?? app.bundleIDs[0]
    guard app.bundleIDs.contains(id) else {
      throw ProAppsError.invalid("Bundle ID does not match the selected app")
    }
    return id
  }
}

public enum UIWaitCondition: String, Codable, Sendable { case exists, absent }

/// One native UI request. The child process receives exactly one encoded value.
public enum UIRequest: Equatable, Sendable {
  case windows(UITarget)
  case inspect(
    UITarget, window: UIWindowLocator, root: UIElementLocator?, maxDepth: Int, maxElements: Int)
  case menuList(UITarget, path: [String])
  case menuSelect(UITarget, path: [String])
  case perform(UITarget, window: UIWindowLocator, target: UIElementLocator, action: UIAction)
  case set(
    UITarget, window: UIWindowLocator, target: UIElementLocator, attribute: UISettableAttribute,
    value: UISettableValue)
  case wait(
    UITarget, window: UIWindowLocator?, target: UIElementLocator?, condition: UIWaitCondition,
    timeoutSeconds: Double)
  case capture(UITarget, window: UIWindowLocator, outputPath: String)
  case inputSourceStatus
  case inputSourceSelect(id: String?, asciiCapable: Bool)
  case launch(UITarget, activate: Bool)
  case quit(UITarget, force: Bool)
  case finalCut(UITarget, FCPRequest)
}

extension UIRequest: Codable {
  private enum Keys: String, CodingKey {
    case operation, target, window, root, maxDepth, maxElements, path, element, action, attribute
    case stringValue, boolValue, condition, timeoutSeconds, outputPath, id, asciiCapable, activate
    case force, finalCut
  }

  private enum Operation: String, Codable {
    case windows, inspect, menuList, menuSelect, perform, set, wait, capture
    case inputSourceStatus, inputSourceSelect, launch, quit, finalCut
  }

  public init(from decoder: any Decoder) throws {
    let c = try decoder.container(keyedBy: Keys.self)
    switch try c.decode(Operation.self, forKey: .operation) {
    case .windows: self = .windows(try c.decode(UITarget.self, forKey: .target))
    case .inspect:
      self = .inspect(
        try c.decode(UITarget.self, forKey: .target),
        window: try c.decode(UIWindowLocator.self, forKey: .window),
        root: try c.decodeIfPresent(UIElementLocator.self, forKey: .root),
        maxDepth: try c.decode(Int.self, forKey: .maxDepth),
        maxElements: try c.decode(Int.self, forKey: .maxElements))
    case .menuList:
      self = .menuList(
        try c.decode(UITarget.self, forKey: .target),
        path: try c.decode([String].self, forKey: .path))
    case .menuSelect:
      self = .menuSelect(
        try c.decode(UITarget.self, forKey: .target),
        path: try c.decode([String].self, forKey: .path))
    case .perform:
      self = .perform(
        try c.decode(UITarget.self, forKey: .target),
        window: try c.decode(UIWindowLocator.self, forKey: .window),
        target: try c.decode(UIElementLocator.self, forKey: .element),
        action: try c.decode(UIAction.self, forKey: .action))
    case .set:
      let value: UISettableValue
      if let text = try c.decodeIfPresent(String.self, forKey: .stringValue) {
        value = .string(text)
      } else {
        value = .bool(try c.decode(Bool.self, forKey: .boolValue))
      }
      self = .set(
        try c.decode(UITarget.self, forKey: .target),
        window: try c.decode(UIWindowLocator.self, forKey: .window),
        target: try c.decode(UIElementLocator.self, forKey: .element),
        attribute: try c.decode(UISettableAttribute.self, forKey: .attribute), value: value)
    case .wait:
      self = .wait(
        try c.decode(UITarget.self, forKey: .target),
        window: try c.decodeIfPresent(UIWindowLocator.self, forKey: .window),
        target: try c.decodeIfPresent(UIElementLocator.self, forKey: .element),
        condition: try c.decode(UIWaitCondition.self, forKey: .condition),
        timeoutSeconds: try c.decode(Double.self, forKey: .timeoutSeconds))
    case .capture:
      self = .capture(
        try c.decode(UITarget.self, forKey: .target),
        window: try c.decode(UIWindowLocator.self, forKey: .window),
        outputPath: try c.decode(String.self, forKey: .outputPath))
    case .inputSourceStatus: self = .inputSourceStatus
    case .inputSourceSelect:
      self = .inputSourceSelect(
        id: try c.decodeIfPresent(String.self, forKey: .id),
        asciiCapable: try c.decode(Bool.self, forKey: .asciiCapable))
    case .launch:
      self = .launch(
        try c.decode(UITarget.self, forKey: .target),
        activate: try c.decode(Bool.self, forKey: .activate))
    case .quit:
      self = .quit(
        try c.decode(UITarget.self, forKey: .target), force: try c.decode(Bool.self, forKey: .force)
      )
    case .finalCut:
      self = .finalCut(
        try c.decode(UITarget.self, forKey: .target),
        try c.decode(FCPRequest.self, forKey: .finalCut))
    }
  }

  public func encode(to encoder: any Encoder) throws {
    var c = encoder.container(keyedBy: Keys.self)
    switch self {
    case .windows(let target):
      try c.encode(Operation.windows, forKey: .operation)
      try c.encode(target, forKey: .target)
    case .inspect(let target, let window, let root, let maxDepth, let maxElements):
      try c.encode(Operation.inspect, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(window, forKey: .window)
      try c.encodeIfPresent(root, forKey: .root)
      try c.encode(maxDepth, forKey: .maxDepth)
      try c.encode(maxElements, forKey: .maxElements)
    case .menuList(let target, let path):
      try c.encode(Operation.menuList, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(path, forKey: .path)
    case .menuSelect(let target, let path):
      try c.encode(Operation.menuSelect, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(path, forKey: .path)
    case .perform(let target, let window, let element, let action):
      try c.encode(Operation.perform, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(window, forKey: .window)
      try c.encode(element, forKey: .element)
      try c.encode(action, forKey: .action)
    case .set(let target, let window, let element, let attribute, let value):
      try c.encode(Operation.set, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(window, forKey: .window)
      try c.encode(element, forKey: .element)
      try c.encode(attribute, forKey: .attribute)
      switch value {
      case .string(let text): try c.encode(text, forKey: .stringValue)
      case .bool(let flag): try c.encode(flag, forKey: .boolValue)
      case .elements:
        throw EncodingError.invalidValue(
          value,
          EncodingError.Context(
            codingPath: c.codingPath,
            debugDescription: "Element handles are process-local and cannot be encoded"))
      }
    case .wait(let target, let window, let element, let condition, let timeout):
      try c.encode(Operation.wait, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encodeIfPresent(window, forKey: .window)
      try c.encodeIfPresent(element, forKey: .element)
      try c.encode(condition, forKey: .condition)
      try c.encode(timeout, forKey: .timeoutSeconds)
    case .capture(let target, let window, let outputPath):
      try c.encode(Operation.capture, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(window, forKey: .window)
      try c.encode(outputPath, forKey: .outputPath)
    case .inputSourceStatus:
      try c.encode(Operation.inputSourceStatus, forKey: .operation)
    case .inputSourceSelect(let id, let ascii):
      try c.encode(Operation.inputSourceSelect, forKey: .operation)
      try c.encodeIfPresent(id, forKey: .id)
      try c.encode(ascii, forKey: .asciiCapable)
    case .launch(let target, let activate):
      try c.encode(Operation.launch, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(activate, forKey: .activate)
    case .quit(let target, let force):
      try c.encode(Operation.quit, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(force, forKey: .force)
    case .finalCut(let target, let request):
      try c.encode(Operation.finalCut, forKey: .operation)
      try c.encode(target, forKey: .target)
      try c.encode(request, forKey: .finalCut)
    }
  }
}

/// One summarized accessibility element. Text is truncated to keep results bounded.
public struct UIElementSummary: Codable, Equatable, Sendable {
  public let path: [Int]
  public let role: String?
  public let subrole: String?
  public let identifier: String?
  public let title: String?
  public let description: String?
  public let value: String?
  public let enabled: Bool?
  public let selected: Bool?
  public let focused: Bool?
  public let actions: [String]
  public let valueSettable: Bool
  public let frame: UIFrame?
  /// Attributes (or `actions`/`settable`) the element failed to report.
  public let unreadable: [String]
}

public struct UIWindowSummary: Codable, Equatable, Sendable {
  public let index: Int
  public let title: String?
  public let role: String?
  public let subrole: String?
  public let main: Bool?
  public let minimized: Bool?
  public let modal: Bool?
  public let frame: UIFrame?
  /// Sheets and other window-like children (for example Open/Save panels).
  public let sheets: [UIElementSummary]
}

public struct UIMenuItem: Codable, Equatable, Sendable {
  public let title: String
  public let enabled: Bool?
  public let hasSubmenu: Bool
}

public struct UIInputSource: Codable, Equatable, Sendable {
  public let id: String
  public let asciiCapable: Bool
  public let selectable: Bool
  public init(id: String, asciiCapable: Bool, selectable: Bool) {
    self.id = id
    self.asciiCapable = asciiCapable
    self.selectable = selectable
  }
}

public struct UIRunningProcess: Equatable, Sendable {
  public let pid: Int32
  public let bundleID: String
  public init(pid: Int32, bundleID: String) {
    self.pid = pid
    self.bundleID = bundleID
  }
}

public struct UICaptureResult: Codable, Equatable, Sendable {
  public let outputPath: String
  public let width: Int
  public let height: Int
  public init(outputPath: String, width: Int, height: Int) {
    self.outputPath = outputPath
    self.width = width
    self.height = height
  }
}

/// Evidence attached to every UI response. Background operation is the default;
/// a changed frontmost application is reported rather than hidden.
public struct UIFocusEvidence: Codable, Equatable, Sendable {
  public let frontmostBefore: String?
  public let frontmostAfter: String?
  public var frontmostChanged: Bool { frontmostBefore != frontmostAfter }

  private enum CodingKeys: String, CodingKey {
    case frontmostBefore, frontmostAfter, frontmostChanged
  }

  public init(frontmostBefore: String?, frontmostAfter: String?) {
    self.frontmostBefore = frontmostBefore
    self.frontmostAfter = frontmostAfter
  }

  public init(from decoder: any Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    frontmostBefore = try c.decodeIfPresent(String.self, forKey: .frontmostBefore)
    frontmostAfter = try c.decodeIfPresent(String.self, forKey: .frontmostAfter)
  }

  public func encode(to encoder: any Encoder) throws {
    var c = encoder.container(keyedBy: CodingKeys.self)
    try c.encodeIfPresent(frontmostBefore, forKey: .frontmostBefore)
    try c.encodeIfPresent(frontmostAfter, forKey: .frontmostAfter)
    try c.encode(frontmostChanged, forKey: .frontmostChanged)
  }
}

/// Encoded child result. Optional fields are populated by the matching request.
public struct UIResponse: Codable, Equatable, Sendable {
  public var focus: UIFocusEvidence
  public var pid: Int32?
  public var running: Bool?
  public var windows: [UIWindowSummary]?
  public var elements: [UIElementSummary]?
  public var truncated: Bool?
  public var menuItems: [UIMenuItem]?
  public var element: UIElementSummary?
  public var elementExists: Bool?
  public var dispatched: Bool?
  public var effectVerified: Bool?
  public var conditionMet: Bool?
  public var elapsedSeconds: Double?
  public var capture: UICaptureResult?
  public var inputSource: UIInputSource?
  public var previousInputSource: UIInputSource?
  public var terminated: Bool?
  public var finalCut: FCPResult?

  public init(focus: UIFocusEvidence) { self.focus = focus }
}
