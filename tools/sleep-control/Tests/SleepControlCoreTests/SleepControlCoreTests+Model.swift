#if canImport(SleepControlCore)
  import SleepControlCore
#endif

extension SleepControlCoreTests {
  internal static func testRefreshLoadsCurrentState() {
    let client = ClientStub(readResult: .success(true))
    let model = SleepSettingsModel(client: client)

    model.refresh()

    expect(model.isSleepDisabled == true)
    expect(model.errorMessage == nil)
    expect(!model.isBusy)
  }

  internal static func testRefreshExposesReadError() {
    let client = ClientStub(readResult: .failure(TestError.readFailed))
    let model = SleepSettingsModel(client: client)

    model.refresh()

    expect(model.isSleepDisabled == nil)
    expect(model.errorMessage == "read failed")
    expect(!model.isBusy)
  }

  internal static func testUpdateStoresAuthenticatedSetting() {
    let client = ClientStub(readResult: .success(false))
    let model = SleepSettingsModel(client: client)
    model.refresh()

    model.updateSleepDisabled(true)

    expect(client.requestedValues == [true])
    expect(model.isSleepDisabled == true)
    expect(model.errorMessage == nil)
  }

  internal static func testUpdatePreservesStateAfterFailure() {
    let client = ClientStub(
      readResult: .success(false),
      updateError: TestError.writeFailed
    )
    let model = SleepSettingsModel(client: client)
    model.refresh()

    model.updateSleepDisabled(true)

    expect(client.requestedValues == [true])
    expect(model.isSleepDisabled == false)
    expect(model.errorMessage == "write failed")
  }

  internal static func testUpdateIgnoresUnchangedValue() {
    let client = ClientStub(readResult: .success(true))
    let model = SleepSettingsModel(client: client)
    model.refresh()

    model.updateSleepDisabled(true)

    expect(client.requestedValues.isEmpty)
  }

  internal static func testToggleReversesLoadedState() {
    let client = ClientStub(readResult: .success(false))
    let model = SleepSettingsModel(client: client)
    model.refresh()

    model.toggleSleep()

    expect(client.requestedValues == [true])
    expect(model.isSleepDisabled == true)
  }

  internal static func testToggleLoadsMissingState() {
    let client = ClientStub(readResult: .success(true))
    let model = SleepSettingsModel(client: client)

    model.toggleSleep()

    expect(client.requestedValues == [false])
    expect(model.isSleepDisabled == false)
  }

  internal static func testToggleStopsAfterReadFailure() {
    let client = ClientStub(readResult: .failure(TestError.readFailed))
    let model = SleepSettingsModel(client: client)

    model.toggleSleep()

    expect(client.requestedValues.isEmpty)
    expect(model.errorMessage == "read failed")
  }
}
