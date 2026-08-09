import Foundation

/// The read-only network identity used by the event-driven monitor.
public struct NetworkState: Equatable, Sendable {
  /// Whether `ipconfig getsummary` returned a usable summary.
  public let isAvailable: Bool

  /// The DHCP connection identifier, when present.
  public let connectionID: String?

  /// The current IPv4 address, when present.
  public let address: String?

  /// The DHCP-provided router address, when present.
  public let router: String?

  /// The CoreWLAN link status value, when present.
  public let linkStatus: String?

  /// The interface selected for the default route, when present.
  public let defaultInterface: String?

  /// A privacy-preserving fingerprint of a non-redacted network identifier.
  public let networkFingerprint: String?

  /// A stable signature suitable for persistence between process launches.
  public var signature: String {
    guard isAvailable else {
      return "unavailable"
    }

    // DHCP ConnectionID changes during renewals and user-space transitions.
    // It is not a network identity, so including it would turn a single
    // ohomemesh association into repeated health decisions.
    return [
      address ?? "none",
      router ?? "none",
      linkStatus ?? "unknown",
      defaultInterface ?? "none",
      networkFingerprint ?? "unknown",
    ].joined(separator: "|")
  }

  /// Whether restarting user-scope network agents is safe for this snapshot.
  public var isReadyForResync: Bool {
    isAvailable && linkStatus?.uppercased() == "TRUE" && address != nil && router != nil
  }
}
