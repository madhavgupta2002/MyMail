// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "MyMail",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "MyMail",
            path: "Sources/MyMail"
            // tools-version 5.10 builds in Swift 5 language mode.
        )
    ]
)
