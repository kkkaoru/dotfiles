import Combine
import Foundation

/// Persists behavior and global-shortcut settings in a shared user-defaults suite.
@MainActor
public final class ShortcutSettingsStore: ObservableObject {
  /// Preferences suite shared with the lid display watcher.
  public static let defaultsSuiteName = "com.kkkaoru.sleep-control"

  /// Preference read by the lid display watcher.
  public static let lidDisplaySleepEnabledDefaultsKey = "lidDisplaySleepEnabled"

  /// Preference controlling the Caps Lock sleep-status indicator.
  public static let capsLockLightEnabledDefaultsKey = "capsLockLightEnabled"

  private static let legacyCapsLockLightDefaultsKey = "behavior.capsLockLight"
  private static let legacyLidDisplaySleepDefaultsKey = "behavior.lidCloseDisplaySleep"
  private static let keyDefaultsKey = "sleepToggleShortcut.key"
  private static let modifiersDefaultsKey = "sleepToggleShortcut.modifiers"

  /// Whether closing the MacBook lid should put all displays to sleep.
  @Published public var isLidDisplaySleepEnabled: Bool {
    didSet {
      defaults.set(
        isLidDisplaySleepEnabled,
        forKey: Self.lidDisplaySleepEnabledDefaultsKey
      )
    }
  }

  /// Whether disabled system sleep should be indicated with the Caps Lock LED.
  @Published public var isCapsLockLightEnabled: Bool {
    didSet {
      defaults.set(
        isCapsLockLightEnabled,
        forKey: Self.capsLockLightEnabledDefaultsKey
      )
    }
  }

  /// Backward-compatible name for the lid-close behavior.
  public var sleepDisplaysWhenLidCloses: Bool {
    get { isLidDisplaySleepEnabled }
    set { isLidDisplaySleepEnabled = newValue }
  }

  /// Backward-compatible name for the Caps Lock indicator behavior.
  public var capsLockLightEnabled: Bool {
    get { isCapsLockLightEnabled }
    set { isCapsLockLightEnabled = newValue }
  }

  /// Shortcut currently selected by the user.
  @Published public var shortcut: SleepToggleShortcut {
    didSet {
      defaults.set(shortcut.key.rawValue, forKey: Self.keyDefaultsKey)
      defaults.set(shortcut.modifiers.rawValue, forKey: Self.modifiersDefaultsKey)
    }
  }

  private let defaults: UserDefaults

  /// Loads preferences from the shared Sleep Control suite.
  public convenience init() {
    self.init(
      defaults: UserDefaults(suiteName: Self.defaultsSuiteName) ?? .standard
    )
  }

  /// Loads persisted settings, falling back to enabled controls and the default shortcut.
  public init(defaults: UserDefaults) {
    self.defaults = defaults
    isLidDisplaySleepEnabled = Self.storedBoolean(
      in: defaults,
      key: Self.lidDisplaySleepEnabledDefaultsKey,
      legacyKey: Self.legacyLidDisplaySleepDefaultsKey
    )
    isCapsLockLightEnabled = Self.storedBoolean(
      in: defaults,
      key: Self.capsLockLightEnabledDefaultsKey,
      legacyKey: Self.legacyCapsLockLightDefaultsKey
    )
    shortcut = SleepToggleShortcut(
      modifiers: ShortcutModifiers(
        rawValue: defaults.string(forKey: Self.modifiersDefaultsKey) ?? ""
      ) ?? .controlOption,
      key: ShortcutKey(rawValue: defaults.string(forKey: Self.keyDefaultsKey) ?? "")
        ?? .letterS
    )
  }

  private static func storedBoolean(
    in defaults: UserDefaults,
    key: String,
    legacyKey: String
  ) -> Bool {
    if let value = defaults.object(forKey: key) as? Bool {
      return value
    }
    return defaults.object(forKey: legacyKey) as? Bool ?? true
  }
}
