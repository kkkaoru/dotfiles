import AppKit
import Combine
import IOKit.ps
#if canImport(SleepControlCore)
  import SleepControlCore
#endif

/// App-owned native notifications with no timer and at most one pending sensor check.
@MainActor
internal final class BatterySleepEvents {
  nonisolated internal static let powerChanged = Notification.Name("SleepControl.powerChanged")
  nonisolated internal static let lidChanged = Notification.Name("SleepControl.lidChanged")

  nonisolated internal static let powerCallback: IOPowerSourceCallbackType = { _ in
    NotificationCenter.default.post(name: BatterySleepEvents.powerChanged, object: nil)
  }

  internal var makeSource: () -> CFRunLoopSource? = {
    IOPSNotificationCreateRunLoopSource(BatterySleepEvents.powerCallback, nil)?.takeRetainedValue()
  }

  private var source: CFRunLoopSource?
  private var subscriptions = Set<AnyCancellable>()
  private var continuation: AsyncThrowingStream<Void, any Error>.Continuation?

  internal func start(settings: ShortcutSettingsStore) -> AsyncThrowingStream<Void, any Error> {
    stop()
    let (events, sink) = AsyncThrowingStream<Void, any Error>.makeStream(
      bufferingPolicy: .bufferingNewest(1)
    )
    continuation = sink
    // User-approved C boundary: no context pointer or captured object; consume the owned source.
    guard let newSource = makeSource() else {
      sink.finish(throwing: CocoaError(.featureUnsupported))
      return events
    }
    source = newSource
    NotificationCenter.default.publisher(for: Self.powerChanged)
      .merge(with: NotificationCenter.default.publisher(for: Self.lidChanged))
      .merge(
        with: NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.didWakeNotification)
      )
      .sink { _ in sink.yield(()) }
      .store(in: &subscriptions)
    settings.$batterySleep
      .sink { _ in sink.yield(()) }
      .store(in: &subscriptions)
    CFRunLoopAddSource(CFRunLoopGetMain(), newSource, .commonModes)
    // Published settings emits its initial value, covering launch without a separate check.
    return events
  }

  internal func stop() {
    if let source {
      CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes)
      CFRunLoopSourceInvalidate(source)
    }
    source = nil
    subscriptions.removeAll()
    continuation?.finish()
    continuation = nil
  }
}
