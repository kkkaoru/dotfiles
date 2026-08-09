import Foundation

internal struct Configuration: Sendable {
  internal let home: URL
  internal let wifiDevice: String
  internal let targetSSID: String
  internal let fallbackSSID: String
  internal let targetRouter: String
  internal let fallbackRouter: String

  internal init() throws {
    let environment = ProcessInfo.processInfo.environment
    guard let homePath = environment["HOME"], !homePath.isEmpty else {
      throw ApplicationError.homeUnavailable
    }
    home = URL(filePath: homePath, directoryHint: .isDirectory)
    wifiDevice = environment["MAC_WIFI_STABILITY_DEVICE"] ?? "en0"
    targetSSID = environment["MAC_WIFI_STABILITY_TARGET_SSID"] ?? "ohomemesh"
    fallbackSSID = environment["MAC_WIFI_STABILITY_FALLBACK_SSID"] ?? "kkk4oru-wifi"
    targetRouter = environment["MAC_WIFI_STABILITY_TARGET_ROUTER"] ?? "192.168.1.1"
    fallbackRouter = environment["MAC_WIFI_STABILITY_FALLBACK_ROUTER"] ?? "172.20.10.1"
  }
}
