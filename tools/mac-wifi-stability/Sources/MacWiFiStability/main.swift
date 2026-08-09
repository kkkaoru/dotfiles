import Foundation
import MacWiFiStabilityCore

internal enum MacWiFiStabilityMain {
  private static let transitionSettleSeconds: TimeInterval = 3

  internal static func main() {
    do {
      try run(arguments: Array(CommandLine.arguments.dropFirst()))
    } catch {
      try? FileHandle.standardError.write(contentsOf: Data("mac-wifi-stability: \(error)\n".utf8))
      exit(EXIT_FAILURE)
    }
  }

  private static func run(arguments: [String]) throws {
    switch try MacWiFiStabilityCommand(arguments: arguments) {
    case .help:
      UsagePrinter.printUsage()

    case .status:
      try StatusCommand.run()

    case .fullResync:
      try runFullResync()

    case .connectTarget:
      try runConnection()

    case .once:
      try runOnce()
    }
  }

  private static func runConnection() throws {
    let context = try ApplicationContext()
    guard let lock = context.store.acquireTransactionLock() else {
      context.logger.log("action=connect-target-skip reason=transaction-busy")
      print("connection=busy")
      return
    }

    try withExtendedLifetime(lock) {
      try ConnectionCoordinator(context: context).connectTarget()
    }
  }

  private static func runOnce() throws {
    let context = try ApplicationContext()
    guard let lock = context.store.acquireTransactionLock() else {
      context.logger.log("action=once-skip reason=transaction-busy")
      return
    }

    try withExtendedLifetime(lock) {
      try runOnce(context: context)
    }
  }

  private static func runOnce(context: ApplicationContext) throws {
    let current = context.probe.currentState()
    guard current.isAvailable else {
      return
    }

    let previous = context.store.readSignature()
    guard previous != current.signature else {
      return
    }
    try handleNetworkChange(context: context, current: current, previous: previous)
  }

  private static func handleNetworkChange(
    context: ApplicationContext,
    current: NetworkState,
    previous: String?
  ) throws {
    if let previous {
      Thread.sleep(forTimeInterval: Self.transitionSettleSeconds)
      let settled = context.probe.currentState()
      guard settled.isAvailable else {
        context.logger.log("action=resync-hold reason=network-not-settled")
        return
      }
      try context.store.saveSignature(settled.signature)
      context.logger.log("network-change previous=\(previous) current=\(settled.signature)")
      if settled.isReadyForResync {
        try runLightResync(context: context)
        try ConnectionCoordinator(context: context).evaluateTargetHealth(state: settled)
      } else {
        context.logger.log("action=light-resync-hold reason=network-not-settled")
      }
      return
    }

    try context.store.saveSignature(current.signature)
    context.logger.log("network-initial signature=\(current.signature)")
    try ConnectionCoordinator(context: context).evaluateTargetHealth(state: current)
  }

  private static func runLightResync(context: ApplicationContext) throws {
    guard context.store.lightActionIsAllowed() else {
      context.logger.log("action=light-resync-skip reason=cooldown cooldown=60s")
      return
    }

    try context.store.recordLightAction()
    context.resynchronizer.light().forEach { context.logger.log($0) }
  }

  private static func runFullResync() throws {
    let context = try ApplicationContext()
    context.resynchronizer.full().forEach { context.logger.log($0) }
  }
}

MacWiFiStabilityMain.main()
