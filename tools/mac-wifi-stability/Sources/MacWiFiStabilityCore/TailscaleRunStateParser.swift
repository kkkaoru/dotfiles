import Foundation

/// Parses the intentionally narrow part of `tailscale debug prefs` that is
/// needed before asking Tailscale to rebuild its network binding.
public enum TailscaleRunStateParser {
  /// Returns `.unavailable` for missing, malformed, or incomplete output.
  public static func parse(_ output: String) -> TailscaleRunState {
    guard let data = output.data(using: .utf8),
      let object = try? JSONSerialization.jsonObject(with: data),
      let preferences = object as? [String: Any],
      let wantRunning = preferences["WantRunning"] as? Bool
    else {
      return .unavailable
    }

    return wantRunning ? .running : .stopped
  }
}
