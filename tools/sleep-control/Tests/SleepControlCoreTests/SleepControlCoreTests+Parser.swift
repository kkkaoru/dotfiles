#if canImport(SleepControlCore)
  import SleepControlCore
#endif

extension SleepControlCoreTests {
  internal static func testParsesEnabled() {
    expect(
      parseSleepDisabled(from: "System-wide power settings:\n SleepDisabled\t\t1") == true
    )
  }

  internal static func testParsesDisabled() {
    expect(
      parseSleepDisabled(from: "System-wide power settings:\n SleepDisabled 0") == false
    )
  }

  internal static func testIgnoresSimilarKey() {
    expect(parseSleepDisabled(from: "OtherSleepDisabled 1") == nil)
  }

  internal static func testRejectsInvalidValue() {
    expect(parseSleepDisabled(from: "SleepDisabled 2") == nil)
  }

  internal static func testRejectsExtraFields() {
    expect(parseSleepDisabled(from: "SleepDisabled 1 unexpected") == nil)
  }

  internal static func testHandlesMissingSetting() {
    expect(parseSleepDisabled(from: "Currently in use:\n sleep 1") == nil)
  }
}
