// swift-tools-version: 6.2

import PackageDescription

internal let package = Package(
  name: "LidDisplayWatcher",
  platforms: [.macOS(.v11)],
  products: [
    .executable(name: "lid-display-watcher", targets: ["LidDisplayWatcher"])
  ],
  targets: [
    .target(name: "LidDisplayWatcherCore"),
    .executableTarget(
      name: "LidDisplayWatcher",
      dependencies: ["LidDisplayWatcherCore"],
      linkerSettings: [
        .linkedFramework("CoreFoundation"),
        .linkedFramework("IOKit"),
        .unsafeFlags([
          "-Xlinker", "-dead_strip",
          "-Xlinker", "-no_data_const",
          "-Xlinker", "-no_function_starts",
          "-Xlinker", "-exported_symbol",
          "-Xlinker", "_main",
        ]),
      ]
    ),
    .executableTarget(
      name: "LidDisplayWatcherCoreTests",
      dependencies: ["LidDisplayWatcherCore"],
      path: "Tests/LidDisplayWatcherCoreTests"
    ),
  ]
)
