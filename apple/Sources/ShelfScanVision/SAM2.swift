// SAM 2.1 tiny (apple/coreml-sam2.1-tiny, Apache-2.0): pixel masks from box prompts.
// SAMRefinedDetector follows MobileSAMv2's idea: object-aware prompts (boxes from another detector) instead of a dense
// point grid, so a frame costs one image encoding plus one small decoder run per box.
import CoreGraphics
import CoreML
import Foundation
import ShelfScan
import Vision

public final class SAM2Tiny: @unchecked Sendable {
    private let encoder: MLModel
    private let prompt: MLModel
    private let decoder: MLModel
    public let inputSide = 1024
    private let lock = NSLock()

    /// Defaults from measurements on an M3 Pro (docs/results/phase1-foundation-models.md): the image encoder is fastest on
    /// the Neural Engine (120 ms vs 427 ms on the GPU), the prompt encoder and mask decoder on the GPU (7 ms vs 48 ms on the ANE).
    public init(modelsDir: URL, encoderUnits: MLComputeUnits = .cpuAndNeuralEngine, decoderUnits: MLComputeUnits = .cpuAndGPU) throws {
        encoder = try CoreMLLoader.load(modelsDir.appendingPathComponent("SAM2_1TinyImageEncoderFLOAT16.mlpackage"), computeUnits: encoderUnits)
        prompt = try CoreMLLoader.load(modelsDir.appendingPathComponent("SAM2_1TinyPromptEncoderFLOAT16.mlpackage"), computeUnits: decoderUnits)
        decoder = try CoreMLLoader.load(modelsDir.appendingPathComponent("SAM2_1TinyMaskDecoderFLOAT16.mlpackage"), computeUnits: decoderUnits)
    }

    /// Image features for one frame. The frame is stretched to 1024x1024 (scale-fill), so normalised frame coordinates map
    /// linearly onto the model's input and back.
    public final class Encoded { fileprivate let features: MLFeatureProvider; fileprivate init(_ f: MLFeatureProvider) { features = f } }

    public func encode(_ cg: CGImage) throws -> Encoded {
        let value = try MLFeatureValue(cgImage: cg, pixelsWide: inputSide, pixelsHigh: inputSide, pixelFormatType: kCVPixelFormatType_32BGRA,
                                       options: [.cropAndScale: VNImageCropAndScaleOption.scaleFill.rawValue])
        lock.lock()
        defer { lock.unlock() }
        return Encoded(try encoder.prediction(from: MLDictionaryFeatureProvider(dictionary: ["image": value])))
    }

    /// Mask for a box prompt (normalised frame coordinates). Returns the best of SAM's 3 candidate masks as a 256x256
    /// grid of logits over the whole frame, plus SAM's own quality score for it.
    public func mask(_ enc: Encoded, box: BBox) throws -> (logits: [Float], side: Int, score: Float) {
        let s = Float(inputSide)
        let pts = try MLMultiArray(shape: [1, 2, 2], dataType: .float32)
        for (i, v) in [box.x * s, box.y * s, (box.x + box.w) * s, (box.y + box.h) * s].enumerated() { pts[i] = NSNumber(value: v) }
        let labels = try MLMultiArray(shape: [1, 2], dataType: .float32)
        labels[0] = 2   // SAM: box top-left corner
        labels[1] = 3   // SAM: box bottom-right corner
        lock.lock()
        defer { lock.unlock() }
        let p = try prompt.prediction(from: MLDictionaryFeatureProvider(dictionary: ["points": MLFeatureValue(multiArray: pts),
                                                                                    "labels": MLFeatureValue(multiArray: labels)]))
        let f = enc.features
        let out = try decoder.prediction(from: MLDictionaryFeatureProvider(dictionary: [
            "image_embedding": f.featureValue(for: "image_embedding")!, "feats_s0": f.featureValue(for: "feats_s0")!,
            "feats_s1": f.featureValue(for: "feats_s1")!,
            "sparse_embedding": p.featureValue(for: "sparse_embeddings")!, "dense_embedding": p.featureValue(for: "dense_embeddings")!,
        ]))
        guard let masks = out.featureValue(for: "low_res_masks")?.multiArrayValue, let scores = out.featureValue(for: "scores")?.multiArrayValue else {
            throw ShelfError.Backend(backend: "sam2", msg: "decoder returned no masks")
        }
        var best = 0
        for k in 1..<scores.count where scores[k].floatValue > scores[best].floatValue { best = k }
        let side = masks.shape[3].intValue
        var logits = [Float](repeating: 0, count: side * side)
        let base = best * side * side
        if masks.dataType == .float16 {
            let ptr = masks.dataPointer.assumingMemoryBound(to: Float16.self)
            for i in 0..<(side * side) { logits[i] = Float(ptr[base + i]) }
        } else {
            for i in 0..<(side * side) { logits[i] = masks[base + i].floatValue }
        }
        return (logits, side, scores[best].floatValue)
    }
}

