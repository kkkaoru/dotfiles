import Foundation

internal struct CommandResult: Sendable {
  private static let timedOutStatus: Int32 = 124

  internal let status: Int32
  internal let stdout: String
  internal let stderr: String

  internal var succeeded: Bool {
    status == 0
  }

  internal var timedOut: Bool {
    status == Self.timedOutStatus
  }
}
