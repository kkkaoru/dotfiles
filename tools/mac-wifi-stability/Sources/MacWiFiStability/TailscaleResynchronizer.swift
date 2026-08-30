import Foundation
import MacWiFiStabilityCore

internal struct TailscaleResynchronizer: Sendable {
  private static let commandTimeoutSeconds: TimeInterval = 8
  private static let defaultBinary = "/Applications/Tailscale.app/Contents/MacOS/Tailscale"

  internal let runner: CommandRunner
  internal let binary: String

  internal init(runner: CommandRunner) {
    self.init(runner: runner, binary: Self.defaultBinary)
  }

  internal init(runner: CommandRunner, binary: String) {
    self.runner = runner
    self.binary = binary
  }

  internal func reconcile() -> String {
    guard FileManager.default.isExecutableFile(atPath: binary) else {
      return "action=tailscale-rebind result=skip reason=not-installed"
    }

    let preferences = runner.run(
      binary,
      arguments: ["debug", "prefs"],
      timeout: Self.commandTimeoutSeconds
    )
    switch TailscaleRunStateParser.parse(preferences.stdout) {
    case .running:
      let rebound = runner.run(
        binary,
        arguments: ["debug", "rebind"],
        timeout: Self.commandTimeoutSeconds
      )
      return "action=tailscale-rebind result=\(rebound.succeeded ? "ok" : "failed")"

    case .stopped:
      // `debug rebind` wakes the on-demand macOS Network Extension even when
      // WantRunning is false. That creates a Connected/Stopped split-brain VPN
      // state, so never call it for a stopped backend.
      return "action=tailscale-rebind result=skip reason=stopped"

    case .unavailable:
      return "action=tailscale-rebind result=skip reason=state-unavailable"
    }
  }
}
