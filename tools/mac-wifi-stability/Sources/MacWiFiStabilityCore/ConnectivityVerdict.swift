/// Reachability verdict for the home mesh path.
public struct ConnectivityVerdict: Equatable, Sendable {
  /// Whether the target path can carry internet traffic.
  public let isHealthy: Bool

  /// Stable machine-readable reason for logs and recovery.
  public let reason: String
}
