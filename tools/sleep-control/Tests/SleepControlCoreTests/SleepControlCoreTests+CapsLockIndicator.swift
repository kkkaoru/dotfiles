#if canImport(SleepControlCore)
  import SleepControlCore
#endif

extension SleepControlCoreTests {
  internal static func testCapsLockLEDLightsOnlyWhenEnabledAndSleepDisabled() {
    expect(shouldIlluminateCapsLockIndicator(sleepDisabled: true, isEnabled: true))
    expect(!shouldIlluminateCapsLockIndicator(sleepDisabled: true, isEnabled: false))
    expect(!shouldIlluminateCapsLockIndicator(sleepDisabled: false, isEnabled: true))
    expect(!shouldIlluminateCapsLockIndicator(sleepDisabled: false, isEnabled: false))
    expect(!shouldIlluminateCapsLockIndicator(sleepDisabled: nil, isEnabled: true))
    expect(!shouldIlluminateCapsLockIndicator(sleepDisabled: nil, isEnabled: false))
  }
}
