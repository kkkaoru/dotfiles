import Foundation

extension MonitorStore {
  internal func healthDecisionIsAllowed(for signature: String) -> Bool {
    guard readText(from: lastHealthDecisionURL) == signature else {
      return true
    }
    if readText(from: lastHealthResultURL) == "ok" {
      return false
    }
    guard let last = readEpoch(from: lastHealthAttemptURL) else {
      return true
    }
    return Date().timeIntervalSince1970 - last >= Self.lightActionCooldown
  }

  internal func recordHealthDecision(for signature: String) throws {
    try saveText(signature, to: lastHealthDecisionURL)
    try saveEpoch(Date().timeIntervalSince1970, to: lastHealthAttemptURL)
    try saveText("pending", to: lastHealthResultURL)
  }

  internal func recordHealthOutcome(isHealthy: Bool) throws {
    try saveText(isHealthy ? "ok" : "failed", to: lastHealthResultURL)
  }

  internal func clearHealthDecision() throws {
    try removeIfPresent(lastHealthDecisionURL)
    try removeIfPresent(lastHealthResultURL)
    try removeIfPresent(lastHealthAttemptURL)
  }
}
