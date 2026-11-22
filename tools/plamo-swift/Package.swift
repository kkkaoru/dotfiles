// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "plamo-swift",
    platforms: [.macOS(.v15)],
    dependencies: [
        .package(url: "https://github.com/ml-explore/mlx-swift.git", from: "0.33.0")
    ],
    targets: [
        .executableTarget(
            name: "plamo-swift",
            dependencies: [
                .product(name: "MLX", package: "mlx-swift"),
                .product(name: "MLXNN", package: "mlx-swift"),
                .product(name: "MLXRandom", package: "mlx-swift"),
            ],
            path: "Sources/plamo-swift"
        )
    ]
)
