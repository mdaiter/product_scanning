// Compiles on macOS 26 / Xcode 26; unit-tested on synthetic fixtures only (docs/results/phase1-vision-unit.md).
// Hypotheses to test (PLAN.md section 10): Vision reports UPC-A as EAN-13 with a leading 0; the engine
// normalises 12/13/14 digit GTINs, so either way should validate. Compare decode rate with the rxing default.
import CoreGraphics
import Foundation
import ShelfScan
import Vision

public final class VisionBarcodeScanner: BarcodeScanner {
    /// `warmUp`: run one scan on a blank image now. Vision's first barcode request takes ~900 ms (measured on an M3 Pro)
    /// while later ones take ~6 ms; without the warm-up that first scan holds the engine's barcode slot and the frames
    /// that arrive meanwhile are skipped, so a focused item missed its two agreeing reads (BarcodeFlowTests).
    public init(warmUp: Bool = true) {
        guard warmUp, let ctx = CGContext(data: nil, width: 64, height: 64, bitsPerComponent: 8, bytesPerRow: 256,
                                          space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue),
              let blank = ctx.makeImage() else { return }
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.ean13]
        try? VNImageRequestHandler(cgImage: blank, options: [:]).perform([request])
    }

    public func scan(frame: Frame) throws -> [BarcodeRead] {
        guard let cg = frame.cgImage() else {
            throw ShelfError.InvalidInput(msg: "VisionBarcodeScanner: unsupported pixel format")
        }
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.ean13, .ean8, .upce, .code128, .itf14]
        try VNImageRequestHandler(cgImage: cg, options: [:]).perform([request])
        return (request.results ?? []).compactMap { obs in
            guard let text = obs.payloadStringValue, !text.isEmpty else { return nil }
            return BarcodeRead(text: text, symbology: obs.symbology.rawValue)
        }
    }
}
