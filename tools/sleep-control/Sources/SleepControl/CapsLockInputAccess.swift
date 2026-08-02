import AppKit
import IOKit.hid

/// Requests the input-monitoring access required by the HID manager.
@MainActor
internal final class CapsLockInputAccess {
  private var didRequest = false
  private var didOpenSettings = false

  internal func isGranted() -> Bool {
    let access = IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)
    guard access != kIOHIDAccessTypeGranted else {
      return true
    }
    guard !didRequest else {
      return false
    }
    didRequest = true
    let granted = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
    if !granted {
      openSettingsIfNeeded()
    }
    return granted
  }

  private func openSettingsIfNeeded() {
    guard !didOpenSettings else {
      return
    }
    didOpenSettings = true
    guard
      let url = URL(
        string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
      )
    else {
      return
    }
    DispatchQueue.main.async {
      NSWorkspace.shared.open(url)
    }
  }
}
