import Darwin

internal struct ApplicationContext: Sendable {
  internal let configuration: Configuration
  internal let store: MonitorStore
  internal let logger: FileLogger
  internal let probe: NetworkProbe
  internal let connector: NetworkConnector
  internal let resynchronizer: UserAgentResynchronizer
  internal let tailscaleResynchronizer: TailscaleResynchronizer

  internal init() throws {
    configuration = try Configuration()
    store = MonitorStore(home: configuration.home)
    logger = FileLogger(home: configuration.home)
    probe = NetworkProbe(runner: CommandRunner(), wifiDevice: configuration.wifiDevice)
    connector = NetworkConnector(runner: CommandRunner(), wifiDevice: configuration.wifiDevice)
    resynchronizer = UserAgentResynchronizer(
      runner: CommandRunner(),
      uid: String(getuid())
    )
    tailscaleResynchronizer = TailscaleResynchronizer(runner: CommandRunner())
    try store.prepare()
  }
}
