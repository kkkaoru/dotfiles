import Foundation

/// Runs the native display power commands without blocking lid notifications.
internal enum DisplayPower {
  internal static func setSleeping(_ sleeping: Bool) {
    let process = Process()
    if sleeping {
      process.executableURL = URL(filePath: "/usr/bin/pmset")
      process.arguments = ["displaysleepnow"]
    } else {
      process.executableURL = URL(filePath: "/usr/bin/caffeinate")
      process.arguments = ["-u", "-t", "2"]
    }
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    try? process.run()
  }
}
