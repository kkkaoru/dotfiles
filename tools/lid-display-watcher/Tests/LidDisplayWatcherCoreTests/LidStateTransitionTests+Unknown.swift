extension LidStateTransitionTests {
  // nil is the not-yet-observed startup state. Event variants keep the pure
  // transition function total even though normal startup initializes it first.
  internal static func testUnknownToInitialOpen() {
    expectTransition(
      previous: nil,
      lidIsClosed: false,
      isInitialState: true,
      expectedAction: nil,
      expectedState: false
    )
  }

  internal static func testUnknownToEventOpen() {
    expectTransition(
      previous: nil,
      lidIsClosed: false,
      isInitialState: false,
      expectedAction: false,
      expectedState: false
    )
  }

  internal static func testUnknownToInitialClosed() {
    expectTransition(
      previous: nil,
      lidIsClosed: true,
      isInitialState: true,
      expectedAction: true,
      expectedState: true
    )
  }

  internal static func testUnknownToEventClosed() {
    expectTransition(
      previous: nil,
      lidIsClosed: true,
      isInitialState: false,
      expectedAction: true,
      expectedState: true
    )
  }
}
