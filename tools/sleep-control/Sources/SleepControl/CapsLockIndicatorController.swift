import CoreFoundation
import CoreGraphics
import Foundation
import IOKit.hid
import SleepControlCore

private let capsLockKeyboardUsagePage: UInt32 = 0x07
private let capsLockKeyUsage: UInt32 = 0x39
private let capsLockReapplyDelayNanoseconds: UInt64 = 20_000_000
private let capsLockInputCallback: IOHIDValueCallback = { context, _, _, value in
  guard
    let context,
    IOHIDElementGetUsagePage(IOHIDValueGetElement(value)) == capsLockKeyboardUsagePage,
    IOHIDElementGetUsage(IOHIDValueGetElement(value)) == capsLockKeyUsage
  else {
    return
  }
  let controller = Unmanaged<CapsLockIndicatorController>
    .fromOpaque(context)
    .takeUnretainedValue()
  Task { @MainActor in
    // Let the keyboard driver finish its normal Caps Lock LED update first.
    try? await Task.sleep(nanoseconds: capsLockReapplyDelayNanoseconds)
    controller.reapplyIndicatorIfNeeded()
  }
}

/// Drives the built-in keyboard Caps Lock LED without changing modifier state.
@MainActor
internal final class CapsLockIndicatorController {
  private static let ledUsagePage: UInt32 = 0x08
  private static let capsLockLEDUsage: UInt32 = 0x02
  private static let hidOptions = IOOptionBits(kIOHIDOptionsTypeNone)
  private static var runLoopMode: CFString {
    RunLoop.Mode.common.rawValue as CFString
  }

  private var hidManager: IOHIDManager?
  private var desiredIndicatorValue: Bool?
  private let inputAccess = CapsLockInputAccess()

  /// Reflects the resolved sleep setting when the optional indicator is enabled.
  internal func update(sleepDisabled: Bool?, isEnabled: Bool) {
    guard isEnabled else {
      desiredIndicatorValue = nil
      restoreSystemCapsLockState()
      return
    }
    desiredIndicatorValue = shouldIlluminateCapsLockIndicator(
      sleepDisabled: sleepDisabled,
      isEnabled: true
    )
    setCapsLockLED(illuminated: desiredIndicatorValue == true)
  }

  /// Restores the hardware indicator to macOS's current logical Caps Lock state.
  internal func restoreSystemCapsLockState() {
    desiredIndicatorValue = nil
    guard
      hidManager != nil || IOHIDCheckAccess(kIOHIDRequestTypeListenEvent) == kIOHIDAccessTypeGranted
    else {
      return
    }
    let flags = CGEventSource.flagsState(.combinedSessionState)
    setCapsLockLED(illuminated: flags.contains(.maskAlphaShift))
  }

  /// Stops the HID callback before the app exits.
  internal func stop() {
    guard let manager = hidManager else {
      return
    }
    IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetMain(), Self.runLoopMode)
    _ = IOHIDManagerClose(manager, Self.hidOptions)
    hidManager = nil
  }

  internal func reapplyIndicatorIfNeeded() {
    guard let desiredIndicatorValue else {
      return
    }
    setCapsLockLED(illuminated: desiredIndicatorValue)
  }

  private func setCapsLockLED(illuminated: Bool) {
    guard let manager = openHIDManager() else {
      return
    }

    guard let devices = IOHIDManagerCopyDevices(manager) else {
      return
    }
    let deviceCount = CFSetGetCount(devices)
    var devicePointers = [UnsafeRawPointer?](repeating: nil, count: deviceCount)
    CFSetGetValues(devices, &devicePointers)

    for devicePointer in devicePointers {
      guard let devicePointer else {
        continue
      }
      let device = Unmanaged<IOHIDDevice>
        .fromOpaque(devicePointer)
        .takeUnretainedValue()
      guard IOHIDDeviceGetProperty(device, kIOHIDBuiltInKey as CFString) as? Bool == true else {
        continue
      }
      setCapsLockLED(on: device, illuminated: illuminated)
    }
  }

  private func openHIDManager() -> IOHIDManager? {
    if let hidManager {
      return hidManager
    }

    guard inputAccess.isGranted() else {
      return nil
    }

    let manager = IOHIDManagerCreate(kCFAllocatorDefault, Self.hidOptions)
    // A manager without an explicit matching dictionary returns an empty set
    // from `IOHIDManagerCopyDevices`. Nil means all HID devices here; the
    // built-in-device and LED-element checks below narrow the write safely.
    IOHIDManagerSetDeviceMatching(manager, nil)
    IOHIDManagerSetInputValueMatching(manager, nil)
    let context = Unmanaged.passUnretained(self).toOpaque()
    IOHIDManagerRegisterInputValueCallback(manager, capsLockInputCallback, context)
    IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), Self.runLoopMode)
    let openStatus = IOHIDManagerOpen(manager, Self.hidOptions)
    guard openStatus == kIOReturnSuccess else {
      IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetMain(), Self.runLoopMode)
      return nil
    }
    hidManager = manager
    return manager
  }

  private func setCapsLockLED(on device: IOHIDDevice, illuminated: Bool) {
    guard let elements = IOHIDDeviceCopyMatchingElements(device, nil, Self.hidOptions) else {
      return
    }

    for index in 0..<CFArrayGetCount(elements) {
      let element = Unmanaged<IOHIDElement>
        .fromOpaque(CFArrayGetValueAtIndex(elements, index))
        .takeUnretainedValue()
      guard
        IOHIDElementGetType(element) == kIOHIDElementTypeOutput,
        IOHIDElementGetUsagePage(element) == Self.ledUsagePage,
        IOHIDElementGetUsage(element) == Self.capsLockLEDUsage
      else {
        continue
      }

      let value = IOHIDValueCreateWithIntegerValue(
        kCFAllocatorDefault,
        element,
        0,
        illuminated ? 1 : 0
      )
      _ = IOHIDDeviceSetValue(device, element, value)
    }
  }
}
