import CoreGraphics
import Foundation
import ImageIO
import ShelfScan
import XCTest

/// Fixtures written by `shelfscan fixtures --out apple/Tests/ShelfScanTests/Fixtures` (scripts/swift_test.sh).
enum Fixtures {
    static let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Fixtures")

    struct Manifest: Decodable {
        struct Scene: Decodable {
            struct FrameRef: Decodable { let file: String; let timestamp_ms: UInt64 }
            let frames: [FrameRef]
            let products: Int
        }
        struct Ean13: Decodable { let file: String; let gtin: String }
        struct Barcodes: Decodable {
            struct Ref: Decodable { let file: String; let module_px: Int; let blur_sigma: Float }
            let gtin: String
            let frames: [Ref]
        }
        let scene: Scene
        let ean13: Ean13
        let barcodes: Barcodes
    }

    static func manifest() throws -> Manifest {
        let url = dir.appendingPathComponent("fixtures.json")
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw XCTSkip("no fixtures: run `cargo run -p shelfscan-cli -- fixtures --out apple/Tests/ShelfScanTests/Fixtures`")
        }
        return try JSONDecoder().decode(Manifest.self, from: Data(contentsOf: url))
    }

    static func cgImage(_ file: String) throws -> CGImage {
        let url = dir.appendingPathComponent(file)
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil), let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else {
            throw NSError(domain: "Fixtures", code: 1, userInfo: [NSLocalizedDescriptionKey: "cannot decode \(file)"])
        }
        return img
    }

    /// A PNG as a tightly packed RGBA frame, the way an app would hand a decoded image to the engine.
    static func frame(_ file: String, timestampMs: UInt64 = 0) throws -> Frame {
        try frame(cgImage(file), timestampMs: timestampMs)
    }

    static func frame(_ img: CGImage, timestampMs: UInt64 = 0) throws -> Frame {
        let w = img.width, h = img.height
        var data = Data(count: w * h * 4)
        let ok = data.withUnsafeMutableBytes { (buf: UnsafeMutableRawBufferPointer) -> Bool in
            guard let ctx = CGContext(data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return false }
            ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        guard ok else { throw NSError(domain: "Fixtures", code: 2, userInfo: [NSLocalizedDescriptionKey: "cannot draw image"]) }
        return Frame(width: UInt32(w), height: UInt32(h), format: .rgba8, data: data, stride: 0, timestampMs: timestampMs)
    }
}
