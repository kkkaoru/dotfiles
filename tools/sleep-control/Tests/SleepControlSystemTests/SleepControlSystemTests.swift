import Foundation

@main
@MainActor
internal enum SleepControlSystemTests {
  internal static func main() async throws {
    try await testSystemCommandArguments()
    try await testCommandExecution()
    try await testCommandCancellation()
    try await testPowerEvents()
    // Read-only smoke test: never invokes sudo or sleepnow on the real machine.
    let reading = try await SystemBatterySleepClient().read()
    print("System power checks passed; lid sensor available: \(reading.lidIsClosed != nil)")
  }

  private static func testSystemCommandArguments() async throws {
    let recorder = PowerCommandRecorder()
    let client = SystemBatterySleepClient { executable, arguments in
      await recorder.run(executable, arguments: arguments)
    }
    let reading = try await client.read()
    let expectedPercentage = 50.0
    guard reading.lidIsClosed == true, reading.batteryPercentage == expectedPercentage else {
      throw TestError.readFailed
    }
    try await client.enableSystemSleep()
    try await client.sleepNow()
    let commands = await recorder.commands
    let writeCommandCount = 2
    guard
      commands.contains(["/usr/sbin/ioreg", "-a", "-r", "-c", "IOPMrootDomain", "-d", "1"]),
      commands.contains(["/usr/sbin/ioreg", "-a", "-r", "-c", "AppleSmartBattery", "-d", "1"]),
      commands.suffix(writeCommandCount) == [
        ["/usr/bin/sudo", "-n", "/usr/bin/pmset", "-a", "disablesleep", "0"],
        ["/usr/bin/pmset", "sleepnow"],
      ]
    else { throw TestError.writeFailed }
  }

  private static func testCommandExecution() async throws {
    let output = try await PowerCommand.run("/usr/bin/printf", arguments: ["power-test"])
    guard output == Data("power-test".utf8) else { throw TestError.readFailed }
    do {
      _ = try await PowerCommand.run("/usr/bin/false", arguments: [])
      throw TestError.writeFailed
    } catch SleepSettingsError.commandFailed {
      // A failed command is surfaced to the automatic-sleep controller.
    }
    do {
      _ = try await PowerCommand.run("/nonexistent/sleep-control-test", arguments: [])
      throw TestError.writeFailed
    } catch is CocoaError {
      // A missing executable must not be treated as a successful sleep request.
    }
  }

  private static func testCommandCancellation() async throws {
    let task = Task { try await PowerCommand.run("/usr/bin/printf", arguments: ["cancelled"]) }
    task.cancel()
    do {
      _ = try await task.value
      throw TestError.writeFailed
    } catch is CancellationError {
      // Cancellation before command launch must avoid the command entirely.
    }
  }
}
