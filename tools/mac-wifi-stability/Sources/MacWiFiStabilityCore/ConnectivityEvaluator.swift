/// Classifies gateway ping plus HTTP probe output.
public enum ConnectivityEvaluator {
  /// Successful ICMP loss percentage.
  public static let noPacketLoss = 0.0

  /// HTTP status treated as a live internet path.
  public static let httpSuccessCode = 200

  /// Slower captive-portal checks are DNS fallback, not a recovered path.
  public static let httpSuccessMaxSeconds = 2.0

  /// Returns the monitor's health decision for one probe pair.
  public static func evaluate(
    gatewayLossPercent: Double?,
    httpSucceeded: Bool,
    httpStatusCode: Int?,
    httpSeconds: Double?,
    httpTimedOut: Bool
  ) -> ConnectivityVerdict {
    let gatewayGood = gatewayLossPercent == Self.noPacketLoss
    let httpFastEnough =
      httpSeconds.map { seconds in
        seconds <= Self.httpSuccessMaxSeconds
      } ?? false
    let httpGood =
      httpSucceeded
      && httpStatusCode == Self.httpSuccessCode
      && httpFastEnough
    return ConnectivityVerdict(
      isHealthy: gatewayGood && httpGood,
      reason: reason(
        gatewayGood: gatewayGood,
        httpGood: httpGood,
        httpStatusCode: httpStatusCode,
        httpTimedOut: httpTimedOut
      )
    )
  }

  private static func reason(
    gatewayGood: Bool,
    httpGood: Bool,
    httpStatusCode: Int?,
    httpTimedOut: Bool
  ) -> String {
    if !gatewayGood {
      return "gateway-unreachable"
    }
    if httpGood {
      return "ok"
    }
    if httpTimedOut {
      return "internet-check-timeout"
    }
    if httpStatusCode == Self.httpSuccessCode {
      return "internet-check-slow"
    }
    return "internet-check-failed"
  }
}
