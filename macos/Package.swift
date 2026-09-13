// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MacOSPlayground",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "MacOSPlayground",
            path: "Sources/MacOSPlayground"
        ),
        .testTarget(
            name: "MacOSPlaygroundTests",
            dependencies: ["MacOSPlayground"],
            path: "Tests/MacOSPlaygroundTests"
        ),
    ]
)
