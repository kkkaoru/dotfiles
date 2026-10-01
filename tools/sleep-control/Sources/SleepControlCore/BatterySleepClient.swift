/// Native power operations are injected so automatic sleep can be tested without sleeping the Mac.
@MainActor
public protocol BatterySleepClient {
  /// Reads the current lid and internal battery state.
  func read() async throws -> BatterySleepReading
  /// Enables system sleep using the existing password-free authorization.
  func enableSystemSleep() async throws
  /// Requests system sleep, not just display sleep.
  func sleepNow() async throws
}
