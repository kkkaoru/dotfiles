// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "plamo-swift",
    platforms: [.macOS(.v15)],
    dependencies: [
        .package(url: "https://github.com/ml-explore/mlx-swift.git", from: "0.33.0")
    ],
    targets: [
        .target(
            name: "PlamoCore",
            dependencies: [.product(name: "MLX", package: "mlx-swift")],
            path: "Sources/PlamoCore"
        ),
        .executableTarget(
            name: "plamo-swift",
            dependencies: ["PlamoCore"],
            path: "Sources/plamo-swift"
        ),
        .testTarget(
            name: "PlamoCoreTests",
            dependencies: ["PlamoCore"],
            path: "Tests/PlamoCoreTests"
        ),
    ]
)
