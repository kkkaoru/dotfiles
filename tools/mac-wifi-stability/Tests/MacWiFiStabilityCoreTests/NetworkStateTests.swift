import MacWiFiStabilityCore

@main
internal enum NetworkStateTests {
  internal static func main() {
    parsesHealthyDHCPState()
    changesSignatureWhenAddressChanges()
    ignoresDHCPConnectionIDChanges()
    ignoresRedactedNetworkIdentifiers()
    rejectsUnavailableState()
    print("Swift tests: 5 passed")
  }

  private static func parsesHealthyDHCPState() {
    let state = NetworkStateParser.parse(
      ipconfigSummary: """
        ConnectionID : 3
        IPv4 : <array> {
          0 : 192.168.1.25
          Router : 192.168.1.1
        }
        LinkStatusActive : TRUE
        NetworkID : home-network
        """,
      defaultRoute: """
        gateway: 192.168.1.1
        interface: en0
        """
    )

    expect(state.isReadyForResync)
    expect(state.address == "192.168.1.25")
    expect(state.router == "192.168.1.1")
    expect(state.defaultInterface == "en0")
    expect(state.networkFingerprint != nil)
  }

  private static func changesSignatureWhenAddressChanges() {
    let route = "interface: en0"
    let first = NetworkStateParser.parse(
      ipconfigSummary:
        "ConnectionID : 3\n0 : 192.168.1.25\nRouter : 192.168.1.1\nLinkStatusActive : TRUE",
      defaultRoute: route
    )
    let second = NetworkStateParser.parse(
      ipconfigSummary:
        "ConnectionID : 3\n0 : 192.168.1.26\nRouter : 192.168.1.1\nLinkStatusActive : TRUE",
      defaultRoute: route
    )

    expect(first.signature != second.signature)
  }

  private static func ignoresDHCPConnectionIDChanges() {
    let route = "interface: en0"
    let first = NetworkStateParser.parse(
      ipconfigSummary:
        "ConnectionID : 3\n0 : 192.168.1.25\nRouter : 192.168.1.1\nLinkStatusActive : TRUE",
      defaultRoute: route
    )
    let renewed = NetworkStateParser.parse(
      ipconfigSummary:
        "ConnectionID : 4\n0 : 192.168.1.25\nRouter : 192.168.1.1\nLinkStatusActive : TRUE",
      defaultRoute: route
    )

    expect(first.signature == renewed.signature)
  }

  private static func ignoresRedactedNetworkIdentifiers() {
    let state = NetworkStateParser.parse(
      ipconfigSummary: "NetworkID : <redacted>\n0 : 192.168.1.25\nRouter : 192.168.1.1",
      defaultRoute: "interface: en0"
    )

    expect(state.networkFingerprint == nil)
    expect(!state.signature.contains("<redacted>"))
  }

  private static func rejectsUnavailableState() {
    let state = NetworkStateParser.parse(ipconfigSummary: "", defaultRoute: "")
    expect(!state.isAvailable)
    expect(state.signature == "unavailable")
    expect(!state.isReadyForResync)
  }

  private static func expect(_ condition: @autoclosure () -> Bool) {
    precondition(condition())
  }
}
