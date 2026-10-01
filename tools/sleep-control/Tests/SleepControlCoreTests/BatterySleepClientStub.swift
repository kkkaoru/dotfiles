#if canImport(SleepControlCore)
  import SleepControlCore
#endif

@MainActor
internal final class BatterySleepClientStub: BatterySleepClient {
  private static let defaultCutoff = 50.0
  internal var reading = BatterySleepReading(lidIsClosed: true, batteryPercentage: defaultCutoff)
  internal var events: [String] = []
  internal var readFails = false
  internal var enableFails = false
  internal var sleepFails = false
  internal var afterEnable: (@MainActor () -> Void)?
  internal var duringRead: (@MainActor () async -> Void)?

  internal func read() async throws -> BatterySleepReading {
    events.append("read")
    await duringRead?()
    if readFails { throw TestError.readFailed }
    return reading
  }

  internal func enableSystemSleep() throws {
    events.append("enable")
    if enableFails { throw TestError.writeFailed }
    afterEnable?()
  }

  internal func sleepNow() throws {
    events.append("sleep")
    if sleepFails { throw TestError.writeFailed }
  }
}
