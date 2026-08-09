import Foundation
import MacWiFiStabilityCore

internal enum StatusCommand {
  private static let degradedLossPercent = 100.0
  private static let degradedAverageMilliseconds = 250.0

  internal static func run() throws {
    let context = try ApplicationContext()
    let state = context.probe.currentState()
    let ping = state.router.map(context.probe.gatewayPing(router:))
    print("device=\(context.configuration.wifiDevice)")
    print("address=\(state.address ?? "none")")
    print("router=\(state.router ?? "none")")
    print("link=\(state.linkStatus ?? "unknown")")
    print("default_interface=\(state.defaultInterface ?? "none")")
    print("gateway_ping_avg_ms=\(formatted(ping?.averageMilliseconds))")
    print("gateway_packet_loss_percent=\(formatted(ping?.packetLossPercent))")
    print("health=\(health(for: state, ping: ping))")
    print("state_file=\(context.store.signatureURL.path(percentEncoded: false))")
    print("log_file=\(context.logger.fileURL.path(percentEncoded: false))")
  }

  private static func health(for state: NetworkState, ping: PingResult?) -> String {
    guard state.isReadyForResync,
      let ping,
      let average = ping.averageMilliseconds,
      let loss = ping.packetLossPercent
    else {
      return "degraded"
    }

    return loss >= Self.degradedLossPercent || average > Self.degradedAverageMilliseconds
      ? "degraded"
      : "good"
  }

  private static func formatted(_ value: Double?) -> String {
    guard let value else {
      return "unavailable"
    }
    return String(format: "%.3f", value)
  }
}
