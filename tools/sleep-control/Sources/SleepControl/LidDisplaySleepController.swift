import Foundation
import IOKit
import IOKit.pwr_mgt
import SleepControlCore

/// Watches public IOKit clamshell notifications without polling.
internal final class LidDisplaySleepController {
  private static let clamshellMessageValue: UInt32 = 0xe003_4100
  private static let clamshellMessage = natural_t(clamshellMessageValue)
  private static let clamshellStateBit: UInt = 1

  private static let callback: IOServiceInterestCallback = { context, _, messageType, argument in
    guard let context, messageType == LidDisplaySleepController.clamshellMessage else {
      return
    }
    let messageBits = argument.map(UInt.init(bitPattern:)) ?? 0
    let isClosed = messageBits & LidDisplaySleepController.clamshellStateBit != 0
    let controller = Unmanaged<LidDisplaySleepController>
      .fromOpaque(context)
      .takeUnretainedValue()
    controller.apply(lidIsClosed: isClosed)
  }

  private var state = LidDisplaySleepState()
  private var notificationPort: IONotificationPortRef?
  private var notifier: io_object_t = 0
  private var rootDomain: io_service_t = 0

  internal init() {
    start()
  }

  internal func setEnabled(_ enabled: Bool) {
    apply(action: state.setEnabled(enabled))
    if enabled {
      // A controller can be created before the registry has exposed its first
      // clamshell state. Reading again makes enabling deterministic in that case.
      readInitialLidState()
    }
  }

  private func start() {
    guard let matching = IOServiceMatching("IOPMrootDomain") else {
      return
    }
    let service = IOServiceGetMatchingService(kIOMainPortDefault, matching)
    guard service != 0, let port = IONotificationPortCreate(kIOMainPortDefault) else {
      return
    }
    var newNotifier: io_object_t = 0
    let context = Unmanaged.passUnretained(self).toOpaque()
    let status = IOServiceAddInterestNotification(
      port,
      service,
      kIOGeneralInterest,
      Self.callback,
      context,
      &newNotifier
    )
    guard status == KERN_SUCCESS else {
      IONotificationPortDestroy(port)
      IOObjectRelease(service)
      return
    }
    rootDomain = service
    notificationPort = port
    notifier = newNotifier
    if let source = IONotificationPortGetRunLoopSource(port)?.takeUnretainedValue() {
      CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
    }
    readInitialLidState()
  }

  private func readInitialLidState() {
    guard rootDomain != 0 else {
      return
    }
    guard
      let value = IORegistryEntryCreateCFProperty(
        rootDomain,
        "AppleClamshellState" as CFString,
        kCFAllocatorDefault,
        0
      )?.takeRetainedValue() as? Bool
    else {
      return
    }
    apply(lidIsClosed: value)
  }

  private func apply(lidIsClosed: Bool) {
    apply(action: state.observe(lidIsClosed: lidIsClosed))
  }

  private func apply(action: LidDisplaySleepAction?) {
    guard let action else {
      return
    }
    DisplayPower.setSleeping(action == .sleep)
  }
}
