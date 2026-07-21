#if canImport(LidDisplayWatcherCore)
  import LidDisplayWatcherCore
#endif

@main
internal enum LidStateTransitionTests {
  internal static func expect(_ condition: @autoclosure () -> Bool) {
    precondition(condition())
  }

  internal static func expectTransition(
    previous initialState: Bool?,
    lidIsClosed: Bool,
    isInitialState: Bool,
    expectedAction: Bool?,
    expectedState: Bool
  ) {
    // Check the requested side effect and the inout state independently. A
    // transition is incomplete if either result is wrong.
    var previousState = initialState
    let action = requestedDisplaySleepState(
      previousLidState: &previousState,
      lidIsClosed: lidIsClosed,
      isInitialState: isInitialState
    )
    expect(action == expectedAction)
    expect(previousState == expectedState)
  }

  private static func testSequentialEvents() {
    // The truth-table tests isolate inputs; this case verifies that each stored
    // state becomes the next event's previous state.
    var previousState: Bool? = false
    let expectedActions: [Bool?] = [nil, true, nil, false, nil]
    let lidStates = [false, true, true, false, false]

    for (lidIsClosed, expectedAction) in zip(lidStates, expectedActions) {
      let action = requestedDisplaySleepState(
        previousLidState: &previousState,
        lidIsClosed: lidIsClosed,
        isInitialState: false
      )
      expect(action == expectedAction)
      expect(previousState == lidIsClosed)
    }
  }

  internal static func main() {
    testUnknownToInitialOpen()
    testUnknownToEventOpen()
    testUnknownToInitialClosed()
    testUnknownToEventClosed()
    testOpenToInitialOpen()
    testOpenToEventOpen()
    testOpenToInitialClosed()
    testOpenToEventClosed()
    testClosedToInitialOpen()
    testClosedToEventOpen()
    testClosedToInitialClosed()
    testClosedToEventClosed()
    testSequentialEvents()
    print("Swift tests: 13 passed")
  }
}
