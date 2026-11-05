import Foundation

/// Decides whether a display wake after a hot-corner display sleep came from the user.
public struct HotCornerDisplaySleepState: Sendable {
  // The hand that pushed the pointer into the corner often still moves it, and that residual
  // input wakes the display within the same second, so it does not count as a deliberate wake.
  private static let settleInterval: TimeInterval = 2
  // WindowServer names its user-activity assertion after the HID event that last woke it;
  // 29 is the biometric event type, which the Touch ID sensor reports.
  private static let biometricInputPrefix = "com.apple.iohideventsystem.queue.tickle "
  private static let biometricInputSuffix = " eventType:29"

  /// Whether the display-off hold is enabled.
  public private(set) var isEnabled = false

  /// Whether displays are being kept off after a hot-corner display sleep.
  public private(set) var isHolding = false

  private var holdStartedAt: TimeInterval = 0

  /// Creates a disabled state with no hold.
  public init() {
    // The stored preference enables the behavior when the app starts observing it.
  }

  /// Whether a power-assertion name shows that Touch ID input woke the displays.
  public static func isBiometricInput(assertionName: String) -> Bool {
    assertionName.hasPrefix(biometricInputPrefix) && assertionName.hasSuffix(biometricInputSuffix)
  }

  /// Enables or disables the behavior; disabling ends any current hold.
  public mutating func setEnabled(_ enabled: Bool) {
    isEnabled = enabled
    if !enabled {
      endHold()
    }
  }

  /// Starts a hold when displays went to sleep through a hot corner.
  public mutating func observeDisplaySleep(triggeredByHotCorner: Bool, at time: TimeInterval) {
    guard isEnabled, !isHolding, triggeredByHotCorner else {
      return
    }
    isHolding = true
    holdStartedAt = time
  }

  /// Returns whether displays must go back to sleep; deliberate user input ends the hold instead.
  ///
  /// The start of the hold stays fixed across repeated sleeps, so continued input always ends it.
  public mutating func shouldSleepDisplaysAfterWake(lastInputAt: TimeInterval) -> Bool {
    guard isHolding else {
      return false
    }
    guard lastInputAt < holdStartedAt + Self.settleInterval else {
      endHold()
      return false
    }
    return true
  }

  /// Ends the hold for a wake that input timing cannot identify.
  public mutating func endHold() {
    isHolding = false
  }
}
