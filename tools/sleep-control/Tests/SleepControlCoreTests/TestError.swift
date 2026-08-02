import Foundation

internal enum TestError: String, LocalizedError {
  case capsLockLEDFailed = "caps lock LED failed"
  case readFailed = "read failed"
  case writeFailed = "write failed"

  internal var errorDescription: String? {
    rawValue
  }
}
