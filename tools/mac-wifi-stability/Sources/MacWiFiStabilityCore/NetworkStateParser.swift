import Foundation

/// Parses the text formats emitted by macOS `ipconfig` and `route`.
public enum NetworkStateParser {
  private static let ipv4ComponentCount = 4
  private static let zeroComponent = "0"

  /// Parses one `ipconfig getsummary` result and one `route -n get default` result.
  public static func parse(ipconfigSummary: String, defaultRoute: String) -> NetworkState {
    let summary = ipconfigSummary.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !summary.isEmpty else {
      return NetworkState(
        isAvailable: false,
        connectionID: nil,
        address: nil,
        router: nil,
        linkStatus: nil,
        defaultInterface: nil,
        networkFingerprint: nil
      )
    }

    let connectionID = value(for: "ConnectionID", in: summary)
    let router = value(for: "Router", in: summary).flatMap(validIPv4)
    let linkStatus = value(for: "LinkStatusActive", in: summary)
    let networkID = value(for: "NetworkID", in: summary) ?? value(for: "SSID", in: summary)
    let address = firstIPv4Address(in: summary)
    let defaultInterface = value(for: "interface", in: defaultRoute, exactCase: false)
    let networkFingerprint = fingerprint(for: networkID)

    return NetworkState(
      isAvailable: true,
      connectionID: connectionID,
      address: address,
      router: router,
      linkStatus: linkStatus,
      defaultInterface: defaultInterface,
      networkFingerprint: networkFingerprint
    )
  }

  private static func value(for key: String, in text: String, exactCase: Bool = true) -> String? {
    let lines = text.split(whereSeparator: \.isNewline)
    for line in lines {
      let trimmed = line.trimmingCharacters(in: .whitespaces)
      guard let separator = trimmed.firstIndex(of: ":") else { continue }

      let candidateKey = String(trimmed[..<separator]).trimmingCharacters(in: .whitespaces)
      let keysMatch =
        exactCase ? candidateKey == key : candidateKey.lowercased() == key.lowercased()
      guard keysMatch else { continue }

      let valueStart = trimmed.index(after: separator)
      let candidateValue = String(trimmed[valueStart...]).trimmingCharacters(in: .whitespaces)
      if candidateValue.isEmpty {
        return nil
      }
      return candidateValue
    }

    return nil
  }

  private static func firstIPv4Address(in text: String) -> String? {
    for line in text.split(whereSeparator: \.isNewline) {
      let trimmed = line.trimmingCharacters(in: .whitespaces)
      guard let separator = trimmed.firstIndex(of: ":") else { continue }

      let candidateKey = String(trimmed[..<separator]).trimmingCharacters(in: .whitespaces)
      guard candidateKey == Self.zeroComponent else { continue }

      let valueStart = trimmed.index(after: separator)
      let candidate = String(trimmed[valueStart...]).trimmingCharacters(in: .whitespaces)
      if let address = validIPv4(candidate) {
        return address
      }
    }

    return nil
  }

  private static func validIPv4(_ value: String) -> String? {
    let components = value.split(separator: ".", omittingEmptySubsequences: false)
    guard components.count == Self.ipv4ComponentCount,
      components.allSatisfy({ component in
        guard let octet = UInt8(component) else {
          return false
        }
        return String(octet) == component || component == Self.zeroComponent
      })
    else {
      return nil
    }

    return value
  }

  private static func fingerprint(for networkID: String?) -> String? {
    guard let networkID,
      !networkID.isEmpty,
      networkID != "<redacted>",
      networkID != "unknown"
    else {
      return nil
    }

    let fnvOffset: UInt64 = 14_695_981_039_346_656_037
    let fnvPrime: UInt64 = 1_099_511_628_211
    var hash = fnvOffset
    for byte in networkID.utf8 {
      hash ^= UInt64(byte)
      hash &*= fnvPrime
    }

    return String(format: "%016llx", hash)
  }
}
