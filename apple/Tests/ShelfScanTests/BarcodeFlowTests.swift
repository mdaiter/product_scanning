import CoreGraphics
import Foundation
import ShelfScan
import ShelfScanVision
import XCTest

/// A CGImage scaled into a camera-like frame: BGRA, padded rows, as CameraSource delivers it.
func cameraFrame(_ img: CGImage, width: Int = 1280, height: Int = 720, timestampMs: UInt64) -> Frame {
    let stride = width * 4 + 64
    var data = Data(count: stride * height)
    data.withUnsafeMutableBytes { (buf: UnsafeMutableRawBufferPointer) in
        let ctx = CGContext(data: buf.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: stride,
                            space: CGColorSpaceCreateDeviceRGB(),
                            bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
        ctx.interpolationQuality = .high
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: width, height: height))
    }
    return Frame(width: UInt32(width), height: UInt32(height), format: .bgra8, data: data, stride: UInt32(stride), timestampMs: timestampMs)
}

/// The flip-for-barcode flow through the real engine: discover items, focus one, show its barcode, get a validated GTIN
/// (two agreeing reads) and Confirmed(barcode). Run with both scanners.
final class BarcodeFlowTests: XCTestCase {
    func testVisionScannerConfirmsFocusedItem() async throws { try await flow(VisionBarcodeScanner(), name: "vision") }
    func testRxingScannerConfirmsFocusedItem() async throws { try await flow(RxingScanner(), name: "rxing") }

    private func flow(_ scanner: BarcodeScanner, name: String) async throws {
        let m = try Fixtures.manifest()
        let collector = Collector()
        let engine = try EngineBuilder(config: replayConfig()).barcodeScanner(s: scanner).listener(l: collector).build()
        defer { engine.shutdown() }

        var t: UInt64 = 1000
        for f in m.scene.frames {
            feed(engine, cameraFrame(try Fixtures.cgImage(f.file), timestampMs: t))
            t += 170
        }
        _ = try await engine.flush()
        let items = try await engine.catalog(includeHidden: false)
        XCTAssertEqual(items.count, 3)
        let target = try XCTUnwrap(items.first)
        XCTAssertTrue(target.needsBarcode, "catalog mode asks for a barcode")

        try await engine.focusItem(itemId: target.id)
        let card = try Fixtures.cgImage("barcode-m3-b0.png")
        let t0 = Date()
        for _ in 0..<8 {
            // camera pace: the barcode path runs at most every 120 ms on its own slot, so frames pushed faster than a
            // camera delivers them are (correctly) skipped by it
            feed(engine, cameraFrame(card, timestampMs: t))
            t += 170
            try await Task.sleep(nanoseconds: 120_000_000)
        }
        _ = try await engine.flush()
        let ms = Date().timeIntervalSince(t0) * 1000

        let fetched = try await engine.item(itemId: target.id)
        let item = try XCTUnwrap(fetched)
        print(String(format: "BARCODE_FLOW scanner=%@ gtin=%@ state=%@ ms=%.0f", name, item.gtin ?? "nil", "\(item.state)", ms))
        XCTAssertEqual(item.gtin, m.barcodes.gtin)
        XCTAssertEqual(item.state, .confirmed(by: .barcode))
        XCTAssertFalse(item.needsBarcode)
        XCTAssertTrue(collector.all.contains { if case .barcodeRead(let id, let g) = $0 { return id == target.id && g == m.barcodes.gtin } else { return false } })
    }

    /// Without a focused item the barcode path is idle by design (reads are bound to the focused item).
    func testNoFocusNoRead() async throws {
        let collector = Collector()
        let engine = try EngineBuilder(config: replayConfig()).barcodeScanner(s: VisionBarcodeScanner()).listener(l: collector).build()
        defer { engine.shutdown() }
        let card = try Fixtures.cgImage("barcode-m3-b0.png")
        for i in 0..<6 { feed(engine, cameraFrame(card, timestampMs: 1000 + UInt64(i) * 170)) }
        _ = try await engine.flush()
        await fulfillment(of: [collector.flushed], timeout: 10)
        XCTAssertFalse(collector.all.contains { if case .barcodeRead = $0 { return true } else { return false } })
    }
}
