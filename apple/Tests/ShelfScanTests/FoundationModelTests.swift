import CoreGraphics
import CoreML
import Foundation
import ShelfScan
@testable import ShelfScanVision
import XCTest

/// models/ (scripts/fetch_models.sh). Tests skip when a model is missing.
enum Models {
    static let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../models").standardized

    static func require(_ name: String) throws {
        guard FileManager.default.fileExists(atPath: dir.appendingPathComponent(name).path) else { throw XCTSkip("run scripts/fetch_models.sh (\(name))") }
    }

    static var clip: MobileCLIP?
    static func mobileCLIP() throws -> MobileCLIP {
        if let c = clip { return c }
        try require("mobileclip_s2_image.mlpackage")
        try require("clip-tokenizer")
        let c = try MobileCLIP(modelsDir: dir, variant: .s2)
        clip = c
        return c
    }
}

final class CLIPTokenizerTests: XCTestCase {
    /// Reference ids from Hugging Face `tokenizers` 0.23.3 configured like OpenAI CLIP (generated once, committed).
    func testMatchesReferenceTokenizer() throws {
        try Models.require("clip-tokenizer")
        let tok = try CLIPTokenizer(directory: Models.dir.appendingPathComponent("clip-tokenizer"))
        let refURL = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("clip_tokenizer_reference.json")
        let ref = try JSONDecoder().decode([String: [Int32]].self, from: Data(contentsOf: refURL))
        XCTAssertGreaterThan(ref.count, 150)
        var bad: [String] = []
        for (text, ids) in ref where tok.encode(text) != ids { bad.append("\(text): \(tok.encode(text)) vs \(ids)") }
        XCTAssertTrue(bad.isEmpty, bad.prefix(5).joined(separator: "\n"))
        let full = tok.tokenize("a photo of a cat")
        XCTAssertEqual(full.count, 77)
        XCTAssertEqual(Array(full.prefix(7)), [49406, 320, 1125, 539, 320, 2368, 49407])
        XCTAssertTrue(full.dropFirst(7).allSatisfy { $0 == 0 })
    }
}

/// Simple drawings with an unambiguous answer.
func drawing(_ draw: (CGContext) -> Void, size: Int = 256) -> CGImage {
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: size, height: size))
    draw(ctx)
    return ctx.makeImage()!
}

final class MobileCLIPTests: XCTestCase {
    func testVectorsAreUnitLengthAnd512() throws {
        let clip = try Models.mobileCLIP()
        let img = try Fixtures.cgImage("ean13.png")
        let a = try clip.embed(image: img), b = try clip.embed(image: img)
        XCTAssertEqual(a.count, 512)
        XCTAssertEqual(a.reduce(0) { $0 + $1 * $1 }, 1, accuracy: 1e-3)
        XCTAssertEqual(zip(a, b).reduce(0) { $0 + $1.0 * $1.1 }, 1, accuracy: 1e-4)
        XCTAssertEqual(try clip.embed(text: "a photo of a mug").count, 512)
    }

    /// Zero-shot sanity on images with a known answer. Printed probabilities go into the results note.
    func testZeroShotKnownAnswers() throws {
        let clip = try Models.mobileCLIP()
        func best(_ img: CGImage, _ words: [String]) throws -> (String, Float) {
            let g = try CLIPZeroShotGuesser(clip: clip, vocabulary: words)
            let r = g.rank(try clip.embed(image: img))
            print("CLIP \(words) -> \(r.map { "\($0.0) \(String(format: "%.2f", $0.1))" })")
            return r[0]
        }
        let barcode = try Fixtures.cgImage("ean13.png")
        XCTAssertEqual(try best(barcode, ["barcode", "cat", "tree", "car"]).0, "barcode")
        let redDisc = drawing { $0.setFillColor(CGColor(red: 0.9, green: 0.1, blue: 0.1, alpha: 1)); $0.fillEllipse(in: CGRect(x: 48, y: 48, width: 160, height: 160)) }
        XCTAssertEqual(try best(redDisc, ["red circle", "blue circle", "green square", "yellow triangle"]).0, "red circle")
        let blueSquare = drawing { $0.setFillColor(CGColor(red: 0.1, green: 0.2, blue: 0.9, alpha: 1)); $0.fill(CGRect(x: 48, y: 48, width: 160, height: 160)) }
        XCTAssertEqual(try best(blueSquare, ["red circle", "blue square", "green square", "yellow triangle"]).0, "blue square")
    }

