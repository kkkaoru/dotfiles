/// Provides the two privileged system operations used by the GUI model.
@MainActor
public protocol SleepSettingsClient: AnyObject {
  /// Reads whether system sleep is currently disabled.
  func readSleepDisabled() throws -> Bool

  /// Sets whether system sleep is disabled.
  func setSleepDisabled(_ disabled: Bool) throws
}
