import CoreGraphics
import CoreML
import Foundation
import ShelfScan
@testable import ShelfScanVision
import XCTest

/// Apple's DETR-ResNet50 semantic segmentation (scripts/fetch_models.sh). Tests skip when the model is not downloaded.
enum DETRModel {
    static let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("../../../models/DETRResnet50SemanticSegmentationF16.mlpackage").standardized

    static func detector(minPixels: Int = 150, includeStuff: Bool = false, skip: Set<String> = [], units: MLComputeUnits = .all) throws -> CoreMLSegmentationDetector {
        guard FileManager.default.fileExists(atPath: url.path) else { throw XCTSkip("run scripts/fetch_models.sh") }
        return try CoreMLSegmentationDetector(modelURL: url, computeUnits: units, minPixels: minPixels, includeStuff: includeStuff, skipLabels: skip)
    }
}

final class CoreMLDetectorTests: XCTestCase {
    func testLabelsComeFromModelMetadata() throws {
        let d = try DETRModel.detector()
        XCTAssertEqual(d.labels.first, "--")
        XCTAssertEqual(d.labels[1], "person")
        XCTAssertEqual(d.labels[44], "bottle")
        XCTAssertGreaterThan(d.labels.count, 180)
    }

    /// Region extraction on a hand-made 20x10 class map: two separate bottles, one person (skipped), a wall (stuff).
    func testRegionsSplitComponentsSkipLabelsAndStuff() throws {
        let d = try DETRModel.detector(minPixels: 4, skip: ["person"])
        let w = 20, h = 10
        var m = [Int32](repeating: 0, count: w * h)
        func fill(_ x0: Int, _ y0: Int, _ x1: Int, _ y1: Int, _ c: Int32) { for y in y0...y1 { for x in x0...x1 { m[y * w + x] = c } } }
        fill(0, 0, 19, 1, 199)   // "wall (other)" across the top: stuff
        fill(2, 3, 4, 8, 44)     // bottle A
        fill(8, 3, 9, 6, 44)     // bottle B, not touching A
        fill(14, 2, 17, 9, 1)    // person
        fill(11, 8, 11, 8, 47)   // a 1-pixel cup: below minPixels
        let dets = d.regions(m, w, h)
        XCTAssertEqual(dets.map(\.label).sorted(), ["bottle", "bottle"])
        let a = try XCTUnwrap(dets.first { $0.bbox.x < 0.15 })
        XCTAssertEqual(a.bbox.x, 2 / 20, accuracy: 1e-6)
        XCTAssertEqual(a.bbox.y, 3 / 10, accuracy: 1e-6)
        XCTAssertEqual(a.bbox.w, 3 / 20, accuracy: 1e-6)
        XCTAssertEqual(a.bbox.h, 6 / 10, accuracy: 1e-6)
        XCTAssertEqual(a.mask?.data.count, 3 * 6)
        XCTAssertTrue(a.mask?.data.allSatisfy { $0 == 255 } ?? false)
        // stuff on request
        let withStuff = try DETRModel.detector(minPixels: 4, includeStuff: true, skip: ["person"]).regions(m, w, h)
        XCTAssertTrue(withStuff.contains { $0.label == "wall (other)" })
    }

    /// The real model on a fixture frame: runs, returns well-formed detections, timing printed. The synthetic scene has
    /// no COCO objects, so what it finds there says nothing about quality; real frames are the 1.7 bake-off.
    func testRunsOnFixtureFrame() throws {
        let m = try Fixtures.manifest()
        let frame = try Fixtures.frame(m.scene.frames[0].file)
        for (name, units) in [("all", MLComputeUnits.all), ("cpu+ane", .cpuAndNeuralEngine), ("cpu+gpu", .cpuAndGPU)] {
            try timeRun(name, try DETRModel.detector(units: units), frame)
        }
    }

    private func timeRun(_ units: String, _ d: CoreMLSegmentationDetector, _ frame: Frame) throws {
        _ = try d.detect(frame: frame)   // warm-up (model load onto the ANE / GPU)
        let t0 = Date()
        let n = 10
        var last: [Detection] = []
        for _ in 0..<n { last = try d.detect(frame: frame) }
        let ms = Date().timeIntervalSince(t0) * 1000 / Double(n)
        print("DETR units=\(units) ms_per_frame=\(String(format: "%.1f", ms)) detections=\(last.count) labels=\(last.map(\.label))")
        for det in last {
            XCTAssertTrue(det.bbox.x >= 0 && det.bbox.y >= 0 && det.bbox.x + det.bbox.w <= 1.0001 && det.bbox.y + det.bbox.h <= 1.0001)
            XCTAssertEqual(det.mask.map { Int($0.width * $0.height) }, det.mask?.data.count)
        }
    }
}
