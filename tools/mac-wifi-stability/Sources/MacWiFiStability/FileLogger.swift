import Foundation

internal struct FileLogger: Sendable {
  private static let directoryPermissions = 0o700
  private static let filePermissions = 0o600
  private static let maximumBytes = 1_048_576

  internal let fileURL: URL

  internal init(home: URL) {
    fileURL = home.appending(path: "Library/Logs/com.kkkaoru.mac-wifi-stability.log")
  }

  internal func log(_ message: String) {
    do {
      try FileManager.default.createDirectory(
        at: fileURL.deletingLastPathComponent(),
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: Self.directoryPermissions]
      )
      let formatter = ISO8601DateFormatter()
      formatter.formatOptions = [.withInternetDateTime, .withTimeZone]
      let line = "\(formatter.string(from: Date())) \(message)\n"
      if !FileManager.default.fileExists(atPath: fileURL.path(percentEncoded: false)) {
        FileManager.default.createFile(
          atPath: fileURL.path(percentEncoded: false),
          contents: nil,
          attributes: [.posixPermissions: Self.filePermissions]
        )
      }
      let handle = try FileHandle(forWritingTo: fileURL)
      try handle.seekToEnd()
      try handle.write(contentsOf: Data(line.utf8))
      try handle.close()
      try FileManager.default.setAttributes(
        [.posixPermissions: Self.filePermissions],
        ofItemAtPath: fileURL.path(percentEncoded: false)
      )
      try rotateIfNeeded()
    } catch {
      try? FileHandle.standardError.write(contentsOf: Data("mac-wifi-stability: \(error)\n".utf8))
    }
  }

  private func rotateIfNeeded() throws {
    let attributes = try FileManager.default.attributesOfItem(
      atPath: fileURL.path(percentEncoded: false)
    )
    guard let size = attributes[.size] as? UInt64, size > Self.maximumBytes else {
      return
    }

    let rotatedURL = fileURL.appendingPathExtension("1")
    try? FileManager.default.removeItem(at: rotatedURL)
    try FileManager.default.moveItem(at: fileURL, to: rotatedURL)
  }
}
