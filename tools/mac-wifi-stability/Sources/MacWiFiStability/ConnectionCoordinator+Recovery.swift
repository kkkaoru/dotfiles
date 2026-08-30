import Foundation

extension ConnectionCoordinator {
  internal func recoverTargetConnectivity(
    initialHealth: NetworkHealth,
    startedAt: Date
  ) -> NetworkHealth {
    guard !initialHealth.isHealthy, withinDecisionWindow(startedAt) else {
      return initialHealth
    }

    context.logger.log("action=target-recovery-start reason=\(initialHealth.reason)")
    let userspaceHealth = performUserspaceRecovery()
    guard !userspaceHealth.isHealthy, withinDecisionWindow(startedAt) else {
      return userspaceHealth
    }

    return performReassociationRecovery()
  }

  private func performUserspaceRecovery() -> NetworkHealth {
    context.resynchronizer.full().forEach { context.logger.log($0) }
    context.logger.log(context.tailscaleResynchronizer.reconcile())
    Thread.sleep(forTimeInterval: Self.userspaceRecoverySettleSeconds)

    let candidate = context.probe.currentState()
    let health = targetHealth(for: candidate)
    context.logger.log("action=target-recovery-health stage=userspace \(health.logFields)")
    return health
  }

  private func performReassociationRecovery() -> NetworkHealth {
    // Reassociating the same saved SSID makes CoreWLAN discard a stale mesh
    // client path without cycling Wi-Fi power or deleting the network profile.
    let reassociated = context.connector.connect(to: context.configuration.targetSSID)
    context.logger.log(
      "action=target-reassociate ssid=\(context.configuration.targetSSID) "
        + "result=\(reassociated.succeeded ? "ok" : "failed") status=\(reassociated.status)"
    )
    Thread.sleep(forTimeInterval: Self.reassociationSettleSeconds)

    var candidate = context.probe.currentState()
    guard candidate.router == context.configuration.targetRouter else {
      return targetHealth(for: candidate)
    }

    context.resynchronizer.full().forEach { context.logger.log($0) }
    context.logger.log(context.tailscaleResynchronizer.reconcile())
    Thread.sleep(forTimeInterval: Self.userspaceRecoverySettleSeconds)
    candidate = context.probe.currentState()
    let health = targetHealth(for: candidate)
    context.logger.log("action=target-recovery-health stage=reassociate \(health.logFields)")
    return health
  }
}