    func testGuesserSkipsRejectedTitlesAndReportsAlternatives() throws {
        let clip = try Models.mobileCLIP()
        let g = try CLIPZeroShotGuesser(clip: clip, vocabulary: ["barcode", "cat", "tree"])
        let frame = try Fixtures.frame("ean13.png")
        func req(_ excluded: [String]) -> GuessRequest {
            GuessRequest(itemId: "item-001", image: frame, extraViews: [], detectorLabel: "product", dominantColors: [], excludedTitles: excluded, mode: .describeFirst)
        }
        let first = try g.guess(request: req([]))
        XCTAssertEqual(first.title, "Barcode")
        XCTAssertGreaterThan(first.confidence, 0)
        XCTAssertLessThanOrEqual(first.confidence, 1)
        XCTAssertTrue(first.attributes.contains { $0.name == "Other guesses" })
        let second = try g.guess(request: req(["Barcode"]))
        XCTAssertNotEqual(second.title, "Barcode")
    }
}

/// Stands in for AnthropicGuesser: records what it was sent (tests never touch the network).
final class FakeCloud: ProductGuesser, @unchecked Sendable {
    var requests: [GuessRequest] = []
    var fail = false
    func guess(request: GuessRequest) throws -> ProductGuess {
        requests.append(request)
        if fail { throw ShelfError.Backend(backend: "anthropic", msg: "HTTP 529: overloaded") }
        return ProductGuess(title: "Acme Barcode Card", brand: "Acme", category: nil, attributes: [], confidence: 0.8)
    }
}

final class HybridGuesserTests: XCTestCase {
    func testGateKeepsNonProductsLocal() throws {
        let clip = try Models.mobileCLIP()
        let h = HybridGuesser(local: try CLIPZeroShotGuesser(clip: clip, vocabulary: ["person", "mug"]), cloud: FakeCloud())
        XCTAssertEqual(h.keepLocal([("person", 0.9), ("mug", 0.1)]), "person")
        XCTAssertNil(h.keepLocal([("person", 0.3), ("mug", 0.25)]), "below the gate: send")
        XCTAssertNil(h.keepLocal([("mug", 0.9), ("person", 0.1)]))
    }

    func testProductGoesToCloudWithHintAndFallsBackVisibly() throws {
        let clip = try Models.mobileCLIP()
        let cloud = FakeCloud()
        let h = HybridGuesser(local: try CLIPZeroShotGuesser(clip: clip, vocabulary: ["barcode", "person", "tree"]), cloud: cloud)
        let req = GuessRequest(itemId: "item-001", image: try Fixtures.frame("ean13.png"), extraViews: [], detectorLabel: "product",
                               dominantColors: [], excludedTitles: [], mode: .barcodeFirst)
        let g = try h.guess(request: req)
        XCTAssertEqual(g.title, "Acme Barcode Card")
        XCTAssertEqual(cloud.requests.count, 1)
        XCTAssertTrue(cloud.requests[0].detectorLabel.contains("barcode"), cloud.requests[0].detectorLabel)
        XCTAssertTrue(cloud.requests[0].detectorLabel.hasPrefix("product; an on-device image model suggests: barcode"), "confident hint passed")
        XCTAssertTrue(g.attributes.contains { $0.name == "On-device guess" })
        // a crop too small to read stays local
        let tiny = GuessRequest(itemId: "item-002", image: try Fixtures.frame(XCTUnwrap(Fixtures.cgImage("ean13.png").cropping(to: CGRect(x: 0, y: 0, width: 200, height: 40)))),
                                extraViews: [], detectorLabel: "toothbrush", dominantColors: [], excludedTitles: [], mode: .barcodeFirst)
        let small = try h.guess(request: tiny)
        XCTAssertEqual(cloud.requests.count, 1, "tiny crop must not be sent")
        XCTAssertTrue(small.attributes.contains { $0.value.contains("too small") })
        // a weak on-device guess is not passed as a hint, and the detector class never is
        let unsure = HybridGuesser(local: try CLIPZeroShotGuesser(clip: clip, vocabulary: ["barcode", "barcode label", "price tag", "receipt"]), cloud: cloud, hintMin: 0.99)
        _ = try unsure.guess(request: GuessRequest(itemId: "item-003", image: try Fixtures.frame("ean13.png"), extraViews: [], detectorLabel: "toothbrush",
                                                   dominantColors: [], excludedTitles: [], mode: .barcodeFirst))
        XCTAssertEqual(cloud.requests.last?.detectorLabel, "product")
        cloud.fail = true
        let f = try h.guess(request: req)
        XCTAssertEqual(f.title, "Barcode")
        XCTAssertTrue(f.attributes.contains { $0.name == "Cloud" && $0.value.contains("failed") })
    }
}

final class SAM2Tests: XCTestCase {
    static var sam: SAM2Tiny?
    func sam() throws -> SAM2Tiny {
        if let s = Self.sam { return s }
        try Models.require("SAM2_1TinyImageEncoderFLOAT16.mlpackage")
        let s = try SAM2Tiny(modelsDir: Models.dir)
        Self.sam = s
        return s
    }

