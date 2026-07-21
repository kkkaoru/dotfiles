// swift-tools-version: 6.2

import PackageDescription

internal let package = Package(
  name: "SleepControl",
  platforms: [.macOS(.v13)],
  products: [
    .executable(name: "sleep-control", targets: ["SleepControl"])
  ],
  targets: [
    .target(name: "SleepControlCore"),
    .target(
      name: "SleepControlUI",
      dependencies: ["SleepControlCore"]
    ),
    .executableTarget(
      name: "SleepControl",
      dependencies: ["SleepControlCore", "SleepControlUI"]
    ),
    .executableTarget(
      name: "SleepControlCoreTests",
      dependencies: ["SleepControlCore"],
      path: "Tests/SleepControlCoreTests"
    ),
    .executableTarget(
      name: "SleepControlSnapshots",
      dependencies: ["SleepControlCore", "SleepControlUI"],
      path: "Tests/SleepControlSnapshots"
    ),
  ]
)
