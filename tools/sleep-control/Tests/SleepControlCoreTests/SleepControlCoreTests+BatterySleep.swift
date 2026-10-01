#if canImport(SleepControlCore)
  import SleepControlCore
#endif

extension SleepControlCoreTests {
  internal static func runBatterySleepTests() async throws {
    // Read-only integration coverage of the app's shared-defaults initializer; never writes it.
    let sharedSettings = ShortcutSettingsStore()
    expect(BatterySleepThreshold.allCases.contains(sharedSettings.batterySleep.threshold))
    try testBatteryPreferences()
    try testBatteryCutoffs()
    try testBatterySensorPlists()
    try await testBatterySleepOrderAndRecheck()
    try await testBatterySleepDisabledAndReadFailure()
    try await testBatterySleepWriteFailures()
    try await testBatterySleepRevalidation()
    try await testBatterySleepCancellationAndOverlap()
    try await testBatteryMonitorStopsOnCancellation()
    try await testBatteryMonitorEvents()
  }

  private static func testBatterySleepOrderAndRecheck() async throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    let belowCutoff = 30.0
    await context.controller.check()
    expect(context.client.events == ["read", "enable", "refresh", "read", "sleep"])
    context.client.events = []
    context.client.reading = BatterySleepReading(lidIsClosed: false, batteryPercentage: belowCutoff)
    await context.controller.check()
    expect(context.client.events == ["read"])
    context.client.events = []
    context.client.reading = BatterySleepReading(lidIsClosed: true, batteryPercentage: belowCutoff)
    await context.controller.check()
    expect(context.client.events == ["read", "enable", "refresh", "read", "sleep"])
  }

  private static func testBatterySleepDisabledAndReadFailure() async throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    context.settings.batterySleep.isEnabled = false
    await context.controller.check()
    expect(context.client.events.isEmpty)
    context.settings.batterySleep.isEnabled = true
    context.client.readFails = true
    await context.controller.check()
    expect(context.client.events == ["read"])
    expect(context.controller.errorMessage == "read failed")
    context.settings.batterySleep.isEnabled = false
    await context.controller.check()
    expect(context.controller.errorMessage == nil)
  }

  private static func testBatterySleepWriteFailures() async throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    context.client.enableFails = true
    await context.controller.check()
    expect(context.client.events == ["read", "enable"])
    expect(context.controller.errorMessage == "write failed")
    context.client.events = []
    context.client.enableFails = false
    context.client.sleepFails = true
    await context.controller.check()
    expect(context.client.events == ["read", "enable", "refresh", "read", "sleep"])
    expect(context.controller.errorMessage == "write failed")
    context.client.sleepFails = false
    await context.controller.check()
    expect(context.controller.errorMessage == nil)
    expect(context.client.events.filter { $0 == "sleep" } == ["sleep", "sleep"])
  }

  private static func testBatterySleepRevalidation() async throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    let belowCutoff = 20.0
    context.client.afterEnable = {
      context.settings.batterySleep.isEnabled = false
    }
    defer { context.client.afterEnable = nil }
    await context.controller.check()
    expect(context.client.events == ["read", "enable", "refresh", "read"])
    context.settings.batterySleep.isEnabled = true
    context.client.afterEnable = {
      context.client.reading = BatterySleepReading(
        lidIsClosed: false,
        batteryPercentage: belowCutoff
      )
    }
    await context.controller.check()
    expect(!context.client.events.contains("sleep"))
  }

  private static func testBatteryMonitorStopsOnCancellation() async throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    var monitor: Task<Void, Never>?
    context.client.duringRead = { monitor?.cancel() }
    defer { context.client.duringRead = nil }
    let (events, continuation) = AsyncThrowingStream<Void, any Error>.makeStream()
    continuation.yield(())
    monitor = Task { await context.controller.monitor(events: events) }
    await monitor?.value
    expect(context.client.events == ["read"])
  }

  private static func testBatteryMonitorEvents() async throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    context.client.reading = BatterySleepReading(lidIsClosed: false, batteryPercentage: nil)
    let (events, continuation) = AsyncThrowingStream<Void, any Error>.makeStream(
      bufferingPolicy: .bufferingNewest(1)
    )
    continuation.yield(())
    context.client.duringRead = {
      continuation.yield(())
      continuation.yield(())
      continuation.finish()
    }
    defer { context.client.duringRead = nil }
    await context.controller.monitor(events: events)
    expect(context.client.events == ["read", "read"])
    let failed = AsyncThrowingStream<Void, any Error> { $0.finish(throwing: TestError.readFailed) }
    await context.controller.monitor(events: failed)
    expect(context.controller.errorMessage == "read failed")
    let cancelled = AsyncThrowingStream<Void, any Error> { sink in
      sink.finish(throwing: CancellationError())
    }
    await context.controller.monitor(events: cancelled)
    expect(context.controller.errorMessage == "read failed")
  }

  private static func testBatterySleepCancellationAndOverlap() async throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    context.client.duringRead = { await context.controller.check() }
    defer { context.client.duringRead = nil }
    let task = Task { await context.controller.check() }
    task.cancel()
    await task.value
    expect(context.client.events == ["read"])
    expect(context.controller.errorMessage == nil)
  }
}
