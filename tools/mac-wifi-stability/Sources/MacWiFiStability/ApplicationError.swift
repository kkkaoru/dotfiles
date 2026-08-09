internal enum ApplicationError: Error, CustomStringConvertible {
  case homeUnavailable
  case unknownOption(String)

  internal var description: String {
    switch self {
    case .homeUnavailable:
      return "HOME is not set"

    case let .unknownOption(option):
      return "unknown option: \(option)"
    }
  }
}