    /// Box prompts slightly larger than each synthetic product: SAM's mask must hug the product (box IoU with truth),
    /// which also confirms the prompt coordinate convention (input pixels of the 1024 square).
    func testBoxPromptsGiveTightMasksOnSyntheticScene() throws {
        let m = try Fixtures.manifest()
        let sam = try sam()
        let img = try Fixtures.cgImage(m.scene.frames[0].file)
        let t0 = Date()
        let enc = try sam.encode(img)
        let encMs = Date().timeIntervalSince(t0) * 1000
        for t in sceneTruth {
            let loose = BBox(x: t.box.x - 0.03, y: t.box.y - 0.03, w: t.box.w + 0.06, h: t.box.h + 0.06)
            let t1 = Date()
            let r = try sam.mask(enc, box: loose)
            let decMs = Date().timeIntervalSince(t1) * 1000
            let d = try XCTUnwrap(SAMRefinedDetector.detection(from: r.logits, side: r.side, within: loose,
                                                               like: Detection(bbox: loose, score: 0.9, label: t.name, mask: nil),
                                                               frameWidth: img.width, frameHeight: img.height))
            let fill = Float(d.mask!.data.filter { $0 == 255 }.count) / Float(d.mask!.data.count)
            print(String(format: "SAM %@ iou=%.3f (loose box %.3f) score=%.2f fill=%.2f encode_ms=%.0f decode_ms=%.1f",
                         t.name, iou(d.bbox, t.box), iou(loose, t.box), r.score, fill, encMs, decMs))
            XCTAssertGreaterThan(iou(d.bbox, t.box), 0.85, t.name)
            XCTAssertGreaterThan(fill, 0.85, "\(t.name): a solid rectangle should fill its mask")
            // the mask is at frame resolution now, not SAM's 256 grid
            XCTAssertEqual(Float(d.mask!.width), d.bbox.w * Float(img.width), accuracy: 2, t.name)
        }
    }

    func testRefinedDetectorKeepsLabelsAndCount() throws {
        let m = try Fixtures.manifest()
        let det = SAMRefinedDetector(base: BlobDetector(), sam: try sam())
        let frame = try Fixtures.frame(m.scene.frames[0].file)
        _ = try det.detect(frame: frame)   // warm-up
        let t0 = Date()
        let dets = try det.detect(frame: frame)
        print(String(format: "SAM refined detector ms_per_frame=%.0f detections=%d", Date().timeIntervalSince(t0) * 1000, dets.count))
        XCTAssertEqual(dets.count, 3)
        for t in sceneTruth { XCTAssertGreaterThan(dets.map { iou($0.bbox, t.box) }.max() ?? 0, 0.85, t.name) }
    }

    /// Warm timings per compute unit (printed for docs/results; no assertion).
    func testTimingByComputeUnits() throws {
        try Models.require("SAM2_1TinyImageEncoderFLOAT16.mlpackage")
        let m = try Fixtures.manifest()
        let img = try Fixtures.cgImage(m.scene.frames[0].file)
        for (name, units) in [("cpu+ane", MLComputeUnits.cpuAndNeuralEngine), ("cpu+gpu", .cpuAndGPU), ("all", .all)] {
            let sam = try SAM2Tiny(modelsDir: Models.dir, encoderUnits: units, decoderUnits: units)
            var enc = try sam.encode(img)
            _ = try sam.mask(enc, box: sceneTruth[0].box)
            let n = 5
            var t = Date()
            for _ in 0..<n { enc = try sam.encode(img) }
            let e = Date().timeIntervalSince(t) * 1000 / Double(n)
            t = Date()
            for _ in 0..<n { _ = try sam.mask(enc, box: sceneTruth[0].box) }
            let d = Date().timeIntervalSince(t) * 1000 / Double(n)
            print(String(format: "SAMTIME units=%@ encode_ms=%.1f decode_ms=%.1f", name, e, d))
        }
        let split = try SAM2Tiny(modelsDir: Models.dir)
        let det = SAMRefinedDetector(base: BlobDetector(), sam: split)
        let frame = try Fixtures.frame(m.scene.frames[0].file)
        _ = try det.detect(frame: frame)
        let t = Date()
        for _ in 0..<5 { _ = try det.detect(frame: frame) }
        print(String(format: "SAMTIME default split (encoder ANE, decoder GPU): refined detector, 3 boxes, ms_per_frame=%.1f", Date().timeIntervalSince(t) * 200))
        if let clipDir = Optional(Models.dir), FileManager.default.fileExists(atPath: clipDir.appendingPathComponent("mobileclip_s0_image.mlpackage").path) {
            for v in MobileCLIP.Variant.allCases {
                for (name, units) in [("cpu+ane", MLComputeUnits.cpuAndNeuralEngine), ("cpu+gpu", .cpuAndGPU)] {
                    let clip = try MobileCLIP(modelsDir: clipDir, variant: v, withText: false, computeUnits: units)
                    _ = try clip.embed(image: img)
                    let t = Date()
                    for _ in 0..<10 { _ = try clip.embed(image: img) }
                    print(String(format: "CLIPTIME variant=%@ units=%@ image_ms=%.1f", v.rawValue, name, Date().timeIntervalSince(t) * 100))
                }
            }
        }
    }
}
