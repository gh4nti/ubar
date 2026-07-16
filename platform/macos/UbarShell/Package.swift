// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "UbarShell",
    platforms: [.macOS(.v13)],
    products: [.executable(name: "UbarShell", targets: ["UbarShell"])],
    targets: [
        .target(
            name: "CEngine",
            path: "Sources/CEngine",
            publicHeadersPath: "include"
        ),
        .executableTarget(name: "UbarShell", dependencies: ["CEngine"]),
    ]
)
