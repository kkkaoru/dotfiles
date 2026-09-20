import Foundation
import MacWiFiStabilityCore

internal struct NetworkProbe: Sendable {
  private static let standardPingPacketCount = 3
  private static let oneShotPingPacketCount = 1
  private static let stateCommandTimeoutSeconds: TimeInterval = 5
  private static let pingTimeoutSeconds: TimeInterval = 10
  private static let httpConnectTimeoutSeconds = 3
  private static let httpTimeoutSeconds: TimeInterval = 8
  private static let httpProcessOverheadSeconds: TimeInterval = 2

  internal let runner: CommandRunner
  internal let wifiDevice: String

  private static func evaluate(
    gateway: PingResult,
    http: ProbeOutputParser.HTTPCheck
  ) -> NetworkHealth {
    // Gateway latency is not a health signal: mesh roam can be briefly slow.
    // A 3s HTTP 200 after tether-to-Wi-Fi is DNS fallback, not a recovered path.
    let verdict = ConnectivityEvaluator.evaluate(
      gatewayLossPercent: gateway.packetLossPercent,
      httpSucceeded: http.command.succeeded,
      httpStatusCode: http.response.statusCode,
      httpSeconds: http.response.seconds,
      httpTimedOut: http.command.timedOut
    )
    return NetworkHealth(
      isHealthy: verdict.isHealthy,
      gateway: gateway,
      httpStatusCode: http.response.statusCode,
      httpSeconds: http.response.seconds,
      reason: verdict.reason
    )
  }

  internal func currentState() -> NetworkState {
    let summary = runner.run(
      "/usr/sbin/ipconfig",
      arguments: ["getsummary", wifiDevice],
      timeout: Self.stateCommandTimeoutSeconds
    )
    let route = runner.run(
      "/sbin/route",
      arguments: ["-n", "get", "default"],
      timeout: Self.stateCommandTimeoutSeconds
    )
    return NetworkStateParser.parse(
      ipconfigSummary: summary.succeeded ? summary.stdout : "",
      defaultRoute: route.succeeded ? route.stdout : ""
    )
  }

  internal func gatewayPing(router: String) -> PingResult {
    gatewayPing(router: router, packetCount: Self.standardPingPacketCount)
  }

  internal func gatewayPing(router: String, packetCount: Int) -> PingResult {
    let result = runner.run(
      "/sbin/ping",
      arguments: ["-n", "-c", String(packetCount), "-W", "1000", router],
      timeout: Self.pingTimeoutSeconds
    )
    return ProbeOutputParser.ping(result.stdout)
  }

  internal func oneShotHealth(for state: NetworkState) -> NetworkHealth {
    guard state.isReadyForResync, let router = state.router else {
      return NetworkHealth(
        isHealthy: false,
        gateway: nil,
        httpStatusCode: nil,
        httpSeconds: nil,
        reason: "network-not-ready"
      )
    }

    let gateway = gatewayPing(router: router, packetCount: Self.oneShotPingPacketCount)
    return Self.evaluate(gateway: gateway, http: internetCheck())
  }

  private func internetCheck() -> ProbeOutputParser.HTTPCheck {
    let command = runner.run(
      "/usr/bin/curl",
      arguments: [
        "-4",
        "-fsS",
        "-L",
        "--connect-timeout",
        String(Self.httpConnectTimeoutSeconds),
        "--max-time",
        String(Int(Self.httpTimeoutSeconds)),
        "-o",
        "/dev/null",
        "-w",
        "%{http_code} %{time_total}",
        "https://captive.apple.com/hotspot-detect.html",
      ],
      timeout: Self.httpTimeoutSeconds + Self.httpProcessOverheadSeconds
    )
    return ProbeOutputParser.HTTPCheck(
      command: command,
      response: ProbeOutputParser.http(command.stdout)
    )
  }
}
