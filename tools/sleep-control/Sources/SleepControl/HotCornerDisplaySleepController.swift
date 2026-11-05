import AppKit
import IOKit
import IOKit.pwr_mgt
import OSLog
import SleepControlCore

/// Keeps displays off after a hot-corner display sleep until the user provides input.
@MainActor
internal final class HotCornerDisplaySleepController: NSObject {
  private static let nanosecondsPerSecond = 1e9
  private static let screenUnlocked = Notification.Name("com.apple.screenIsUnlocked")
  private static let logger = Logger(
    subsystem: "com.kkkaoru.sleep-control",
    category: "hot-corner"
  )

  private var state = HotCornerDisplaySleepState()
  private let dockDefaults = UserDefaults(
    suiteName: HotCornerDisplaySleepSetup.dockDefaultsSuiteName
  )

  override internal init() {
    super.init()
    // AppKit may suspend distributed notifications while the app is inactive, and a menu-bar app
    // usually is, so this needs immediate delivery rather than a Combine publisher.
    DistributedNotificationCenter.default().addObserver(
      self,
      selector: #selector(screenDidUnlock),
      name: Self.screenUnlocked,
      object: nil,
      suspensionBehavior: .deliverImmediately
    )
  }

  private static func hidIdleTime() -> TimeInterval? {
    guard let matching = IOServiceMatching("IOHIDSystem") else {
      return nil
    }
    let service = IOServiceGetMatchingService(kIOMainPortDefault, matching)
    guard service != 0 else {
      return nil
    }
    defer { IOObjectRelease(service) }
    let value =
      IORegistryEntryCreateCFProperty(
        service,
        "HIDIdleTime" as CFString,
        kCFAllocatorDefault,
        0
      )?.takeRetainedValue() as? UInt64
    return value.map { nanoseconds in Double(nanoseconds) / nanosecondsPerSecond }
  }

  private static func activeUserAssertionNames() -> [String] {
    var assertions: Unmanaged<CFDictionary>?
    guard
      IOPMCopyAssertionsByProcess(&assertions) == kIOReturnSuccess,
      let byProcess = assertions?.takeRetainedValue() as? [Int: [[String: NSObject]]]
    else {
      return []
    }
    return byProcess.values.joined().compactMap { assertion in
      assertion["AssertName"] as? String
    }
  }

  internal func setEnabled(_ enabled: Bool) {
    state.setEnabled(enabled)
  }

  internal func displaysDidSleep() {
    let wasHolding = state.isHolding
    state.observeDisplaySleep(
      triggeredByHotCorner: isCursorInDisplaySleepCorner(),
      at: ProcessInfo.processInfo.systemUptime
    )
    if state.isHolding, !wasHolding {
      Self.logger.notice("Holding displays off after a hot-corner display sleep")
    }
  }

  internal func displaysDidWake() {
    guard state.isHolding else {
      return
    }
    // A finger on Touch ID is deliberate even right after the hot corner, and re-sleeping here
    // would blank the display while the system authenticates it.
    let assertionNames = Self.activeUserAssertionNames()
    if assertionNames.contains(where: HotCornerDisplaySleepState.isBiometricInput) {
      Self.logger.notice("Touch ID woke displays; ending the hold")
      endHold()
      return
    }
    // Both ages count only real input: power assertions such as `caffeinate -u` do not reset them.
    // The input that wakes a display can reach the event age only after this notification.
    guard let anyInput = CGEventType(rawValue: ~0) else {
      endHold()
      return
    }
    let eventAge = CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: anyInput)
    let idleTime = Self.hidIdleTime()
    let lastInputAt = ProcessInfo.processInfo.systemUptime - min(eventAge, idleTime ?? eventAge)
    Self.logger.notice("Displays woke: event age \(eventAge) s, idle \(idleTime ?? -1) s")
    if state.shouldSleepDisplaysAfterWake(lastInputAt: lastInputAt) {
      Self.logger.notice("Displays woke without user input; sleeping them again")
      DisplayPower.setSleeping(true)
    } else {
      Self.logger.notice("User input woke displays; ending the hold")
    }
  }

  /// Ends the hold for wakes without a reliable input event: the key press that wakes a sleeping
  /// Mac is not always delivered as an event, and opening the lid is not an input event at all.
  internal func endHold() {
    state.endHold()
  }

  /// Any unlock, including Touch ID, proves the user is present even without an input event.
  @objc
  private func screenDidUnlock() {
    Self.logger.notice("Screen unlocked; ending any hold and waking displays")
    endHold()
    DisplayPower.setSleeping(false)
  }

  private func isCursorInDisplaySleepCorner() -> Bool {
    guard let dockDefaults else {
      return false
    }
    return HotCornerDisplaySleepSetup(defaults: dockDefaults).isTriggered(
      cursor: NSEvent.mouseLocation,
      screens: NSScreen.screens.map(\.frame),
      modifiers: NSEvent.modifierFlags.rawValue
    )
  }
}
