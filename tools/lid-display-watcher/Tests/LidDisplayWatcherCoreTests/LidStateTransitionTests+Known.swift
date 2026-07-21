extension LidStateTransitionTests {
  // Known-state pairs are checked as both startup observations and live events;
  // only an initial opening is allowed to suppress an otherwise required wake.
  internal static func testOpenToInitialOpen() {
    expectTransition(
      previous: false,
      lidIsClosed: false,
      isInitialState: true,
      expectedAction: nil,
      expectedState: false
    )
  }

  internal static func testOpenToEventOpen() {
    expectTransition(
      previous: false,
      lidIsClosed: false,
      isInitialState: false,
      expectedAction: nil,
      expectedState: false
    )
  }

  internal static func testOpenToInitialClosed() {
    expectTransition(
      previous: false,
      lidIsClosed: true,
      isInitialState: true,
      expectedAction: true,
      expectedState: true
    )
  }

  internal static func testOpenToEventClosed() {
    expectTransition(
      previous: false,
      lidIsClosed: true,
      isInitialState: false,
      expectedAction: true,
      expectedState: true
    )
  }

  internal static func testClosedToInitialOpen() {
    expectTransition(
      previous: true,
      lidIsClosed: false,
      isInitialState: true,
      expectedAction: nil,
      expectedState: false
    )
  }

  internal static func testClosedToEventOpen() {
    expectTransition(
      previous: true,
      lidIsClosed: false,
      isInitialState: false,
      expectedAction: false,
      expectedState: false
    )
  }

  internal static func testClosedToInitialClosed() {
    expectTransition(
      previous: true,
      lidIsClosed: true,
      isInitialState: true,
      expectedAction: nil,
      expectedState: true
    )
  }

  internal static func testClosedToEventClosed() {
    expectTransition(
      previous: true,
      lidIsClosed: true,
      isInitialState: false,
      expectedAction: nil,
      expectedState: true
    )
  }
}
