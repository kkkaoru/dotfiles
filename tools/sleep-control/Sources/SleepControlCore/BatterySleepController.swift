import Combine
import Foundation

/// Enables and requests sleep when a live, closed-lid battery reading reaches the cutoff.
@MainActor
public final class BatterySleepController: ObservableObject {
  /// The latest recoverable automatic-sleep error, exposed in Settings.
  @Published public private(set) var errorMessage: String?

  private let settings: ShortcutSettingsStore
  private let client: any BatterySleepClient
  private let didEnableSleep: @MainActor () -> Void
  private var isChecking = false

  /// Creates a controller with app-owned preferences and a system-setting refresh callback.
  public init(
    settings: ShortcutSettingsStore,
    client: any BatterySleepClient,
    didEnableSleep: @escaping @MainActor () -> Void
  ) {
    self.settings = settings
    self.client = client
    self.didEnableSleep = didEnableSleep
  }

  /// Checks live preferences and sensors, skipping cancelled or unavailable observations.
  public func check() async {
    guard !isChecking else {
      return
    }
    guard settings.batterySleep.isEnabled else {
      errorMessage = nil
      return
    }
    isChecking = true
    defer { isChecking = false }
    do {
      try await checkAndSleep()
      errorMessage = nil
    } catch is CancellationError {
      // A SwiftUI task restart or app shutdown must not request sleep using stale observations.
    } catch {
      errorMessage = error.localizedDescription
    }
  }

  /// Consumes coalesced events serially until the owning task is cancelled.
  public func monitor(events: AsyncThrowingStream<Void, any Error>) async {
    do {
      for try await _ in events {
        try Task.checkCancellation()
        await check()
      }
    } catch is CancellationError {
      // Cancellation belongs to the owner, not to the user-facing error state.
    } catch {
      errorMessage = error.localizedDescription
    }
  }

  private func checkAndSleep() async throws {
    let reading = try await client.read()
    try Task.checkCancellation()
    guard reading.requiresSleep(settings: settings.batterySleep) else {
      return
    }
    try await client.enableSystemSleep()
    didEnableSleep()
    try Task.checkCancellation()
    // Enabling sleep may await authorization/process completion. Recheck lid, battery and settings.
    let latest = try await client.read()
    try Task.checkCancellation()
    guard latest.requiresSleep(settings: settings.batterySleep) else {
      return
    }
    try await client.sleepNow()
  }
}
