@main
@MainActor
internal enum SleepControlCoreTests {
  // Source-location defaults make assertion failures identify their call site.
  // swiftlint:disable discouraged_default_parameter
  internal static func expect(
    _ condition: @autoclosure () -> Bool,
    file: StaticString = #fileID,
    line: UInt = #line
  ) {
    precondition(condition(), file: file, line: line)
  }
  // swiftlint:enable discouraged_default_parameter

  internal static func main() {
    testParsesEnabled()
    testParsesDisabled()
    testIgnoresSimilarKey()
    testRejectsInvalidValue()
    testRejectsExtraFields()
    testHandlesMissingSetting()
    testRefreshLoadsCurrentState()
    testRefreshExposesReadError()
    testUpdateStoresAuthenticatedSetting()
    testUpdatePreservesStateAfterFailure()
    testUpdateIgnoresUnchangedValue()
    testToggleReversesLoadedState()
    testToggleLoadsMissingState()
    testToggleStopsAfterReadFailure()
    testShortcutDefaults()
    testShortcutPersistence()
    testShortcutPresentation()
    print("Swift tests: 17 passed")
  }
}
