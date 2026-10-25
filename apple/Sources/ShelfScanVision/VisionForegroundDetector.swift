// Compiles on macOS 26 / Xcode 26; unit-tested on synthetic fixtures only (docs/results/phase1-vision-unit.md).
// UNTESTED on dense racks. Foreground-instance masks find "salient objects", not products: expect merged touching
// items and missed small ones; measure against BlobDetector in the Phase 1 detector bake-off.
// Label buffer: top-left origin, aligned with the input (confirmed on synthetic frames; check on a real frame too).
import CoreVideo
import Foundation
import ShelfScan
import Vision

@available(iOS 17.0, macOS 14.0, *)
public final class VisionForegroundDetector: Detector {
    private let minPixels: Int
    public init(minPixels: Int = 64) { self.minPixels = minPixels }

    public func detect(frame: Frame) throws -> [Detection] {
        guard let cg = frame.cgImage() else {
            throw ShelfError.InvalidInput(msg: "VisionForegroundDetector: unsupported pixel format")
        }
        let request = VNGenerateForegroundInstanceMaskRequest()
        try VNImageRequestHandler(cgImage: cg, options: [:]).perform([request])
        guard let obs = request.results?.first else { return [] }

        let labels = obs.instanceMask
        CVPixelBufferLockBaseAddress(labels, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(labels, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(labels) else { return [] }
        let w = CVPixelBufferGetWidth(labels), h = CVPixelBufferGetHeight(labels)
        let rowBytes = CVPixelBufferGetBytesPerRow(labels)
        let px = base.assumingMemoryBound(to: UInt8.self)

        struct Box { var x0: Int; var y0: Int; var x1: Int; var y1: Int; var n: Int }
        var boxes: [UInt8: Box] = [:]
        for y in 0..<h {
            for x in 0..<w {
                let l = px[y * rowBytes + x]
                if l == 0 { continue }
                if var b = boxes[l] {
                    b.x0 = min(b.x0, x); b.y0 = min(b.y0, y); b.x1 = max(b.x1, x); b.y1 = max(b.y1, y); b.n += 1
                    boxes[l] = b
                } else {
                    boxes[l] = Box(x0: x, y0: y, x1: x, y1: y, n: 1)
                }
            }
        }
        return boxes.compactMap { (label, b) -> Detection? in
            guard b.n >= minPixels else { return nil }
            let bw = b.x1 - b.x0 + 1, bh = b.y1 - b.y0 + 1
            var mask = Data(count: bw * bh)
            for y in 0..<bh {
                for x in 0..<bw where px[(b.y0 + y) * rowBytes + b.x0 + x] == label { mask[y * bw + x] = 255 }
            }
            return Detection(
                bbox: BBox(x: Float(b.x0) / Float(w), y: Float(b.y0) / Float(h), w: Float(bw) / Float(w), h: Float(bh) / Float(h)),
                score: 0.9, label: "product",
                mask: Mask(width: UInt32(bw), height: UInt32(bh), data: mask))
        }
    }
}
