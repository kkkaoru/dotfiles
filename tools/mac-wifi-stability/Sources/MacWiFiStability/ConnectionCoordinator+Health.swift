import Foundation
import MacWiFiStabilityCore

extension ConnectionCoordinator {
  internal func targetHealth(for state: NetworkState) -> NetworkHealth {
    guard state.router == context.configuration.targetRouter else {
      return NetworkHealth(
        isHealthy: false,
        gateway: nil,
        httpStatusCode: nil,
        httpSeconds: nil,
        reason: "target-not-connected"
      )
    }
    return context.probe.oneShotHealth(for: state)
  }

  internal func withinDecisionWindow(_ startedAt: Date) -> Bool {
    Date().timeIntervalSince(startedAt) < Self.decisionWindowSeconds
  }

  internal func targetStateAfterDelay(startedAt: Date) -> NetworkState? {
    guard withinDecisionWindow(startedAt) else {
      context.logger.log(
        "action=target-health-skip reason=decision-window-expired "
          + "window_seconds=\(Int(Self.decisionWindowSeconds))"
      )
      return nil
    }

    let candidate = context.probe.currentState()
    guard candidate.router == context.configuration.targetRouter else {
      context.logger.log("action=target-health-skip reason=network-changed")
      return nil
    }
    return candidate
  }
}
