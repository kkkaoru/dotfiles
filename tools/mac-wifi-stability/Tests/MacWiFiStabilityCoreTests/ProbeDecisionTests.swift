import MacWiFiStabilityCore

internal enum ProbeDecisionTests {
  private static let extraSlowHTTPSeconds =
    ConnectivityEvaluator.httpSuccessMaxSeconds + ConnectivityEvaluator.httpSuccessMaxSeconds

  internal static func run() {
    parsesRunningTailscaleStatus()
    parsesStoppedTailscaleStatus()
    rejectsMalformedTailscaleStatus()
    acceptsFastHealthyHTTP()
    rejectsSlowHTTPSuccess()
    rejectsFailedHTTPWhenGatewayIsUp()
  }

  private static func parsesRunningTailscaleStatus() {
    let state = TailscaleRunStateParser.parseStatus(#"{"BackendState":"Running"}"#)
    expect(state == .running)
  }

  private static func parsesStoppedTailscaleStatus() {
    let state = TailscaleRunStateParser.parseStatus(#"{"BackendState":"Stopped"}"#)
    expect(state == .stopped)
  }

  private static func rejectsMalformedTailscaleStatus() {
    expect(TailscaleRunStateParser.parseStatus("not-json") == .unavailable)
    expect(TailscaleRunStateParser.parseStatus(#"{"BackendState":"Starting"}"#) == .unavailable)
  }

  private static func acceptsFastHealthyHTTP() {
    let verdict = ConnectivityEvaluator.evaluate(
      gatewayLossPercent: ConnectivityEvaluator.noPacketLoss,
      httpSucceeded: true,
      httpStatusCode: ConnectivityEvaluator.httpSuccessCode,
      httpSeconds: ConnectivityEvaluator.httpSuccessMaxSeconds,
      httpTimedOut: false
    )
    expect(verdict.isHealthy)
    expect(verdict.reason == "ok")
  }

  private static func rejectsSlowHTTPSuccess() {
    let verdict = ConnectivityEvaluator.evaluate(
      gatewayLossPercent: ConnectivityEvaluator.noPacketLoss,
      httpSucceeded: true,
      httpStatusCode: ConnectivityEvaluator.httpSuccessCode,
      httpSeconds: Self.extraSlowHTTPSeconds,
      httpTimedOut: false
    )
    expect(!verdict.isHealthy)
    expect(verdict.reason == "internet-check-slow")
  }

  private static func rejectsFailedHTTPWhenGatewayIsUp() {
    let verdict = ConnectivityEvaluator.evaluate(
      gatewayLossPercent: ConnectivityEvaluator.noPacketLoss,
      httpSucceeded: false,
      httpStatusCode: nil,
      httpSeconds: Self.extraSlowHTTPSeconds,
      httpTimedOut: false
    )
    expect(!verdict.isHealthy)
    expect(verdict.reason == "internet-check-failed")
  }

  private static func expect(_ condition: @autoclosure () -> Bool) {
    precondition(condition())
  }
}
