// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "DeadreckonKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "DeadreckonKit", targets: ["DeadreckonKit"])
    ],
    targets: [
        .target(name: "DeadreckonKit"),
        .testTarget(name: "DeadreckonKitTests", dependencies: ["DeadreckonKit"]),
    ]
)
