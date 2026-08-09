internal enum MacWiFiStabilityCommand {
  case connectTarget
  case fullResync
  case help
  case once
  case status

  private static let commandByArgument: [String: Self] = [
    "--connect-ohomemesh": .connectTarget,
    "--force": .fullResync,
    "--force-rebind": .fullResync,
    "--help": .help,
    "-h": .help,
    "--ohomemesh": .connectTarget,
    "--once": .once,
    "--soft": .fullResync,
    "--status": .status,
  ]

  internal init(arguments: [String]) throws {
    guard let argument = arguments.first else {
      self = .once
      return
    }
    guard let command = Self.commandByArgument[argument] else {
      throw ApplicationError.unknownOption(argument)
    }
    self = command
  }
}
