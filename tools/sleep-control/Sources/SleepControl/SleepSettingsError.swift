import Foundation

internal enum SleepSettingsError: LocalizedError {
  case authorizationUnavailable
  case commandFailed(Int32, String)
  case invalidOutput

  internal var errorDescription: String? {
    switch self {
    case .authorizationUnavailable:
      return NSLocalizedString(
        "error.authorization_unavailable",
        bundle: .main,
        comment: "Password-free authorization is missing"
      )

    case let .commandFailed(status, message):
      guard message.isEmpty else {
        return message
      }
      let format = NSLocalizedString(
        "error.pmset_failed",
        bundle: .main,
        comment: "pmset failure with an exit status"
      )
      return String(format: format, status)

    case .invalidOutput:
      return NSLocalizedString(
        "error.invalid_output",
        bundle: .main,
        comment: "pmset output did not contain SleepDisabled"
      )
    }
  }
}
