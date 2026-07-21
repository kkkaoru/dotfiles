/// Determines whether a distinct lid-state transition should change display power.
///
/// - Parameters:
///   - previousLidState: The last observed state, updated before returning.
///   - lidIsClosed: Whether the current notification reports a closed lid.
///   - isInitialState: Whether this observation was read during startup.
/// - Returns: `true` to sleep displays, `false` to wake them, or `nil` for no action.
@inlinable
@inline(__always)
public func requestedDisplaySleepState(
  previousLidState: inout Bool?,
  lidIsClosed: Bool,
  isInitialState: Bool
) -> Bool? {
  let oldState = previousLidState
  previousLidState = lidIsClosed

  if oldState == lidIsClosed {
    return nil
  }

  if lidIsClosed {
    return true
  }

  return isInitialState ? nil : false
}
