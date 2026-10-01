import Foundation

/// Runs fixed power-management commands off the UI and cooperative executor threads.
internal enum PowerCommand {
  internal static func run(_ executable: String, arguments: [String]) async throws -> Data {
    try Task.checkCancellation()
    return try await withCheckedThrowingContinuation { continuation in
      DispatchQueue.global(qos: .utility).async {
        do {
          continuation.resume(returning: try execute(executable, arguments: arguments))
        } catch {
          continuation.resume(throwing: error)
        }
      }
    }
  }

  private static func execute(_ executable: String, arguments: [String]) throws -> Data {
    let process = Process()
    let output = Pipe()
    defer {
      if process.isRunning {
        process.terminate()
        process.waitUntilExit()
      }
    }
    process.executableURL = URL(filePath: executable)
    process.arguments = arguments
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    try process.run()
    let data = try output.fileHandleForReading.readToEnd() ?? Data()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
      throw SleepSettingsError.commandFailed(process.terminationStatus, "")
    }
    return data
  }
}
