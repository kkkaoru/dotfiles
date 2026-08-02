import Foundation

/// Resolves settings text from an explicitly selectable bundle.
public struct SleepControlSettingsStrings: Sendable {
  /// Heading for automatic controls.
  public let automaticControls: String
  /// Label for lid-close display sleep.
  public let lidDisplaySleep: String
  /// Explanation of lid-close display sleep.
  public let lidDisplaySleepDescription: String
  /// Label for the Caps Lock light setting.
  public let capsLockLight: String
  /// Explanation of the Caps Lock light setting.
  public let capsLockLightDescription: String
  /// Heading for shortcut controls.
  public let shortcut: String
  /// Label for the modifier picker.
  public let modifiers: String
  /// Label for the key picker.
  public let key: String
  /// Label for the current shortcut.
  public let current: String
  /// Explanation of the global shortcut.
  public let description: String
  /// Message shown when shortcut registration conflicts.
  public let conflict: String

  /// Loads settings text from the app bundle or a language-specific test bundle.
  public init(bundle: Bundle = .main) {
    automaticControls = Self.localized("settings.behavior", in: bundle)
    lidDisplaySleep = Self.localized("settings.lid_close_display_sleep", in: bundle)
    lidDisplaySleepDescription = Self.localized(
      "settings.lid_close_display_sleep.description",
      in: bundle
    )
    capsLockLight = Self.localized("settings.caps_lock_light", in: bundle)
    capsLockLightDescription = Self.localized(
      "settings.caps_lock_light.description",
      in: bundle
    )
    shortcut = Self.localized("settings.shortcut", in: bundle)
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

/// Backward-compatible name for shortcut-settings callers.
public typealias ShortcutSettingsStrings = SleepControlSettingsStrings
