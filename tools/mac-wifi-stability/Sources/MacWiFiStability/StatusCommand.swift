import Foundation

internal enum StatusCommand {
  internal static func run() throws {
    let context = try ApplicationContext()
    let state = context.probe.currentState()
    let health = context.probe.oneShotHealth(for: state)
    print("device=\(context.configuration.wifiDevice)")
    print("address=\(state.address ?? "none")")
    print("router=\(state.router ?? "none")")
    print("link=\(state.linkStatus ?? "unknown")")
    print("default_interface=\(state.defaultInterface ?? "none")")
    print("gateway_ping_avg_ms=\(formatted(health.gateway?.averageMilliseconds))")
    print("gateway_packet_loss_percent=\(formatted(health.gateway?.packetLossPercent))")
    print("internet_http_status=\(health.httpStatusCode.map(String.init) ?? "unavailable")")
    print("internet_http_seconds=\(formatted(health.httpSeconds))")
    print("health=\(health.isHealthy ? "good" : "degraded")")
    print("health_reason=\(health.reason)")
    print("state_file=\(context.store.signatureURL.path(percentEncoded: false))")
    print("log_file=\(context.logger.fileURL.path(percentEncoded: false))")
  }

  private static func formatted(_ value: Double?) -> String {
    guard let value else {
      return "unavailable"
    }
    return String(format: "%.3f", value)
  }
}
