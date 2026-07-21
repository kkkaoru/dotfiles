import Foundation

/// Resolves shortcut-settings text from an explicitly selectable bundle.
public struct ShortcutSettingsStrings: Sendable {
  /// Label for the modifier picker.
  public let modifiers: String

  /// Label for the letter-key picker.
  public let key: String

  /// Label for the combined shortcut preview.
  public let current: String

  /// Explanation of the shortcut's lifetime and scope.
  public let description: String

  /// Message shown when another application owns the shortcut.
  public let conflict: String

  /// Loads settings text from the app bundle or a language-specific test bundle.
  public init(bundle: Bundle = .main) {
    modifiers = Self.localized("settings.modifiers", in: bundle)
    key = Self.localized("settings.key", in: bundle)
    current = Self.localized("settings.current", in: bundle)
    description = Self.localized("settings.description", in: bundle)
    conflict = Self.localized("settings.conflict", in: bundle)
  }

  private static func localized(_ key: String, in bundle: Bundle) -> String {
    bundle.localizedString(forKey: key, value: nil, table: nil)
  }
}
