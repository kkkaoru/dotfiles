#if canImport(LidDisplayWatcherCore)
  import LidDisplayWatcherCore
#endif

// Values from the public IOPM, CoreFoundation, Mach, and signal headers are
// kept local so the release build does not import their Swift overlay modules.
private let clamshellMessage: UInt32 = 0xe003_4100
private let clamshellStateBit: UInt = 1
private let utf8Encoding: UInt32 = 0x0800_0100
private let machReceiveMessage: Int32 = 2
private let machMessageSuccess: Int32 = 0
// 32 pointer-sized words provide a 256-byte, correctly aligned receive buffer.
private let machMessageBufferWords = 32
private let failureStatus: Int32 = 1
private let childSignal: Int32 = 20
private let ignoreSignalHandler: UInt = 1

private let rootDomainClass: StaticString = "IOPMrootDomain"
private let generalInterest: StaticString = "IOGeneralInterest"
private let clamshellStateKey: StaticString = "AppleClamshellState"

// The callback runs synchronously on the single Mach receive loop. This is the
// only persistent mutable byte: nil before startup completes, then open or closed.
nonisolated(unsafe) private var previousLidState: Bool?

private func applyLidState(_ lidIsClosed: Bool, isInitialState: Bool) {
  guard
    let displayShouldSleep = requestedDisplaySleepState(
      previousLidState: &previousLidState,
      lidIsClosed: lidIsClosed,
      isInitialState: isInitialState
    )
  else {
    return
  }

  setDisplaySleeping(displayShouldSleep)
}

private func readLidState(service: IOServiceHandle, key: UnsafeRawPointer) -> Bool? {
  guard let value = ioRegistryEntryCreateCFProperty(service, key, nil, 0) else {
    return nil
  }
  let lidIsClosed = cfBooleanGetValue(value) != 0
  cfRelease(value)
  return lidIsClosed
}

private func notificationCallback(
  _: UnsafeMutableRawPointer?,
  _: IOServiceHandle,
  _ messageType: UInt32,
  _ messageArgument: UnsafeMutableRawPointer?
) {
  guard messageType == clamshellMessage else {
    return
  }
  let bits = messageArgument.map(UInt.init(bitPattern:)) ?? 0
  applyLidState(bits & clamshellStateBit != 0, isInitialState: false)
}

@inline(__always)
private func receiveNotification(
  message: UnsafeMutableRawPointer,
  size: UInt32,
  machPort: MachPortName,
  notificationPort: UnsafeMutableRawPointer
) -> Bool {
  let status = machMessage(message, machReceiveMessage, 0, size, machPort, 0, 0)
  guard status == machMessageSuccess else {
    return false
  }
  ioDispatchCalloutFromMessage(nil, message, notificationPort)
  return true
}

private func receiveNotifications(port: UnsafeMutableRawPointer) -> Int32 {
  let machPort = ioNotificationPortGetMachPort(port)
  guard machPort != 0 else {
    return failureStatus
  }

  // machMessage blocks inside the kernel until IOKit sends an event. There is
  // no timer, polling interval, or idle wakeup.
  return withUnsafeTemporaryAllocation(
    of: UInt.self,
    capacity: machMessageBufferWords
  ) { buffer in
    guard let message = buffer.baseAddress else {
      return failureStatus
    }
    let messageSize = UInt32(buffer.count * MemoryLayout<UInt>.stride)
    while receiveNotification(
      message: message,
      size: messageSize,
      machPort: machPort,
      notificationPort: port
    ) {
      // The blocking receive dispatches each callback before continuing.
    }
    return failureStatus
  }
}

private func findRootDomain() -> IOServiceHandle? {
  guard let matching = ioServiceMatching(cString(rootDomainClass)) else {
    return nil
  }
  let rootDomain = ioServiceGetMatchingService(0, matching)
  guard rootDomain != 0 else {
    return nil
  }
  return rootDomain
}

private func registerForNotifications(rootDomain: IOServiceHandle) -> UnsafeMutableRawPointer? {
  guard let port = ioNotificationPortCreate(0) else {
    return nil
  }

  // IOKit writes the registration token. The daemon retains it for its lifetime.
  var notifier: UInt32 = 0
  let status = ioServiceAddInterestNotification(
    port,
    rootDomain,
    cString(generalInterest),
    notificationCallback,
    nil,
    &notifier
  )
  guard status == 0 else {
    return nil
  }
  return port
}

private func applyInitialLidState(rootDomain: IOServiceHandle) -> Bool {
  guard
    let stateKey = cfStringCreateWithCString(nil, cString(clamshellStateKey), utf8Encoding)
  else {
    return false
  }

  let initialLidState = readLidState(service: rootDomain, key: stateKey)
  cfRelease(stateKey)
  guard let initialLidState else {
    return true
  }
  applyLidState(initialLidState, isInitialState: true)
  return true
}

internal func runLidDisplayWatcher() -> Int32 {
  _ = setSignalHandler(childSignal, ignoreSignalHandler)

  guard let rootDomain = findRootDomain() else {
    return failureStatus
  }
  guard let notificationPort = registerForNotifications(rootDomain: rootDomain) else {
    return failureStatus
  }
  guard applyInitialLidState(rootDomain: rootDomain) else {
    return failureStatus
  }

  return receiveNotifications(port: notificationPort)
}
