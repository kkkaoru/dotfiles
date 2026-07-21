import Foundation

internal enum TestError: String, LocalizedError {
  case readFailed = "read failed"
  case writeFailed = "write failed"

  internal var errorDescription: String? {
    rawValue
  }
}
