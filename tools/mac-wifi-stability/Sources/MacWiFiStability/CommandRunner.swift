import Foundation

internal struct CommandRunner: Sendable {
  private static let commandNotFoundStatus: Int32 = 127
  private static let commandTimedOutStatus: Int32 = 124
  private static let defaultTimeout: TimeInterval = 30
  private static let waitSlice: TimeInterval = 0.05

  internal func run(_ executable: String, arguments: [String]) -> CommandResult {
    run(executable, arguments: arguments, timeout: Self.defaultTimeout)
  }

  internal func run(
    _ executable: String,
    arguments: [String],
    timeout: TimeInterval
  ) -> CommandResult {
    let process = Process()
    let stdoutPipe = Pipe()
    let stderrPipe = Pipe()
    process.executableURL = URL(filePath: executable)
    process.arguments = arguments
    process.standardOutput = stdoutPipe
    process.standardError = stderrPipe

    do {
      try process.run()
      let deadline = Date().addingTimeInterval(timeout)
      while process.isRunning, Date() < deadline {
        Thread.sleep(forTimeInterval: Self.waitSlice)
      }

      let timedOut = process.isRunning
      if timedOut {
        process.terminate()
      }
      process.waitUntilExit()
      return CommandResult(
        status: timedOut ? Self.commandTimedOutStatus : process.terminationStatus,
        stdout: output(from: stdoutPipe),
        stderr: output(from: stderrPipe)
      )
    } catch {
      return CommandResult(
        status: Self.commandNotFoundStatus,
        stdout: "",
        stderr: error.localizedDescription
      )
    }
  }

  private func output(from pipe: Pipe) -> String {
    let data = try? pipe.fileHandleForReading.readToEnd()
    return String(bytes: data ?? Data(), encoding: .utf8) ?? ""
  }
}
