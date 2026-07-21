/// Supported modifier combinations for the global shortcut.
public enum ShortcutModifiers: String, CaseIterable, Identifiable, Sendable {
  case commandOption = "commandOption"
  case commandShift = "commandShift"
  case controlCommand = "controlCommand"
  case controlOption = "controlOption"
  case controlOptionCommand = "controlOptionCommand"
  case controlShift = "controlShift"

  /// Stable identity used by SwiftUI pickers.
  public var id: String {
    rawValue
  }

  /// Standard macOS modifier symbols shown to the user.
  public var displayName: String {
    switch self {
    case .controlOption:
      "⌃⌥"

    case .commandOption:
      "⌘⌥"

    case .controlShift:
      "⌃⇧"

    case .commandShift:
      "⌘⇧"

    case .controlCommand:
      "⌃⌘"

    case .controlOptionCommand:
      "⌃⌥⌘"
    }
  }
}
