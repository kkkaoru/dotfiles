#if canImport(SleepControlCore)
  import SleepControlCore
#endif

@MainActor
internal final class ClientStub: SleepSettingsClient {
  private let readResult: Result<Bool, Error>
  private let updateError: Error?
  internal private(set) var requestedValues: [Bool] = []

  internal init(readResult: Result<Bool, Error>) {
    self.readResult = readResult
    updateError = nil
  }

  internal init(readResult: Result<Bool, Error>, updateError: Error) {
    self.readResult = readResult
    self.updateError = updateError
  }

  internal func readSleepDisabled() throws -> Bool {
    try readResult.get()
  }

  internal func setSleepDisabled(_ disabled: Bool) throws {
    requestedValues.append(disabled)
    if let updateError {
      throw updateError
    }
  }
}
