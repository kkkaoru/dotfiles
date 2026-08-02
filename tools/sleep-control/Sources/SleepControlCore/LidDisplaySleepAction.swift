/// A display-power action produced by the lid-close behavior.
public enum LidDisplaySleepAction: Equatable, Sendable {
  /// Turn all displays off because the lid closed.
  case sleep
  /// Wake displays that this behavior previously turned off.
  case wake
}
