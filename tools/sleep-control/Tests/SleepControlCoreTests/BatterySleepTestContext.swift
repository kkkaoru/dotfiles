#if canImport(SleepControlCore)
  import SleepControlCore
#endif

import Foundation

@MainActor
internal struct BatterySleepTestContext {
  internal let defaults: UserDefaults
  internal let settings: ShortcutSettingsStore
  internal let client: BatterySleepClientStub
  internal let controller: BatterySleepController
  private let suiteName: String

  internal init() throws {
    let name = "SleepControlBatteryTests.\(UUID().uuidString)"
    guard let testDefaults = UserDefaults(suiteName: name) else { throw TestError.writeFailed }
    suiteName = name
    defaults = testDefaults
    settings = ShortcutSettingsStore(defaults: testDefaults)
    let stub = BatterySleepClientStub()
    client = stub
    controller = BatterySleepController(settings: settings, client: stub) {
      stub.events.append("refresh")
    }
  }

  internal func cleanUp() {
    defaults.removePersistentDomain(forName: suiteName)
  }
}
