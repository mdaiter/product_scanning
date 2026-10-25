// Compiles on macOS 26 / Xcode 26; unit-tested on synthetic fixtures only (docs/results/phase1-vision-unit.md).
// Hypotheses to test (PLAN.md section 10): Vision reports UPC-A as EAN-13 with a leading 0; the engine
// normalises 12/13/14 digit GTINs, so either way should validate. Compare decode rate with the rxing default.
import Foundation
import ShelfScan
import Vision

public final class VisionBarcodeScanner: BarcodeScanner {
    public init() {}

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
