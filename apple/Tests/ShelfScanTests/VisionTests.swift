import CoreGraphics
import Foundation
import ShelfScan
import ShelfScanVision
import XCTest

/// Truth boxes of `Scene::three_products()` (crates/shelfscan/src/synthetic.rs), top-left origin, normalised.
let sceneTruth: [(name: String, box: BBox)] = [
    ("red", BBox(x: 0.08, y: 0.20, w: 0.20, h: 0.35)),
    ("blue", BBox(x: 0.40, y: 0.25, w: 0.18, h: 0.40)),
    ("green", BBox(x: 0.70, y: 0.18, w: 0.20, h: 0.38)),
]

func iou(_ a: BBox, _ b: BBox) -> Float {
    let x0 = max(a.x, b.x), y0 = max(a.y, b.y), x1 = min(a.x + a.w, b.x + b.w), y1 = min(a.y + a.h, b.y + b.h)
    let inter = max(0, x1 - x0) * max(0, y1 - y0)
    return inter / (a.w * a.h + b.w * b.h - inter)
}

func cosine(_ a: [Float], _ b: [Float]) -> Float {
    var dot: Float = 0, na: Float = 0, nb: Float = 0
    for i in 0..<min(a.count, b.count) { dot += a[i] * b[i]; na += a[i] * a[i]; nb += b[i] * b[i] }
    return dot / (na.squareRoot() * nb.squareRoot())
}

func crop(_ img: CGImage, _ b: BBox) -> CGImage? {
    img.cropping(to: CGRect(x: CGFloat(b.x) * CGFloat(img.width), y: CGFloat(b.y) * CGFloat(img.height),
                            width: CGFloat(b.w) * CGFloat(img.width), height: CGFloat(b.h) * CGFloat(img.height)))
}

final class VisionBarcodeTests: XCTestCase {
    func testDecodesRenderedEan13Card() throws {
        let m = try Fixtures.manifest()
        let reads = try VisionBarcodeScanner().scan(frame: try Fixtures.frame(m.ean13.file))
        XCTAssertTrue(reads.contains { $0.text == m.ean13.gtin }, "reads: \(reads)")
    }

    /// Decode table, Vision vs rxing, over module size ("distance") and blur. Printed for docs/results; asserts only
    /// the easy cases so the table itself is the measurement.
    func testDecodeRateByModuleAndBlur() throws {
        let m = try Fixtures.manifest()
        let scanners: [(String, BarcodeScanner)] = [("vision", VisionBarcodeScanner()), ("rxing", RxingScanner())]
        var lines = ["scanner | module_px | blur | decoded | ms"]
        var ok: [String: Bool] = [:]
        for ref in m.barcodes.frames {
            let frame = try Fixtures.frame(ref.file)
            for (name, s) in scanners {
                let t0 = Date()
                let reads = (try? s.scan(frame: frame)) ?? []
                let ms = Date().timeIntervalSince(t0) * 1000
                let hit = reads.contains { $0.text.hasSuffix(String(m.barcodes.gtin.dropFirst())) }
                ok["\(name)-\(ref.module_px)-\(Int(ref.blur_sigma))"] = hit
                lines.append("\(name) | \(ref.module_px) | \(ref.blur_sigma) | \(hit ? "yes" : "no") | \(String(format: "%.1f", ms))")
            }
        }
        print("BARCODE_TABLE\n" + lines.joined(separator: "\n"))
        XCTAssertEqual(ok["vision-3-0"], true)
        XCTAssertEqual(ok["rxing-3-0"], true)
    }
}

final class VisionEmbedderTests: XCTestCase {
    func testVectorShapeNormAndSelfSimilarity() throws {
        let m = try Fixtures.manifest()
        let scene = try Fixtures.cgImage(m.scene.frames[0].file)
        let e = VisionFeaturePrintEmbedder()
        let red = try Fixtures.frame(XCTUnwrap(crop(scene, sceneTruth[0].box)))
        let a = try e.embed(image: red), b = try e.embed(image: red)
        let norm = a.reduce(0) { $0 + $1 * $1 }.squareRoot()
        print("FEATUREPRINT length=\(a.count) norm=\(norm) self_cos=\(cosine(a, b))")
        XCTAssertGreaterThan(a.count, 0)
        XCTAssertEqual(norm, 1, accuracy: 1e-3)
        XCTAssertEqual(cosine(a, b), 1, accuracy: 1e-4)
    }

