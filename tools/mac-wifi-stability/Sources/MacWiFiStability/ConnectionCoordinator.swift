import Foundation
import MacWiFiStabilityCore

internal struct ConnectionCoordinator: Sendable {
  internal static let decisionWindowSeconds: TimeInterval = 90
  internal static let healthDecisionDelaySeconds: TimeInterval = 5
  internal static let targetSettleSeconds: TimeInterval = 8
  internal static let fallbackSettleSeconds: TimeInterval = 3
  internal static let userspaceRecoverySettleSeconds: TimeInterval = 6
  internal static let reassociationSettleSeconds: TimeInterval = 10
  internal static let singleAttempt = "1/1"

  internal let context: ApplicationContext

  internal func evaluateTargetHealth(state: NetworkState) throws {
    guard state.router == context.configuration.targetRouter else {
      return
    }
    guard context.store.healthDecisionIsAllowed(for: state.signature) else {
      context.logger.log("action=target-health-skip reason=already-decided")
      return
    }

    let startedAt = Date()
    try context.store.recordHealthDecision(for: state.signature)
    Thread.sleep(forTimeInterval: Self.healthDecisionDelaySeconds)
    guard let candidate = targetStateAfterDelay(startedAt: startedAt) else {
      return
    }

    let initialHealth = context.probe.oneShotHealth(for: candidate)
    context.logger.log("action=target-health-once \(initialHealth.logFields)")
    let health = recoverTargetConnectivity(initialHealth: initialHealth, startedAt: startedAt)
    guard !health.isHealthy else {
      return
    }
    guard withinDecisionWindow(startedAt) else {
      context.logger.log(
        "action=fallback-skip reason=decision-window-expired "
          + "window_seconds=\(Int(Self.decisionWindowSeconds))"
      )
      return
    }
    try fallbackToTethering(reason: health.reason)
  }

  internal func connectTarget() throws {
    let startedAt = Date()
    let target = context.connector.connect(to: context.configuration.targetSSID)
    context.logger.log(
      "action=connect-target ssid=\(context.configuration.targetSSID) "
        + "attempt=\(Self.singleAttempt) window_seconds=\(Int(Self.decisionWindowSeconds)) "
        + "result=\(target.succeeded ? "ok" : "failed") status=\(target.status)"
    )

    Thread.sleep(forTimeInterval: Self.targetSettleSeconds)
    let state = context.probe.currentState()
    let initialHealth = targetHealth(for: state)
    if state.router == context.configuration.targetRouter {
      try context.store.recordHealthDecision(for: state.signature)
    }
    context.logger.log("action=target-health-once \(initialHealth.logFields)")
    let health = recoverTargetConnectivity(initialHealth: initialHealth, startedAt: startedAt)

    if health.isHealthy {
      try finishSuccessfulTargetConnection()
      return
    }

    guard withinDecisionWindow(startedAt) else {
      context.logger.log(
        "action=fallback-skip reason=decision-window-expired "
          + "window_seconds=\(Int(Self.decisionWindowSeconds))"
      )
      return
    }
    try fallbackToTethering(reason: health.reason)
  }

  private func finishSuccessfulTargetConnection() throws {
    let recoveredState = context.probe.currentState()
    try context.store.saveSignature(recoveredState.signature)
    print("connected=\(context.configuration.targetSSID)")
    print("attempt=\(Self.singleAttempt)")
    print("health=good")
  }

  private func fallbackToTethering(reason: String) throws {
    context.logger.log(
      "action=fallback-start ssid=\(context.configuration.fallbackSSID) "
        + "attempt=\(Self.singleAttempt) reason=\(reason)"
    )
    let fallback = context.connector.connect(to: context.configuration.fallbackSSID)
    context.logger.log(
      "action=fallback-connect ssid=\(context.configuration.fallbackSSID) "
        + "result=\(fallback.succeeded ? "ok" : "failed") status=\(fallback.status)"
    )
    Thread.sleep(forTimeInterval: Self.fallbackSettleSeconds)
    let state = context.probe.currentState()
    let connected =
      state.isReadyForResync
      && state.router == context.configuration.fallbackRouter
    context.logger.log(
      "action=fallback-result ssid=\(context.configuration.fallbackSSID) "
        + "connected=\(connected) router=\(state.router ?? "none")"
    )
    guard connected else {
      return
    }
    try context.store.saveSignature(state.signature)
    print("connected=\(context.configuration.fallbackSSID)")
    print("attempt=\(Self.singleAttempt)")
    print("fallback_reason=\(reason)")
  }
}
