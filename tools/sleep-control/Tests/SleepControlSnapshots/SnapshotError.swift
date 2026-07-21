import Foundation

internal enum SnapshotError: LocalizedError {
  case encodingFailed
  case invalidArguments
  case missingLocalization(String)
  case renderFailed

  internal var errorDescription: String? {
    switch self {
    case .encodingFailed:
      "Could not encode the snapshot as PNG."

    case .invalidArguments:
      "Expected resource and output directory arguments."

    case let .missingLocalization(language):
      "Missing localization bundle: \(language)"

    case .renderFailed:
      "The SwiftUI view did not produce a bitmap."
    }
  }
}
