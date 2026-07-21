internal typealias CChar = Int8
internal typealias IOServiceHandle = UInt32
internal typealias MachPortName = UInt32

internal typealias InterestCallback =
  @convention(c) (
    UnsafeMutableRawPointer?, IOServiceHandle, UInt32, UnsafeMutableRawPointer?
  ) -> Void

// Direct FFI keeps the release executable independent of SwiftCore, Foundation,
// and imported Darwin wrappers. These signatures mirror the public macOS headers.

// MARK: - CoreFoundation

@_silgen_name("CFBooleanGetValue")
internal func cfBooleanGetValue(_ value: UnsafeRawPointer) -> UInt8

@_silgen_name("CFRelease")
internal func cfRelease(_ value: UnsafeRawPointer)

@_silgen_name("CFStringCreateWithCString")
internal func cfStringCreateWithCString(
  _ allocator: UnsafeRawPointer?,
  _ string: UnsafePointer<CChar>,
  _ encoding: UInt32
) -> UnsafeRawPointer?

// MARK: - IOKit

@_silgen_name("IODispatchCalloutFromMessage")
internal func ioDispatchCalloutFromMessage(
  _ unused: UnsafeMutableRawPointer?,
  _ message: UnsafeMutableRawPointer,
  _ reference: UnsafeMutableRawPointer
)

@_silgen_name("IONotificationPortCreate")
internal func ioNotificationPortCreate(_ mainPort: MachPortName) -> UnsafeMutableRawPointer?

@_silgen_name("IONotificationPortGetMachPort")
internal func ioNotificationPortGetMachPort(_ port: UnsafeMutableRawPointer) -> MachPortName

@_silgen_name("IORegistryEntryCreateCFProperty")
internal func ioRegistryEntryCreateCFProperty(
  _ entry: IOServiceHandle,
  _ key: UnsafeRawPointer,
  _ allocator: UnsafeRawPointer?,
  _ options: UInt32
) -> UnsafeRawPointer?

@_silgen_name("IOServiceAddInterestNotification")
internal func ioServiceAddInterestNotification(
  _ port: UnsafeMutableRawPointer,
  _ service: IOServiceHandle,
  _ interestType: UnsafePointer<CChar>,
  _ callback: InterestCallback,
  _ context: UnsafeMutableRawPointer?,
  _ notifier: UnsafeMutablePointer<UInt32>
) -> Int32

@_silgen_name("IOServiceGetMatchingService")
internal func ioServiceGetMatchingService(
  _ mainPort: MachPortName,
  _ matching: UnsafeMutableRawPointer
) -> IOServiceHandle

@_silgen_name("IOServiceMatching")
internal func ioServiceMatching(_ name: UnsafePointer<CChar>) -> UnsafeMutableRawPointer?

// MARK: - libSystem

@_silgen_name("mach_msg")
internal func machMessage(
  _ message: UnsafeMutableRawPointer,
  _ options: Int32,
  _ sendSize: UInt32,
  _ receiveSize: UInt32,
  _ receivePort: MachPortName,
  _ timeout: UInt32,
  _ notificationPort: MachPortName
) -> Int32

@_silgen_name("_exit")
internal func processExit(_ status: Int32) -> Never

@_silgen_name("posix_spawn")
internal func spawn(
  _ processID: UnsafeMutablePointer<Int32>,
  _ path: UnsafePointer<CChar>,
  _ fileActions: UnsafeRawPointer?,
  _ attributes: UnsafeRawPointer?,
  _ arguments: UnsafePointer<UnsafeMutablePointer<CChar>?>,
  _ environment: UnsafePointer<UnsafeMutablePointer<CChar>?>
) -> Int32

@_silgen_name("signal")
internal func setSignalHandler(_ signal: Int32, _ handler: UInt) -> UInt

/// Returns the null-terminated UTF-8 storage emitted for a string literal.
@inline(__always)
internal func cString(_ value: StaticString) -> UnsafePointer<CChar> {
  UnsafeRawPointer(value.utf8Start).assumingMemoryBound(to: CChar.self)
}
