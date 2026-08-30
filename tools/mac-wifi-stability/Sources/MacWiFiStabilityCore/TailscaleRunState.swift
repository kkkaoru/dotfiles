/// Whether the Tailscale backend intends its Network Extension to carry traffic.
public enum TailscaleRunState: Equatable, Sendable {
  case running
  case stopped
  case unavailable
}
