/// User-configurable global shortcut for toggling Mac sleep.
public struct SleepToggleShortcut: Equatable, Sendable {
  /// Default shortcut chosen to avoid common application commands.
  public static let defaultValue = Self(modifiers: .controlOption, key: .letterS)

  /// Modifier combination pressed with the key.
  public var modifiers: ShortcutModifiers

  /// Letter key pressed with the modifiers.
  public var key: ShortcutKey

  /// Human-readable macOS shortcut notation.
  public var displayName: String {
    modifiers.displayName + key.displayName
  }

  /// Creates a shortcut from its modifier combination and letter key.
  public init(modifiers: ShortcutModifiers, key: ShortcutKey) {
    self.modifiers = modifiers
    self.key = key
  }
}
