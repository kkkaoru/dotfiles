import Darwin
import Foundation

/// A process-wide lock backed by a kernel advisory lock.
///
/// launchd can start the event handler while an explicit connection command is
/// still settling. A non-blocking lock makes the event handler leave that
/// transaction alone instead of issuing a second Wi-Fi join or health check.
internal final class ProcessLock: @unchecked Sendable {
  private let fileDescriptor: Int32

  internal init?(url: URL) {
    let descriptor = Darwin.open(
      url.path(percentEncoded: false),
      O_CREAT | O_RDWR,
      S_IRUSR | S_IWUSR
    )
    guard descriptor >= 0 else {
      return nil
    }
    guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
      Darwin.close(descriptor)
      return nil
    }
    fileDescriptor = descriptor
  }

  deinit {
    _ = flock(fileDescriptor, LOCK_UN)
    _ = Darwin.close(fileDescriptor)
  }
}
