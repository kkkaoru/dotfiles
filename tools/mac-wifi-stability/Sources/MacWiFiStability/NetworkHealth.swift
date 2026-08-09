import Foundation

internal struct NetworkHealth: Sendable {
  internal let isHealthy: Bool
  internal let gateway: PingResult?
  internal let httpStatusCode: Int?
  internal let httpSeconds: Double?
  internal let reason: String

  internal var logFields: String {
    let gatewayAverage = gateway?.averageMilliseconds.map { String(format: "%.3f", $0) } ?? "none"
    let gatewayLoss = gateway?.packetLossPercent.map { String(format: "%.3f", $0) } ?? "none"
    let httpCode = httpStatusCode.map(String.init) ?? "none"
    let httpDuration = httpSeconds.map { String(format: "%.3f", $0) } ?? "none"
    return "healthy=\(isHealthy) reason=\(reason) gateway_avg_ms=\(gatewayAverage) "
      + "gateway_loss_percent=\(gatewayLoss) http_code=\(httpCode) http_seconds=\(httpDuration)"
  }
}
