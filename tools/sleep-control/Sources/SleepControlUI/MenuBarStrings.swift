import Foundation

/// Resolves menu-bar popover text from an explicitly selectable bundle.
public struct MenuBarStrings: Sendable {
  /// Status shown while system sleep is disabled.
  public let disabledStatus: String
  /// Status shown while system sleep is enabled.
  public let enabledStatus: String
  /// Status shown when the setting cannot be read.
  public let unavailableStatus: String
  /// Action that enables system sleep.
  public let enableSleep: String
  /// Action that disables system sleep.
  public let disableSleep: String
  /// Label of the manual reload button.
  public let reload: String
  /// Label of the settings link.
  public let settings: String
  /// Label of the quit button.
  public let quit: String

  /// Loads menu-bar text from the app bundle or a language-specific test bundle.
  public init(bundle: Bundle = .main) {
    disabledStatus = Self.localized("status.disabled", in: bundle)
    enabledStatus = Self.localized("status.enabled", in: bundle)
    unavailableStatus = Self.localized("status.unavailable", in: bundle)
    enableSleep = Self.localized("menu.enable_sleep", in: bundle)
    disableSleep = Self.localized("menu.disable_sleep", in: bundle)
    reload = Self.localized("button.reload", in: bundle)
    settings = Self.localized("menu.settings", in: bundle)
    quit = Self.localized("menu.quit", in: bundle)
  }

  private static func localized(_ key: String, in bundle: Bundle) -> String {
    bundle.localizedString(forKey: key, value: nil, table: nil)
  }
}