    /// Same product in two frames (shake + noise) vs different products: only printed here; the real decision is the 1.8 bake-off.
    func testSameVsDifferentProductCosine() throws {
        let m = try Fixtures.manifest()
        let f0 = try Fixtures.cgImage(m.scene.frames[0].file), f5 = try Fixtures.cgImage(m.scene.frames[5].file)
        let e = VisionFeaturePrintEmbedder()
        func vec(_ img: CGImage, _ i: Int) throws -> [Float] { try e.embed(image: Fixtures.frame(XCTUnwrap(crop(img, sceneTruth[i].box)))) }
        let same = cosine(try vec(f0, 0), try vec(f5, 0))
        let diff = cosine(try vec(f0, 0), try vec(f0, 1))
        print("FEATUREPRINT same_product_cos=\(same) different_product_cos=\(diff)")
        XCTAssertGreaterThan(same, diff)
    }
}

final class VisionDetectorTests: XCTestCase {
    /// Boxes must land on the truth boxes: a flipped (bottom-left origin) mask buffer would miss them, since the
    /// products sit at different heights.
    func testSceneBoxesMatchTruthAndMasksAlign() throws {
        let m = try Fixtures.manifest()
        let dets = try VisionForegroundDetector().detect(frame: try Fixtures.frame(m.scene.frames[0].file))
        print("FOREGROUND detections=\(dets.count) " + dets.map { "[\($0.bbox.x),\($0.bbox.y),\($0.bbox.w),\($0.bbox.h)]" }.joined(separator: " "))
        for t in sceneTruth {
            let best = dets.max { iou($0.bbox, t.box) < iou($1.bbox, t.box) }
            let score = best.map { iou($0.bbox, t.box) } ?? 0
            print("FOREGROUND \(t.name) best_iou=\(score)")
            XCTAssertGreaterThan(score, 0.7, "\(t.name)")
            if let mask = best?.mask {
                // the centre of a solid rectangle must be inside its mask
                let cx = Int(mask.width) / 2, cy = Int(mask.height) / 2
                XCTAssertEqual(mask.data[cy * Int(mask.width) + cx], 255, "\(t.name) mask centre")
                XCTAssertEqual(mask.data.count, Int(mask.width * mask.height))
            } else {
                XCTFail("\(t.name): no mask")
            }
        }
    }

    /// The camera delivers `.bgra8` with padded rows (CameraSource passes `bytesPerRow` as stride). Same boxes expected.
    func testBgraWithPaddedStrideGivesSameBoxes() throws {
        let m = try Fixtures.manifest()
        let rgba = try Fixtures.frame(m.scene.frames[0].file)
        let w = Int(rgba.width), h = Int(rgba.height), stride = w * 4 + 64
        var bgra = Data(count: stride * h)
        rgba.data.withUnsafeBytes { (s: UnsafeRawBufferPointer) in
            bgra.withUnsafeMutableBytes { (d: UnsafeMutableRawBufferPointer) in
                for y in 0..<h {
                    for x in 0..<w {
                        let si = (y * w + x) * 4, di = y * stride + x * 4
                        d[di] = s[si + 2]; d[di + 1] = s[si + 1]; d[di + 2] = s[si]; d[di + 3] = 255
                    }
                }
            }
        }
        let frame = Frame(width: rgba.width, height: rgba.height, format: .bgra8, data: bgra, stride: UInt32(stride), timestampMs: 0)
        let dets = try VisionForegroundDetector().detect(frame: frame)
        for t in sceneTruth {
            XCTAssertGreaterThan(dets.map { iou($0.bbox, t.box) }.max() ?? 0, 0.7, "\(t.name)")
        }
        // and the Rust side reads the same padded BGRA frame
        let blob = try BlobDetector().detect(frame: frame)
        for t in sceneTruth {
            XCTAssertGreaterThan(blob.map { iou($0.bbox, t.box) }.max() ?? 0, 0.7, "blob \(t.name)")
        }
    }

    func testBlobDetectorBaselineOnSameFrame() throws {
        let m = try Fixtures.manifest()
        let dets = try BlobDetector().detect(frame: try Fixtures.frame(m.scene.frames[0].file))
        for t in sceneTruth {
            let score = dets.map { iou($0.bbox, t.box) }.max() ?? 0
            print("BLOB \(t.name) best_iou=\(score)")
            XCTAssertGreaterThan(score, 0.7, "\(t.name)")
        }
    }
}
