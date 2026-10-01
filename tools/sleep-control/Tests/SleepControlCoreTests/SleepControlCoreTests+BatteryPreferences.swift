#if canImport(SleepControlCore)
  import SleepControlCore
#endif

extension SleepControlCoreTests {
  internal static func testBatteryPreferences() throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    let settings = context.settings
    let defaultCutoff = 50.0
    let upperCutoff = 90.0
    let invalidCutoff = 55.0
    let choices = BatterySleepThreshold.allCases.map { String($0.rawValue) }.joined(separator: ",")
    expect(settings.batterySleep.isEnabled)
    expect(settings.batterySleepThresholdPercentage == defaultCutoff)
    expect(choices == "0,10,20,30,40,50,60,70,80,90")
    settings.batterySleepThresholdPercentage = 0
    expect(ShortcutSettingsStore(defaults: context.defaults).batterySleep.threshold == .percent00)
    settings.batterySleepThresholdPercentage = upperCutoff
    expect(ShortcutSettingsStore(defaults: context.defaults).batterySleep.threshold == .percent90)
    settings.batterySleep.isEnabled = false
    expect(!ShortcutSettingsStore(defaults: context.defaults).batterySleep.isEnabled)
    settings.batterySleepThresholdPercentage = .nan
    settings.batterySleepThresholdPercentage = invalidCutoff
    expect(settings.batterySleepThresholdPercentage == upperCutoff)
    context.defaults.set("55", forKey: "batterySleep.threshold")
    expect(ShortcutSettingsStore(defaults: context.defaults).batterySleep.threshold == .percent50)
    context.defaults.set("invalid", forKey: "batterySleep.threshold")
    expect(ShortcutSettingsStore(defaults: context.defaults).batterySleep.threshold == .percent50)
    context.defaults.set(false, forKey: "batterySleep.threshold")
    expect(ShortcutSettingsStore(defaults: context.defaults).batterySleep.threshold == .percent50)
  }

  internal static func testBatteryCutoffs() throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    let settings = context.settings.batterySleep
    let cutoff = 50.0
    let justAbove = 50.1
    let excessive = 101.0
    let atCutoff = BatterySleepReading(lidIsClosed: true, batteryPercentage: cutoff)
    let aboveCutoff = BatterySleepReading(lidIsClosed: true, batteryPercentage: justAbove)
    let openLid = BatterySleepReading(lidIsClosed: false, batteryPercentage: 0)
    let unknownLid = BatterySleepReading(lidIsClosed: nil, batteryPercentage: 0)
    let unknownBattery = BatterySleepReading(lidIsClosed: true, batteryPercentage: nil)
    let negativeBattery = BatterySleepReading(lidIsClosed: true, batteryPercentage: -1)
    let excessiveBattery = BatterySleepReading(lidIsClosed: true, batteryPercentage: excessive)
    let invalidBattery = BatterySleepReading(lidIsClosed: true, batteryPercentage: .nan)
    expect(atCutoff.requiresSleep(settings: settings))
    expect(!aboveCutoff.requiresSleep(settings: settings))
    expect(!openLid.requiresSleep(settings: settings))
    expect(!unknownLid.requiresSleep(settings: settings))
    expect(!unknownBattery.requiresSleep(settings: settings))
    expect(negativeBattery.batteryPercentage == nil)
    expect(excessiveBattery.batteryPercentage == nil)
    expect(invalidBattery.batteryPercentage == nil)
    try testZeroBatteryCutoff()
  }

  private static func testZeroBatteryCutoff() throws {
    let context = try BatterySleepTestContext()
    defer { context.cleanUp() }
    let emptyBattery = BatterySleepReading(lidIsClosed: true, batteryPercentage: 0)
    let onePercent = BatterySleepReading(lidIsClosed: true, batteryPercentage: 1)
    context.settings.batterySleep.threshold = .percent00
    expect(emptyBattery.requiresSleep(settings: context.settings.batterySleep))
    expect(!onePercent.requiresSleep(settings: context.settings.batterySleep))
    context.settings.batterySleep.isEnabled = false
    expect(!emptyBattery.requiresSleep(settings: context.settings.batterySleep))
  }
}
