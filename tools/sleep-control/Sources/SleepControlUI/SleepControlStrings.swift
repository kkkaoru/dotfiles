import Foundation

/// Resolves all user-facing view text from one explicitly selectable bundle.
public struct SleepControlStrings: Sendable {
  /// Main heading shown above the current state.
  public let title: String

  /// Status shown while system sleep is disabled.
  public let disabledStatus: String

  /// Status shown while system sleep is enabled.
  public let enabledStatus: String

  /// Status shown while the setting is being read.
  public let loadingStatus: String

  /// Status shown when the setting cannot be read.
  public let unavailableStatus: String

  /// Label of the sleep-enable switch.
  public let enableSleepToggle: String

  /// Explanation of the setting's scope.
  public let explanation: String

  /// Label of the manual reload button.
  public let reloadButton: String

  /// Loads view text from the app bundle or a language-specific test bundle.
  public init(bundle: Bundle = .main) {
    title = Self.localized("sleep.title", in: bundle)
    disabledStatus = Self.localized("status.disabled", in: bundle)
    enabledStatus = Self.localized("status.enabled", in: bundle)
    loadingStatus = Self.localized("status.loading", in: bundle)
    unavailableStatus = Self.localized("status.unavailable", in: bundle)
    enableSleepToggle = Self.localized("toggle.enable_sleep", in: bundle)
    explanation = Self.localized("sleep.explanation", in: bundle)
    reloadButton = Self.localized("button.reload", in: bundle)
  }

  private static func localized(_ key: String, in bundle: Bundle) -> String {
    bundle.localizedString(forKey: key, value: nil, table: nil)
  }
}
