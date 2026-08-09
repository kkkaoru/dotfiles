// swift-tools-version: 6.2

import PackageDescription

internal let package = Package(
  name: "MacWiFiStability",
  platforms: [.macOS(.v13)],
  products: [
    .executable(name: "mac-wifi-stability", targets: ["MacWiFiStability"])
  ],
  targets: [
    .target(name: "MacWiFiStabilityCore"),
    .executableTarget(
      name: "MacWiFiStability",
      dependencies: ["MacWiFiStabilityCore"]
    ),
    .executableTarget(
      name: "MacWiFiStabilityCoreTests",
      dependencies: ["MacWiFiStabilityCore"],
      path: "Tests/MacWiFiStabilityCoreTests"
    ),
  ]
)
