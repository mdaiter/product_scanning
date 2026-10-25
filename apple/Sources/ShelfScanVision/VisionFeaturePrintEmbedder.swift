// UNCOMPILED SKETCH: written without a Mac. Expect small compile fixes.
// OPEN QUESTION for the Phase 1 bake-off: Apple documents feature prints for `computeDistance(_:to:)`. Cosine on the raw
// vector (what the Rust side does) is a hypothesis. If it ranks poorly, change the engine's similarity for this embedder
// (ask Matt first: that touches the Embedder contract) or switch to a CoreML CLIP/DINO-class model.
import Accelerate
import Foundation
import ShelfScan
import Vision

public final class VisionFeaturePrintEmbedder: Embedder {
    public init() {}

    public func embed(image: Frame) throws -> [Float] {
        guard let cg = image.cgImage() else {
            throw ShelfError.InvalidInput(msg: "VisionFeaturePrintEmbedder: unsupported pixel format")
        }
        let request = VNGenerateImageFeaturePrintRequest()
        try VNImageRequestHandler(cgImage: cg, options: [:]).perform([request])
        guard let obs = request.results?.first else {
            throw ShelfError.Backend(backend: "vision-featureprint", msg: "no feature print")
        }
        guard obs.elementType == .float else {
            throw ShelfError.Backend(backend: "vision-featureprint", msg: "unexpected element type")
        }
        var v = obs.data.withUnsafeBytes { Array($0.bindMemory(to: Float.self).prefix(obs.elementCount)) }
        var norm: Float = 0
        vDSP_svesq(v, 1, &norm, vDSP_Length(v.count))
        norm = norm.squareRoot()
        if norm > 0 { var inv = 1 / norm; vDSP_vsmul(v, 1, &inv, &v, 1, vDSP_Length(v.count)) }
        return v
    }
}
