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
    runParserTests()
    runModelTests()
    runSettingsTests()
    runShortcutTests()
    runBehaviorSpecTests()
    print("Swift tests: 40 passed")
  }

  private static func runParserTests() {
    testParsesEnabled()
    testParsesDisabled()
    testIgnoresSimilarKey()
    testRejectsInvalidValue()
    testRejectsExtraFields()
    testHandlesMissingSetting()
  }

  private static func runModelTests() {
    testRefreshLoadsCurrentState()
    testRefreshExposesReadError()
    testUpdateStoresAuthenticatedSetting()
    testUpdatePreservesStateAfterFailure()
    testUpdateIgnoresUnchangedValue()
    testToggleReversesLoadedState()
    testToggleLoadsMissingState()
    testToggleStopsAfterReadFailure()
    testUpdateCanInitializeMissingState()
  }

  private static func runSettingsTests() {
    testUnifiedSettingsDefaults()
    testUnifiedSettingsPersistence()
    testShortcutDefaults()
    testShortcutPersistence()
    testShortcutInvalidPersistedValuesFallBackIndependently()
    testShortcutPersistsEveryCombination()
    testShortcutFallsBackWhenKeyIsMissing()
    testShortcutFallsBackWhenModifiersAreMissing()
    testShortcutFallsBackWhenValuesAreNotStrings()
    testShortcutWritesRawValuesImmediately()
    testBooleanSettingsFallBackToLegacyKeys()
    testBooleanSettingsPreferCurrentKeyOverLegacy()
    testBooleanSettingsIgnoreNonBooleanLegacyValues()
    testBooleanSettingsWriteThroughRawValues()
    testBooleanSettingsExposeBackwardCompatibleNames()
  }

  private static func runShortcutTests() {
    testShortcutPresentation()
    testShortcutDisplayNamesComposePickerLabels()
    testKeyDisplayNamesCoverAlphabet()
    testModifierDisplayNamesAreDistinctGlyphs()
    testDefaultShortcutIsComposed()
  }

  private static func runBehaviorSpecTests() {
    testCapsLockLEDLightsOnlyWhenEnabledAndSleepDisabled()
    testLidDisplaySleepTransitions()
    testLidDisplaySleepOnlyWakesOwnSleep()
    testLidDisplaySleepIgnoresRepeatedEnableAndDisable()
    testLidDisplaySleepHandlesUnknownState()
  }
}
