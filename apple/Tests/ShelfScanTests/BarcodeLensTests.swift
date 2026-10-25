import CoreGraphics
import CoreImage
import CoreImage.CIFilterBuiltins
import Foundation
@testable import ShelfScanVision
import XCTest

/// A QR code for `text`, `side` px, drawn at `at` (top-left origin, pixels) on a grey 1920x1080 "room".
func qrScene(_ text: String, side: CGFloat, at: CGPoint, size: CGSize = CGSize(width: 1920, height: 1080)) -> CGImage {
    let f = CIFilter.qrCodeGenerator()
    f.message = Data(text.utf8)
    f.correctionLevel = "M"
    let qr = f.outputImage!.transformed(by: CGAffineTransform(scaleX: side / f.outputImage!.extent.width, y: side / f.outputImage!.extent.height))
    let ci = CIContext()
    let qrCG = ci.createCGImage(qr, from: qr.extent)!
    let ctx = CGContext(data: nil, width: Int(size.width), height: Int(size.height), bitsPerComponent: 8, bytesPerRow: Int(size.width) * 4,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    ctx.setFillColor(CGColor(red: 0.55, green: 0.5, blue: 0.45, alpha: 1))
    ctx.fill(CGRect(origin: .zero, size: size))
    // white quiet zone, then the code; CGContext is bottom-left origin
    let y = size.height - at.y - side
    ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    ctx.fill(CGRect(x: at.x - side * 0.1, y: y - side * 0.1, width: side * 1.2, height: side * 1.2))
    ctx.interpolationQuality = .none
    ctx.draw(qrCG, in: CGRect(x: at.x, y: y, width: side, height: side))
    return ctx.makeImage()!
}

final class BarcodeLensTests: XCTestCase {
    func testReadsQRAndLocatesItTopLeftOrigin() throws {
        let lens = BarcodeLens(warmUp: false)
        let img = qrScene("https://example.com/item/42", side: 300, at: CGPoint(x: 200, y: 150))
        let hits = try lens.read(img)
        let h = try XCTUnwrap(hits.first)
        XCTAssertEqual(h.payload, "https://example.com/item/42")
        XCTAssertEqual(h.symbology, "QR")
        let truth = CGRect(x: 200 / 1920.0, y: 150 / 1080.0, width: 300 / 1920.0, height: 300 / 1080.0)
        let inter = h.box.intersection(truth)
        let iou = (inter.width * inter.height) / (h.box.width * h.box.height + truth.width * truth.height - inter.width * inter.height)
        print(String(format: "LENS qr box iou=%.3f", iou))
        XCTAssertGreaterThan(iou, 0.8, "box must be where the code is (top-left origin)")
        XCTAssertLessThan(h.corners[0].y, h.corners[3].y, "top-left corner above bottom-left")
        XCTAssertNil(h.gtinValid)
    }

    func testReadsEAN13AndValidatesCheckDigit() throws {
        let lens = BarcodeLens(warmUp: false)
        let h = try XCTUnwrap(try lens.read(Fixtures.cgImage("barcode-m3-b0.png")).first)
        XCTAssertEqual(h.payload, "4006381333931")
        XCTAssertTrue(h.isProductCode)
        XCTAssertEqual(h.gtinValid, true)
        XCTAssertEqual(BarcodeLens.gtinCheck("4006381333932"), false)
        XCTAssertEqual(BarcodeLens.gtinCheck("036000291452"), true)   // UPC-A
        let zoom = try XCTUnwrap(BarcodeLens.zoomImage(Fixtures.cgImage("barcode-m3-b0.png"), hit: h))
        XCTAssertGreaterThan(zoom.width, Int(h.box.width * 640))
    }

    func testStabilizerNeedsTwoAgreeingFrames() {
        var s = ReadStabilizer()
        XCTAssertNil(s.add("X"))
        XCTAssertEqual(s.add("X"), "X")
        XCTAssertNil(s.add("X"), "reported once, not every frame")
        XCTAssertNil(s.add("Y"))
        XCTAssertNil(s.add(nil))
        XCTAssertNil(s.add("Y"))
        XCTAssertEqual(s.add("Y"), "Y")
    }
}

final class BarcodeLensCameraPathTests: XCTestCase {
    /// Same path as the Mac demo's barcode mode: BGRA CVPixelBuffer → CIImage → CGImage → lens. Catches flips / channel order.
    func testPixelBufferPathKeepsGeometry() throws {
        let scene = qrScene("https://example.com/p/9", side: 260, at: CGPoint(x: 1400, y: 700))
        var pb: CVPixelBuffer?
        CVPixelBufferCreate(nil, scene.width, scene.height, kCVPixelFormatType_32BGRA,
                            [kCVPixelBufferCGImageCompatibilityKey: true, kCVPixelBufferCGBitmapContextCompatibilityKey: true] as CFDictionary, &pb)
        let buf = try XCTUnwrap(pb)
        CVPixelBufferLockBaseAddress(buf, [])
        let ctx = CGContext(data: CVPixelBufferGetBaseAddress(buf), width: scene.width, height: scene.height, bitsPerComponent: 8,
                            bytesPerRow: CVPixelBufferGetBytesPerRow(buf), space: CGColorSpaceCreateDeviceRGB(),
                            bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
        ctx.draw(scene, in: CGRect(x: 0, y: 0, width: scene.width, height: scene.height))
        CVPixelBufferUnlockBaseAddress(buf, [])

        let ci = CIImage(cvPixelBuffer: buf)
        let cg = try XCTUnwrap(CIContext().createCGImage(ci, from: ci.extent))
        let h = try XCTUnwrap(try BarcodeLens(warmUp: false).read(cg).first)
        XCTAssertEqual(h.payload, "https://example.com/p/9")
        XCTAssertEqual(h.box.midX, (1400 + 130) / 1920.0, accuracy: 0.02)
        XCTAssertEqual(h.box.midY, (700 + 130) / 1080.0, accuracy: 0.02, "top-left origin preserved through the pixel buffer")
        let zoom = try XCTUnwrap(BarcodeLens.zoomImage(cg, hit: h))
        XCTAssertGreaterThan(zoom.width, 260)
    }
}
