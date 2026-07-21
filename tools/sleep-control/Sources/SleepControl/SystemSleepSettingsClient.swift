import Foundation
import SleepControlCore

@MainActor
internal final class SystemSleepSettingsClient: SleepSettingsClient {
  internal func readSleepDisabled() throws -> Bool {
    let result = try execute("/usr/bin/pmset", arguments: ["-g"])
    guard result.status == 0 else {
      throw SleepSettingsError.commandFailed(result.status, decode(result.error))
    }
    guard
      let output = String(data: result.output, encoding: .utf8),
      let disabled = parseSleepDisabled(from: output)
    else {
      throw SleepSettingsError.invalidOutput
    }
    return disabled
  }

  internal func setSleepDisabled(_ disabled: Bool) throws {
    // -n guarantees that the GUI never falls back to an invisible password prompt.
    let result = try execute(
      "/usr/bin/sudo",
      arguments: ["-n", "/usr/bin/pmset", "-a", "disablesleep", disabled ? "1" : "0"]
    )
    guard result.status == 0 else {
      throw SleepSettingsError.authorizationUnavailable
    }
  }

  private func execute(
    _ executable: String,
    arguments: [String]
  ) throws -> CommandResult {
    let process = Process()
    let standardOutput = Pipe()
    let standardError = Pipe()
    process.executableURL = URL(filePath: executable)
    process.arguments = arguments
    process.standardOutput = standardOutput
    process.standardError = standardError

    try process.run()
    let output = try standardOutput.fileHandleForReading.readToEnd() ?? Data()
    let error = try standardError.fileHandleForReading.readToEnd() ?? Data()
    process.waitUntilExit()
    return CommandResult(status: process.terminationStatus, output: output, error: error)
  }

  private func decode(_ data: Data) -> String {
    String(data: data, encoding: .utf8) ?? ""
  }
}