/// Runs a base detector for proposals, then replaces each box's mask (and tightens the box) with SAM's pixel mask.
/// Labels and scores come from the base detector. At most `maxPrompts` boxes are refined per frame (largest first);
/// the rest keep the base detector's mask.
public final class SAMRefinedDetector: Detector {
    private let base: Detector
    private let sam: SAM2Tiny
    private let maxPrompts: Int
    private let minQuality: Float

    public init(base: Detector, sam: SAM2Tiny, maxPrompts: Int = 8, minQuality: Float = 0.5) {
        self.base = base
        self.sam = sam
        self.maxPrompts = maxPrompts
        self.minQuality = minQuality
    }

    public func detect(frame: Frame) throws -> [Detection] {
        let proposals = try base.detect(frame: frame)
        guard !proposals.isEmpty, maxPrompts > 0 else { return proposals }
        guard let cg = frame.cgImage() else { throw ShelfError.InvalidInput(msg: "SAMRefinedDetector: unsupported pixel format") }
        let enc = try sam.encode(cg)
        let order = proposals.indices.sorted { proposals[$0].bbox.w * proposals[$0].bbox.h > proposals[$1].bbox.w * proposals[$1].bbox.h }
        var out = proposals
        for i in order.prefix(maxPrompts) {
            let m = try sam.mask(enc, box: proposals[i].bbox)
            if m.score >= minQuality, let refined = Self.detection(from: m.logits, side: m.side, within: proposals[i].bbox, like: proposals[i],
                                                                         frameWidth: Int(frame.width), frameHeight: Int(frame.height)) {
                out[i] = refined
            }
        }
        return out
    }

    /// Mask pixels (logit > 0) inside the prompt box grown by 10 %, so a stray region elsewhere in the frame is ignored.
    /// SAM's logits are a 256x256 grid over the whole frame; they are sampled bilinearly at the frame's own resolution
    /// (capped at `maxSide` px for the grown box) so the cut-out edge is smooth instead of 256-grid blocks, and the box is
    /// as tight as the frame allows.
    static func detection(from logits: [Float], side: Int, within box: BBox, like d: Detection,
                          frameWidth: Int, frameHeight: Int, maxSide: Int = 640) -> Detection? {
        let g: Float = 0.1
        let rx0 = max(0, box.x - box.w * g), ry0 = max(0, box.y - box.h * g)
        let rx1 = min(1, box.x + box.w * (1 + g)), ry1 = min(1, box.y + box.h * (1 + g))
        guard rx1 > rx0, ry1 > ry0 else { return nil }
        let pw = (rx1 - rx0) * Float(frameWidth), ph = (ry1 - ry0) * Float(frameHeight)
        let scale = min(1, Float(maxSide) / max(pw, ph))
        let w = max(1, Int((pw * scale).rounded())), h = max(1, Int((ph * scale).rounded()))
        let f = Float(side)
        func logit(_ u: Float, _ v: Float) -> Float {   // u, v normalised frame coordinates
            let gx = min(max(u * f - 0.5, 0), f - 1), gy = min(max(v * f - 0.5, 0), f - 1)
            let x0 = Int(gx), y0 = Int(gy), x1 = min(x0 + 1, side - 1), y1 = min(y0 + 1, side - 1)
            let tx = gx - Float(x0), ty = gy - Float(y0)
            let top = logits[y0 * side + x0] * (1 - tx) + logits[y0 * side + x1] * tx
            let bottom = logits[y1 * side + x0] * (1 - tx) + logits[y1 * side + x1] * tx
            return top * (1 - ty) + bottom * ty
        }
        var on = [Bool](repeating: false, count: w * h)
        var x0 = w, y0 = h, x1 = -1, y1 = -1
        for y in 0..<h {
            let v = ry0 + (Float(y) + 0.5) / Float(h) * (ry1 - ry0)
            for x in 0..<w {
                let u = rx0 + (Float(x) + 0.5) / Float(w) * (rx1 - rx0)
                if logit(u, v) > 0 {
                    on[y * w + x] = true
                    x0 = min(x0, x); x1 = max(x1, x); y0 = min(y0, y); y1 = max(y1, y)
                }
            }
        }
        guard x1 >= x0, y1 >= y0 else { return nil }
        let bw = x1 - x0 + 1, bh = y1 - y0 + 1
        var mask = Data(count: bw * bh)
        for y in 0..<bh { for x in 0..<bw where on[(y0 + y) * w + x0 + x] { mask[y * bw + x] = 255 } }
        let sx = (rx1 - rx0) / Float(w), sy = (ry1 - ry0) / Float(h)
        return Detection(bbox: BBox(x: rx0 + Float(x0) * sx, y: ry0 + Float(y0) * sy, w: Float(bw) * sx, h: Float(bh) * sy),
                         score: d.score, label: d.label, mask: Mask(width: UInt32(bw), height: UInt32(bh), data: mask))
    }
}
