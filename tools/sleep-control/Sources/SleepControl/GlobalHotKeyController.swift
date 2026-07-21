import Carbon.HIToolbox
import Combine
import SleepControlCore

/// Owns the single Carbon hot key registration for the app lifetime.
@MainActor
internal final class GlobalHotKeyController: ObservableObject {
  private static let hotKeySignature: OSType = 0x534C_5043
  private static let hotKeyIdentifier: UInt32 = 1
  private static let eventCount = 1
  private static let handleHotKey: EventHandlerUPP = { _, _, context in
    guard let context else {
      return OSStatus(eventNotHandledErr)
    }

    let controller = Unmanaged<GlobalHotKeyController>
      .fromOpaque(context)
      .takeUnretainedValue()
    Task { @MainActor in
      controller.performAction()
    }
    return noErr
  }

  /// Whether the selected shortcut is registered without a conflict.
  @Published internal private(set) var isRegistered = false

  private var hotKeyReference: EventHotKeyRef?
  private var eventHandlerReference: EventHandlerRef?
  private let action: @MainActor () -> Void

  internal init(action: @escaping @MainActor () -> Void) {
    self.action = action
  }

  /// Replaces the active registration with a new shortcut.
  internal func register(_ shortcut: SleepToggleShortcut) {
    guard installEventHandlerIfNeeded() else {
      isRegistered = false
      return
    }

    if let hotKeyReference {
      UnregisterEventHotKey(hotKeyReference)
      self.hotKeyReference = nil
    }

    var reference: EventHotKeyRef?
    let identifier = EventHotKeyID(
      signature: Self.hotKeySignature,
      id: Self.hotKeyIdentifier
    )
    let status = RegisterEventHotKey(
      shortcut.key.carbonKeyCode,
      shortcut.modifiers.carbonFlags,
      identifier,
      GetApplicationEventTarget(),
      0,
      &reference
    )
    hotKeyReference = reference
    isRegistered = status == noErr && reference != nil
  }

  private func installEventHandlerIfNeeded() -> Bool {
    guard eventHandlerReference == nil else {
      return true
    }

    var event = EventTypeSpec(
      eventClass: OSType(kEventClassKeyboard),
      eventKind: UInt32(kEventHotKeyPressed)
    )
    let context = Unmanaged.passUnretained(self).toOpaque()
    let status = InstallEventHandler(
      GetApplicationEventTarget(),
      Self.handleHotKey,
      Self.eventCount,
      &event,
      context,
      &eventHandlerReference
    )
    return status == noErr && eventHandlerReference != nil
  }

  private func performAction() {
    action()
  }
}
