// swift-tools-version:5.9
// UNVERIFIED: not built yet. Run scripts/build_xcframework.sh first (it creates build/ and Sources/ShelfScan/shelfscan.swift).
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
    ]
)
