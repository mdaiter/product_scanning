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
            if m.score >= minQuality, let refined = Self.detection(from: m.logits, side: m.side, within: proposals[i].bbox, like: proposals[i]) {
                out[i] = refined
            }
        }
        return out
    }

    /// Mask pixels (logit > 0) inside the prompt box grown by 10 %, so a stray region elsewhere in the frame is ignored.
    static func detection(from logits: [Float], side: Int, within box: BBox, like d: Detection) -> Detection? {
        let g: Float = 0.1
        let gx0 = max(0, Int(((box.x - box.w * g) * Float(side)).rounded(.down)))
        let gy0 = max(0, Int(((box.y - box.h * g) * Float(side)).rounded(.down)))
        let gx1 = min(side - 1, Int(((box.x + box.w * (1 + g)) * Float(side)).rounded(.up)))
        let gy1 = min(side - 1, Int(((box.y + box.h * (1 + g)) * Float(side)).rounded(.up)))
        guard gx1 > gx0, gy1 > gy0 else { return nil }
        var x0 = side, y0 = side, x1 = -1, y1 = -1
        for y in gy0...gy1 { for x in gx0...gx1 where logits[y * side + x] > 0 { x0 = min(x0, x); x1 = max(x1, x); y0 = min(y0, y); y1 = max(y1, y) } }
        guard x1 >= x0, y1 >= y0 else { return nil }
        let bw = x1 - x0 + 1, bh = y1 - y0 + 1
        var mask = Data(count: bw * bh)
        for y in 0..<bh { for x in 0..<bw where logits[(y0 + y) * side + x0 + x] > 0 { mask[y * bw + x] = 255 } }
        let f = Float(side)
        return Detection(bbox: BBox(x: Float(x0) / f, y: Float(y0) / f, w: Float(bw) / f, h: Float(bh) / f), score: d.score, label: d.label,
                         mask: Mask(width: UInt32(bw), height: UInt32(bh), data: mask))
    }
}
