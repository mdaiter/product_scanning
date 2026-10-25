// swift-tools-version:5.9
// Live Mac demo (Phase 1.4): webcam -> Engine, preview with boxes, item list, review actions, stats, CSV export.
//   bash scripts/build_xcframework.sh && cd apple/Examples/MacDemo && swift run MacDemo
//   swift run MacDemo --measure 60 --out latency.json [--detector vision] [--embedder featureprint] [--scanner vision]
import Foundation
import PackageDescription

let infoPlist = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Sources/MacDemo/Info.plist").path

let package = Package(
    name: "MacDemo",
    platforms: [.macOS(.v14)],
    dependencies: [.package(path: "../..")],
    targets: [
        .executableTarget(
            name: "MacDemo",
            dependencies: [
                .product(name: "ShelfScan", package: "apple"),
                .product(name: "ShelfScanVision", package: "apple"),
                .product(name: "ShelfScanCapture", package: "apple"),
            ],
            path: "Sources/MacDemo",
            exclude: ["Info.plist"],
            // An unbundled executable needs NSCameraUsageDescription embedded for the camera permission prompt.
            linkerSettings: [.unsafeFlags(["-Xlinker", "-sectcreate", "-Xlinker", "__TEXT", "-Xlinker", "__info_plist",
                                           "-Xlinker", infoPlist])]
        ),
    ]
)
