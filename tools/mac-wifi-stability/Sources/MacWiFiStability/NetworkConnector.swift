import Foundation

internal struct NetworkConnector: Sendable {
  private static let connectTimeout: TimeInterval = 15

  internal let runner: CommandRunner
  internal let wifiDevice: String

  internal func connect(to ssid: String) -> CommandResult {
    runner.run(
      "/usr/sbin/networksetup",
      arguments: ["-setairportnetwork", wifiDevice, ssid],
      timeout: Self.connectTimeout
    )
  }
}
