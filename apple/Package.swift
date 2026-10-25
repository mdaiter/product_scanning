// swift-tools-version:5.9
// Built on macOS 26 / Xcode 26 (2026-10-25). Run scripts/build_xcframework.sh first (it creates build/ and Sources/ShelfScan/shelfscan.swift),
// and `shelfscan fixtures --out apple/Tests/ShelfScanTests/Fixtures` before `swift test` (scripts/swift_test.sh does both).
import PackageDescription

let package = Package(
    name: "ShelfScan",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "ShelfScan", targets: ["ShelfScan"]),
        .library(name: "ShelfScanVision", targets: ["ShelfScanVision"]),
        .library(name: "ShelfScanCapture", targets: ["ShelfScanCapture"]),
    ],
    targets: [
        // Rust static libraries + C header + modulemap (module name: shelfscanFFI).
        .binaryTarget(name: "shelfscanFFI", path: "build/shelfscanFFI.xcframework"),
        // Generated UniFFI Swift (shelfscan.swift). Do not edit by hand.
        .target(name: "ShelfScan", dependencies: ["shelfscanFFI"], path: "Sources/ShelfScan"),
        // Apple Vision plugins: BarcodeScanner, Embedder, Detector. UNCOMPILED SKETCHES.
        .target(name: "ShelfScanVision", dependencies: ["ShelfScan"], path: "Sources/ShelfScanVision"),
        // AVFoundation camera source + SwiftUI overlay model. UNCOMPILED SKETCHES.
        .target(name: "ShelfScanCapture", dependencies: ["ShelfScan"], path: "Sources/ShelfScanCapture"),
        // Fixtures are generated (git-ignored) and read from disk next to the test sources, not bundled.
        .testTarget(name: "ShelfScanTests", dependencies: ["ShelfScan", "ShelfScanVision"], path: "Tests/ShelfScanTests",
                    exclude: ["Fixtures"]),
    ]
)
