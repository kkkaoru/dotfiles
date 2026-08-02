/// Resolves whether the optional Caps Lock indicator should be illuminated.
@inlinable
public func shouldIlluminateCapsLockIndicator(
  sleepDisabled: Bool?,
  isEnabled: Bool
) -> Bool {
  isEnabled && sleepDisabled == true
}
