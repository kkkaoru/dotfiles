import Foundation

internal enum SnapshotError: LocalizedError {
  case encodingFailed
  case invalidArguments
  case menuTitleRegression
  case missingLocalization(String)
  case renderFailed
  case settingsObservationFailed

  internal var errorDescription: String? {
    switch self {
    case .encodingFailed:
      "Could not encode the snapshot as PNG."

    case .invalidArguments:
      "Expected resource and output directory arguments."

    case .menuTitleRegression:
      "Menu title normalization changed menu behavior."

    case let .missingLocalization(language):
      "Missing localization bundle: \(language)"

    case .renderFailed:
      "The SwiftUI view did not produce a bitmap."

    case .settingsObservationFailed:
      "Settings did not deliver exactly one callback for the shortcut change."
    }
  }
}
