import Foundation

internal struct CommandResult {
  internal let status: Int32
  internal let output: Data
  internal let error: Data
}
