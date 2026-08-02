#if canImport(SleepControlCore)
  import SleepControlCore
#endif

extension SleepControlCoreTests {
  internal static func testLidDisplaySleepTransitions() {
    var state = LidDisplaySleepState()

    expect(state.observe(lidIsClosed: true) == nil)
    expect(state.setEnabled(true) == .sleep)
    expect(state.observe(lidIsClosed: true) == nil)
    expect(state.observe(lidIsClosed: false) == .wake)
    expect(state.observe(lidIsClosed: false) == nil)
    expect(state.setEnabled(false) == nil)
  }

  internal static func testLidDisplaySleepOnlyWakesOwnSleep() {
    var state = LidDisplaySleepState()

    expect(state.setEnabled(true) == nil)
    expect(state.observe(lidIsClosed: false) == nil)
    expect(state.observe(lidIsClosed: true) == .sleep)
    expect(state.setEnabled(false) == .wake)
    expect(state.observe(lidIsClosed: false) == nil)
  }

  internal static func testLidDisplaySleepIgnoresRepeatedEnableAndDisable() {
    var state = LidDisplaySleepState()

    expect(state.setEnabled(false) == nil)
    expect(state.setEnabled(true) == nil)
    expect(state.setEnabled(true) == nil)
    expect(state.observe(lidIsClosed: true) == .sleep)
    expect(state.setEnabled(false) == .wake)
    expect(state.setEnabled(false) == nil)
  }

  internal static func testLidDisplaySleepHandlesUnknownState() {
    var state = LidDisplaySleepState()

    expect(state.lidIsClosed == nil)
    expect(state.setEnabled(true) == nil)
    expect(state.observe(lidIsClosed: false) == nil)
    expect(state.lidIsClosed == false)
    expect(state.observe(lidIsClosed: true) == .sleep)
    expect(state.observe(lidIsClosed: true) == nil)
  }
}
