/// Independent automatic-sleep preferences, defaulting to enabled at 50%.
public struct BatterySleepSettings: Equatable, Sendable {
  /// Whether a closed laptop should sleep at the battery cutoff.
  public var isEnabled = true
  /// Inclusive battery cutoff.
  public var threshold = BatterySleepThreshold.percent50

  /// Creates the default preferences.
  public init() {
    // Stored property defaults define first-launch behavior.
  }
}
