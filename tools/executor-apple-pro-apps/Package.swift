// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "AppleProApps",
  platforms: [.macOS(.v15)],
  products: [.executable(name: "apple-pro-apps", targets: ["AppleProApps"])],
  dependencies: [
    .package(url: "https://github.com/modelcontextprotocol/swift-sdk.git", exact: "0.12.1")
  ],
  targets: [
    .target(name: "ProAppsCore"),
    .executableTarget(
      name: "AppleProApps",
      dependencies: ["ProAppsCore", .product(name: "MCP", package: "swift-sdk")]
    ),
    .testTarget(name: "ProAppsCoreTests", dependencies: ["ProAppsCore"]),
    .testTarget(
      name: "ProAppsNativeTests", dependencies: ["ProAppsCore"], resources: [.copy("Fixtures")]),
    .testTarget(
      name: "AppleProAppsTests",
      dependencies: ["AppleProApps", "ProAppsCore", .product(name: "MCP", package: "swift-sdk")]
    ),
  ],
  swiftLanguageModes: [.v6]
)
