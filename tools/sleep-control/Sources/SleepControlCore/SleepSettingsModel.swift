import Combine
import Foundation

/// Owns the displayed state and serializes user-triggered power-setting changes.
@MainActor
public final class SleepSettingsModel: ObservableObject {
  /// The resolved setting, or `nil` while unavailable.
  @Published public private(set) var isSleepDisabled: Bool?

  /// Whether a read or authenticated update is in progress.
  @Published public private(set) var isBusy = false

  /// A recoverable error suitable for display in the window.
  @Published public private(set) var errorMessage: String?

  private let client: any SleepSettingsClient

  /// Creates the model with the system client or a unit-test replacement.
  public init(client: any SleepSettingsClient) {
    self.client = client
  }

  /// Reloads the current `SleepDisabled` value without requesting privileges.
  public func refresh() {
    guard !isBusy else {
      return
    }

    isBusy = true
    defer { isBusy = false }

    do {
      isSleepDisabled = try client.readSleepDisabled()
      errorMessage = nil
    } catch {
      isSleepDisabled = nil
      errorMessage = error.localizedDescription
    }
  }

  /// Applies a new value after the system client completes administrator authentication.
  public func updateSleepDisabled(_ disabled: Bool) {
    guard !isBusy, disabled != isSleepDisabled else {
      return
    }

    isBusy = true
    defer { isBusy = false }

    do {
      try client.setSleepDisabled(disabled)
      isSleepDisabled = disabled
      errorMessage = nil
    } catch {
      errorMessage = error.localizedDescription
    }
  }

  /// Reverses the current setting, loading it first when necessary.
  public func toggleSleep() {
    guard !isBusy else {
      return
    }

    if let isSleepDisabled {
      updateSleepDisabled(!isSleepDisabled)
      return
    }

    refresh()
    guard let isSleepDisabled else {
      return
    }
    updateSleepDisabled(!isSleepDisabled)
  }
}
