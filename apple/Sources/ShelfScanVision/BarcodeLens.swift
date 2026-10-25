// Barcode mode: read QR codes and product barcodes straight from raw camera frames, outside the engine, and say where
// the code is so a UI can highlight and magnify it (the "WeChat zoom-in" pattern).
//
// One Vision request on the full-resolution frame. Software zoom was tried and removed: cropping + upscaling the centre
// or the last code's area read nothing smaller than the full frame did (34 px sharp / 48 px soft QR either way, Vision
// already reads at full resolution). Reading smaller codes needs optical zoom (videoZoomFactor, iPhone; Phase 2).
// All coordinates are normalised with a TOP-LEFT origin (Vision's are bottom-left; converted here).
import CoreGraphics
import Foundation
import Vision

public struct BarcodeHit: Equatable {
    public let payload: String
    /// Vision symbology without the "VNBarcodeSymbology" prefix, e.g. "QR", "EAN13".
    public let symbology: String
    /// top-left, top-right, bottom-right, bottom-left; normalised to the whole frame, top-left origin.
    public let corners: [CGPoint]
    public let box: CGRect

    public var isProductCode: Bool { ["EAN13", "EAN8", "UPCE", "ITF14"].contains(symbology) }
    /// GTIN check digit (mod 10) for 8 / 12 / 13 / 14 digit codes; nil when the payload is not such a number.
    public var gtinValid: Bool? { BarcodeLens.gtinCheck(payload) }
}

/// Immutable after init; each read makes its own Vision request, so one lens can serve any thread.
public final class BarcodeLens: @unchecked Sendable {
    private let symbologies: [VNBarcodeSymbology]

    public init(symbologies: [VNBarcodeSymbology] = [.qr, .ean13, .ean8, .upce, .code128, .code39, .itf14, .dataMatrix, .pdf417, .aztec],
                warmUp: Bool = true) {
        self.symbologies = symbologies
        if warmUp, let ctx = CGContext(data: nil, width: 64, height: 64, bitsPerComponent: 8, bytesPerRow: 256,
                                       space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue),
           let blank = ctx.makeImage() {
            _ = try? detect(blank)   // Vision's first barcode request is ~900 ms; pay it now, not on the first frame
        }
    }

    /// All codes found in this frame, largest first.
    public func read(_ frame: CGImage) throws -> [BarcodeHit] {
        try detect(frame).compactMap { o -> BarcodeHit? in
            guard let payload = o.payloadStringValue, !payload.isEmpty else { return nil }
            let corners = [o.topLeft, o.topRight, o.bottomRight, o.bottomLeft].map { CGPoint(x: $0.x, y: 1 - $0.y) }
            let xs = corners.map(\.x), ys = corners.map(\.y)
            let box = CGRect(x: xs.min()!, y: ys.min()!, width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
            let sym = o.symbology.rawValue.replacingOccurrences(of: "VNBarcodeSymbology", with: "")
            return BarcodeHit(payload: payload, symbology: sym, corners: corners, box: box)
        }
        .sorted { $0.box.width * $0.box.height > $1.box.width * $1.box.height }
    }

    private func detect(_ img: CGImage) throws -> [VNBarcodeObservation] {
        let request = VNDetectBarcodesRequest()
        request.symbologies = symbologies
        try VNImageRequestHandler(cgImage: img, options: [:]).perform([request])
        return request.results ?? []
    }

    /// The code and a margin around it, cut from the frame at full resolution, for a magnified view.
    public static func zoomImage(_ frame: CGImage, hit: BarcodeHit, margin: CGFloat = 0.25) -> CGImage? {
        let w = CGFloat(frame.width), h = CGFloat(frame.height)
        let b = hit.box.insetBy(dx: -hit.box.width * margin, dy: -hit.box.height * margin)
            .intersection(CGRect(x: 0, y: 0, width: 1, height: 1))
        return frame.cropping(to: CGRect(x: b.minX * w, y: b.minY * h, width: b.width * w, height: b.height * h).integral)
    }

    /// Standard GS1 mod-10 check digit.
    public static func gtinCheck(_ s: String) -> Bool? {
        guard [8, 12, 13, 14].contains(s.count), s.allSatisfy(\.isNumber) else { return nil }
        let d = s.compactMap { $0.wholeNumberValue }
        let body = d.dropLast().reversed()
        let sum = body.enumerated().reduce(0) { $0 + $1.element * ($1.offset % 2 == 0 ? 3 : 1) }
        return (10 - sum % 10) % 10 == d.last!
    }
}

/// A code is "read" once the same payload shows up in `votes` consecutive frames (like the engine's two agreeing reads),
/// so a misread in one frame is never presented.
public struct ReadStabilizer {
    public let votes: Int
    private var last: String?
    private var count = 0

    public init(votes: Int = 2) { self.votes = votes }

    /// Feed the best payload of a frame (nil = nothing read). Returns the payload when it becomes confirmed.
    public mutating func add(_ payload: String?) -> String? {
        guard let p = payload else { last = nil; count = 0; return nil }
        if p == last { count += 1 } else { last = p; count = 1 }
        return count == votes ? p : nil
    }
}
