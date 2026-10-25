// Detector over a Core ML semantic-segmentation model, built for Apple's DETR-ResNet50 conversion
// (apple/coreml-detr-semantic-segmentation, Apache-2.0; `scripts/fetch_models.sh`). The model labels every pixel of a
// 448x448 image with a COCO-panoptic class; each connected region of one class becomes a Detection (box, mask, label).
//
// * The frame is scaled to the model input with `.scaleFill`, so the class map covers the whole frame and boxes need no
//   crop correction (the aspect ratio is distorted for the model; the model card evaluated center crops instead).
// * "Stuff" classes (wall, floor, table, curtain, ...) are skipped unless `includeStuff`; they are surfaces, not objects.
// * Semantic segmentation has no instances: two touching objects of the same class come out as one region.
// * The model outputs class ids only (argmax), no probabilities, so every detection gets the same `score`.
import CoreML
import Foundation
import ShelfScan
@preconcurrency import Vision

public final class CoreMLSegmentationDetector: Detector {
    public let labels: [String]
    private let model: VNCoreMLModel
    private let minPixels: Int
    private let includeStuff: Bool
    private let skip: Set<String>
    private let score: Float
    /// COCO panoptic ids above this are "stuff" (banner, wall, floor, ...); 1...90 are "things".
    private static let lastThingId = 90

    /// `modelURL`: a `.mlpackage`, `.mlmodel` (compiled on first use) or `.mlmodelc`. `minPixels` is in model-output
    /// pixels (448x448 for DETR). `skipLabels`: class names never reported (e.g. "person").
    public init(modelURL: URL, computeUnits: MLComputeUnits = .cpuAndNeuralEngine, minPixels: Int = 150, includeStuff: Bool = false,
                skipLabels: Set<String> = [], score: Float = 0.9) throws {
        let cfg = MLModelConfiguration()
        cfg.computeUnits = computeUnits
        let compiled = modelURL.pathExtension == "mlmodelc" ? modelURL : try Self.compiledURL(for: modelURL)
        let ml = try MLModel(contentsOf: compiled, configuration: cfg)
        model = try VNCoreMLModel(for: ml)
        labels = Self.readLabels(ml)
        self.minPixels = minPixels
        self.includeStuff = includeStuff
        self.skip = skipLabels
        self.score = score
    }

    /// Compiling an .mlpackage takes seconds; keep the result next to the system's caches, keyed by path and mtime.
    private static func compiledURL(for src: URL) throws -> URL {
        let attrs = try FileManager.default.attributesOfItem(atPath: src.appendingPathComponent(src.pathExtension == "mlpackage" ? "Manifest.json" : "").path)
        let stamp = Int((attrs[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0)
        let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("shelfscan-coreml/\(abs(src.path.hashValue))-\(stamp)", isDirectory: true)
        let dst = cache.appendingPathComponent(src.deletingPathExtension().lastPathComponent + ".mlmodelc")
        if FileManager.default.fileExists(atPath: dst.path) { return dst }
        let tmp = try MLModel.compileModel(at: src)
        try FileManager.default.createDirectory(at: cache, withIntermediateDirectories: true)
        try? FileManager.default.removeItem(at: dst)
        try FileManager.default.moveItem(at: tmp, to: dst)
        return dst
    }

    /// Apple's conversions keep the class names in creator metadata (`com.apple.coreml.model.preview.params`).
    private static func readLabels(_ ml: MLModel) -> [String] {
        guard let meta = ml.modelDescription.metadata[.creatorDefinedKey] as? [String: String],
              let params = meta["com.apple.coreml.model.preview.params"]?.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: params) as? [String: Any],
              let labels = obj["labels"] as? [String] else { return [] }
        return labels
    }

    public func detect(frame: Frame) throws -> [Detection] {
        guard let cg = frame.cgImage() else {
            throw ShelfError.InvalidInput(msg: "CoreMLSegmentationDetector: unsupported pixel format")
        }
        let request = VNCoreMLRequest(model: model)
        request.imageCropAndScaleOption = .scaleFill
        try VNImageRequestHandler(cgImage: cg, options: [:]).perform([request])
        guard let obs = request.results?.first as? VNCoreMLFeatureValueObservation, let arr = obs.featureValue.multiArrayValue,
              arr.shape.count >= 2 else {
            throw ShelfError.Backend(backend: "coreml-segmentation", msg: "model did not return a class map")
        }
        let h = arr.shape[arr.shape.count - 2].intValue, w = arr.shape[arr.shape.count - 1].intValue
        var classes = [Int32](repeating: 0, count: w * h)
        if arr.dataType == .int32 {
            let p = arr.dataPointer.assumingMemoryBound(to: Int32.self)
            let sy = arr.strides[arr.strides.count - 2].intValue, sx = arr.strides[arr.strides.count - 1].intValue
            for y in 0..<h { for x in 0..<w { classes[y * w + x] = p[y * sy + x * sx] } }
        } else {
            for y in 0..<h { for x in 0..<w { classes[y * w + x] = arr[[y, x] as [NSNumber]].int32Value } }
        }
        return regions(classes, w, h)
    }

    /// Connected components (4-neighbour) per class id, then box + mask per component.
    func regions(_ classes: [Int32], _ w: Int, _ h: Int) -> [Detection] {
        var seen = [Bool](repeating: false, count: w * h)
        var out: [Detection] = []
        var stack: [Int] = []
        var pixels: [Int] = []
        for start in 0..<(w * h) where !seen[start] {
            let c = classes[start]
            seen[start] = true
            let id = Int(c)
            let name = id >= 0 && id < labels.count ? labels[id] : "class \(id)"
            let wanted = id > 0 && name != "--" && (includeStuff || id <= Self.lastThingId) && !skip.contains(name)
            stack.removeAll(keepingCapacity: true)
            pixels.removeAll(keepingCapacity: true)
            stack.append(start)
            var x0 = w, y0 = h, x1 = -1, y1 = -1
            while let i = stack.popLast() {
                pixels.append(i)
                let x = i % w, y = i / w
                x0 = min(x0, x); x1 = max(x1, x); y0 = min(y0, y); y1 = max(y1, y)
                for n in [x > 0 ? i - 1 : -1, x < w - 1 ? i + 1 : -1, y > 0 ? i - w : -1, y < h - 1 ? i + w : -1] where n >= 0 && !seen[n] && classes[n] == c {
                    seen[n] = true
                    stack.append(n)
                }
            }
            guard wanted, pixels.count >= minPixels else { continue }
            let bw = x1 - x0 + 1, bh = y1 - y0 + 1
            var mask = Data(count: bw * bh)
            for i in pixels { mask[(i / w - y0) * bw + (i % w - x0)] = 255 }
            out.append(Detection(
                bbox: BBox(x: Float(x0) / Float(w), y: Float(y0) / Float(h), w: Float(bw) / Float(w), h: Float(bh) / Float(h)),
                score: score, label: name, mask: Mask(width: UInt32(bw), height: UInt32(bh), data: mask)))
        }
        return out
    }
}
