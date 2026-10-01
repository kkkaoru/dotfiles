import AppKit

extension SleepControlSystemTests {
  internal static func testPowerEvents() async throws {
    let suite = "SleepControlEventTests.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: suite) else { throw TestError.readFailed }
    defer { defaults.removePersistentDomain(forName: suite) }
    let settings = ShortcutSettingsStore(defaults: defaults)
    let observer = BatterySleepEvents()
    defer { observer.stop() }
    try await testEventBurst(settings: settings, observer: observer)
    try await testSettingsAndWake(settings: settings, observer: observer)
    try await testRegistrationFailure(settings: settings, observer: observer)
  }

  private static func testEventBurst(
    settings: ShortcutSettingsStore, observer: BatterySleepEvents
  ) async throws {
    var iterator = observer.start(settings: settings).makeAsyncIterator()
    guard try await iterator.next() != nil else { throw TestError.readFailed }
    // A burst while a check is in flight retains only one follow-up check.
    BatterySleepEvents.powerCallback(nil)
    BatterySleepEvents.powerCallback(nil)
    NotificationCenter.default.post(name: BatterySleepEvents.lidChanged, object: nil)
    observer.stop()
    guard try await iterator.next() != nil, try await iterator.next() == nil else {
      throw TestError.readFailed
    }
  }

  private static func testSettingsAndWake(
    settings: ShortcutSettingsStore, observer: BatterySleepEvents
  ) async throws {
    var iterator = observer.start(settings: settings).makeAsyncIterator()
    guard try await iterator.next() != nil else { throw TestError.readFailed }
    settings.batterySleep.isEnabled = false
    observer.stop()
    guard try await iterator.next() != nil, try await iterator.next() == nil else {
      throw TestError.readFailed
    }
    iterator = observer.start(settings: settings).makeAsyncIterator()
    guard try await iterator.next() != nil else { throw TestError.readFailed }
    NSWorkspace.shared.notificationCenter.post(name: NSWorkspace.didWakeNotification, object: nil)
    observer.stop()
    guard try await iterator.next() != nil, try await iterator.next() == nil else {
      throw TestError.readFailed
    }
  }

  private static func testRegistrationFailure(
    settings: ShortcutSettingsStore, observer: BatterySleepEvents
  ) async throws {
    observer.makeSource = { nil }
    var iterator = observer.start(settings: settings).makeAsyncIterator()
    do {
      _ = try await iterator.next()
      throw TestError.readFailed
    } catch let error as CocoaError where error.code == .featureUnsupported {
      // Registration failure is surfaced, never replaced with a hidden polling loop.
    }
  }
}
