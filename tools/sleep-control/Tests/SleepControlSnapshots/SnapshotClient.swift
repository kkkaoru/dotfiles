import SleepControlCore

@MainActor
internal final class SnapshotClient: SleepSettingsClient {
  private let sleepDisabled: Bool

  internal init(sleepDisabled: Bool) {
    self.sleepDisabled = sleepDisabled
  }

  internal func readSleepDisabled() -> Bool {
    sleepDisabled
  }

  internal func setSleepDisabled(_: Bool) {
    // Snapshots are read-only and never exercise the toggle callback.
  }
}
