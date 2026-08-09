import Foundation
import MacWiFiStabilityCore

internal struct NetworkProbe: Sendable {
  private static let standardPingPacketCount = 3
  private static let oneShotPingPacketCount = 1
  private static let stateCommandTimeoutSeconds: TimeInterval = 5
  private static let pingTimeoutSeconds: TimeInterval = 10
  private static let noPacketLoss = 0.0
  private static let httpConnectTimeoutSeconds = 3
  private static let httpTimeoutSeconds: TimeInterval = 8
  private static let httpProcessOverheadSeconds: TimeInterval = 2
  private static let httpSuccessCode = 200

  internal let runner: CommandRunner
  internal let wifiDevice: String

  private static func evaluate(
    gateway: PingResult,
    http: ProbeOutputParser.HTTPCheck
  ) -> NetworkHealth {
    // This is a reachability decision, not a latency policy. A Multi-AP mesh
    // can briefly take a slower path while it roams; falling back on that
    // transient latency would make a healthy ohomemesh association flap.
    let gatewayGood = gateway.packetLossPercent == Self.noPacketLoss
    let httpDurationGood =
      http.response.seconds.map { seconds in
        seconds <= Self.httpTimeoutSeconds
      } ?? false
    let httpGood =
      http.command.succeeded
      && http.response.statusCode == Self.httpSuccessCode
      && httpDurationGood
    let reason: String
    if !gatewayGood {
      reason = "gateway-unreachable"
    } else if !httpGood {
      reason = http.command.timedOut ? "internet-check-timeout" : "internet-check-failed"
    } else {
      reason = "ok"
    }

    return NetworkHealth(
      isHealthy: gatewayGood && httpGood,
      gateway: gateway,
      httpStatusCode: http.response.statusCode,
      httpSeconds: http.response.seconds,
      reason: reason
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
