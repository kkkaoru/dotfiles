#if canImport(SleepControlCore)
  import SleepControlCore
#endif

import Foundation

extension SleepControlCoreTests {
  private static let beforeHold: TimeInterval = 99
  private static let holdStart: TimeInterval = 100
  private static let residualInput: TimeInterval = 101.9
  private static let firstDeliberateInput: TimeInterval = 102
  private static let laterInput: TimeInterval = 300
  private static let laterSleep: TimeInterval = 500

  internal static func testHotCornerHoldSleepsDisplaysAgainWithoutInput() {
    var state = holdingState()

    expect(state.isHolding)
    expect(state.shouldSleepDisplaysAfterWake(lastInputAt: beforeHold))
    state.observeDisplaySleep(triggeredByHotCorner: false, at: laterSleep)
    expect(state.isHolding)
    expect(state.shouldSleepDisplaysAfterWake(lastInputAt: beforeHold))
  }

  internal static func testHotCornerHoldIgnoresResidualPointerInput() {
    var state = holdingState()

    expect(state.shouldSleepDisplaysAfterWake(lastInputAt: residualInput))
    expect(state.isHolding)
  }

  internal static func testHotCornerHoldEndsOnUserInput() {
    var state = holdingState()

    expect(!state.shouldSleepDisplaysAfterWake(lastInputAt: firstDeliberateInput))
    expect(!state.isHolding)
    expect(!state.shouldSleepDisplaysAfterWake(lastInputAt: beforeHold))
  }

  internal static func testHotCornerHoldKeepsItsStartAcrossRepeatedSleeps() {
    var state = holdingState()
    state.observeDisplaySleep(triggeredByHotCorner: true, at: laterSleep)

    expect(!state.shouldSleepDisplaysAfterWake(lastInputAt: laterInput))
  }

  internal static func testHotCornerHoldIgnoresOtherDisplaySleeps() {
    var state = HotCornerDisplaySleepState()
    state.setEnabled(true)

    state.observeDisplaySleep(triggeredByHotCorner: false, at: holdStart)

    expect(!state.isHolding)
    expect(!state.shouldSleepDisplaysAfterWake(lastInputAt: beforeHold))
  }

  internal static func testHotCornerHoldRespectsTheSwitch() {
    var disabled = HotCornerDisplaySleepState()
    expect(!disabled.isEnabled)
    disabled.observeDisplaySleep(triggeredByHotCorner: true, at: holdStart)
    expect(!disabled.isHolding)

    var state = holdingState()
    state.setEnabled(false)
    expect(!state.isHolding)
    expect(!state.shouldSleepDisplaysAfterWake(lastInputAt: beforeHold))
  }

  internal static func testHotCornerHoldCanEndExplicitly() {
    var state = holdingState()

    state.endHold()

    expect(!state.isHolding)
    expect(!state.shouldSleepDisplaysAfterWake(lastInputAt: beforeHold))
  }

  internal static func testHotCornerHoldRecognizesTouchIDInput() {
    let prefix = "com.apple.iohideventsystem.queue.tickle"
    let touchID = "\(prefix) serviceID:1000005e4 service:AppleMesaShim product:(null) eventType:29"
    let sameEvent = "\(prefix) serviceID:0 service:(null) product:(null) eventType:29"
    let pointer = "\(prefix) serviceID:10002fe13 service:Pointer product:P v.0.2.0  eventType:17"

    expect(HotCornerDisplaySleepState.isBiometricInput(assertionName: touchID))
    expect(HotCornerDisplaySleepState.isBiometricInput(assertionName: sameEvent))
    expect(!HotCornerDisplaySleepState.isBiometricInput(assertionName: pointer))
    expect(!HotCornerDisplaySleepState.isBiometricInput(assertionName: "\(prefix) eventType:291"))
    expect(!HotCornerDisplaySleepState.isBiometricInput(assertionName: "other eventType:29"))
    let caffeinate = "caffeinate command-line tool"
    expect(!HotCornerDisplaySleepState.isBiometricInput(assertionName: caffeinate))
  }

  private static func holdingState() -> HotCornerDisplaySleepState {
    var state = HotCornerDisplaySleepState()
    state.setEnabled(true)
    state.observeDisplaySleep(triggeredByHotCorner: true, at: holdStart)
    return state
  }
}
