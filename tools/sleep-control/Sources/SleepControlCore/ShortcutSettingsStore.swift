import Combine
import Foundation

/// Persists the selected global shortcut in user defaults.
@MainActor
public final class ShortcutSettingsStore: ObservableObject {
  private static let keyDefaultsKey = "sleepToggleShortcut.key"
  private static let modifiersDefaultsKey = "sleepToggleShortcut.modifiers"

  /// Shortcut currently selected by the user.
  @Published public var shortcut: SleepToggleShortcut {
    didSet {
      defaults.set(shortcut.key.rawValue, forKey: Self.keyDefaultsKey)
      defaults.set(shortcut.modifiers.rawValue, forKey: Self.modifiersDefaultsKey)
    }
  }

  private let defaults: UserDefaults

  /// Loads a persisted shortcut, falling back to `⌃⌥S`.
  public init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
    shortcut = SleepToggleShortcut(
      modifiers: ShortcutModifiers(
        rawValue: defaults.string(forKey: Self.modifiersDefaultsKey) ?? ""
      ) ?? .controlOption,
      key: ShortcutKey(rawValue: defaults.string(forKey: Self.keyDefaultsKey) ?? "")
        ?? .letterS
    )
  }
}
