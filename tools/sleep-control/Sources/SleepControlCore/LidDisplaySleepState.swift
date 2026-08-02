/// Tracks lid-close display behavior independently from the system sleep toggle.
public struct LidDisplaySleepState: Sendable {
  /// Whether the automatic display behavior is enabled.
  public private(set) var isEnabled = false

  /// The latest observed lid state, or `nil` before the first observation.
  public private(set) var lidIsClosed: Bool?

  private var displaysWereSleptByWatcher = false

  /// Creates an inactive state with no observed lid state.
  public init() {
    // The first lid notification supplies the initial state.
  }

  /// Enables or disables the behavior and returns any required display action.
  public mutating func setEnabled(_ enabled: Bool) -> LidDisplaySleepAction? {
    guard enabled != isEnabled else {
      return nil
    }
    isEnabled = enabled

    if enabled, lidIsClosed == true {
      return sleepDisplays()
    }
    return enabled ? nil : wakeDisplaysIfNeeded()
  }

  /// Records a lid notification and returns any required display action.
  public mutating func observe(lidIsClosed closed: Bool) -> LidDisplaySleepAction? {
    guard closed != lidIsClosed else {
      return nil
    }
    lidIsClosed = closed
    guard isEnabled else {
      return nil
    }
    return closed ? sleepDisplays() : wakeDisplaysIfNeeded()
  }

  private mutating func sleepDisplays() -> LidDisplaySleepAction? {
    guard !displaysWereSleptByWatcher else {
      return nil
    }
    displaysWereSleptByWatcher = true
    return .sleep
  }

  private mutating func wakeDisplaysIfNeeded() -> LidDisplaySleepAction? {
    guard displaysWereSleptByWatcher else {
      return nil
    }
    displaysWereSleptByWatcher = false
    return .wake
  }
}
