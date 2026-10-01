import Foundation
#if canImport(SleepControlCore)
  import SleepControlCore
#endif

/// Uses fixed system commands without new sudoers permissions or private sensor APIs.
@MainActor
internal struct SystemBatterySleepClient: BatterySleepClient {
  internal var runCommand: @Sendable (String, [String]) async throws -> Data = { path, arguments in
    try await PowerCommand.run(path, arguments: arguments)
  }

  internal func read() async throws -> BatterySleepReading {
    async let lid = runCommand(
      "/usr/sbin/ioreg", ["-a", "-r", "-c", "IOPMrootDomain", "-d", "1"]
    )
    async let battery = runCommand(
      "/usr/sbin/ioreg", ["-a", "-r", "-c", "AppleSmartBattery", "-d", "1"]
    )
    return try await BatterySleepReading(lidData: lid, batteryData: battery)
  }

  internal func enableSystemSleep() async throws {
    // This exact command is already covered by Sleep Control's existing sudoers rule.
    _ = try await runCommand(
      "/usr/bin/sudo", ["-n", "/usr/bin/pmset", "-a", "disablesleep", "0"]
    )
  }

  internal func sleepNow() async throws {
    // pmset sleepnow is available to the logged-in user; no additional root grant is needed.
    _ = try await runCommand("/usr/bin/pmset", ["sleepnow"])
  }
}
