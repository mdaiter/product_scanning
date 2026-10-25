// MobileCLIP (apple/coreml-mobileclip) image + text towers: an Embedder for SKU clustering and a zero-shot ProductGuesser.
// LICENCE: the weights are an "Apple Machine Learning Research Model": research use only, no product development or
// commercial use (github.com/apple/ml-mobileclip LICENSE_MODELS). Matt chose it for the prototype on 2026-10-25;
// docs/results/design-segment-embed-describe.md lists SigLIP 2 (Apache-2.0) as the commercial-safe replacement.
import CoreGraphics
import CoreML
import Foundation
import ShelfScan

/// Shared model pair. Image input 256x256 (the crop is letterboxed onto white first, so it is not distorted or cropped);
/// both towers return 512 floats, normalised here to unit length.
public final class MobileCLIP: @unchecked Sendable {
    public enum Variant: String, CaseIterable { case s0, s2 }

    private let image: MLModel
    private let text: MLModel?
    private let tokenizer: CLIPTokenizer?
    private let side: Int
    private let lock = NSLock()   // MLModel predictions are serialised; the engine calls plugins from a thread pool

    /// `modelsDir` holds `mobileclip_<variant>_image.mlpackage`, optionally `..._text.mlpackage` and `clip-tokenizer/`.
    public init(modelsDir: URL, variant: Variant = .s2, withText: Bool = true, computeUnits: MLComputeUnits = .cpuAndNeuralEngine) throws {
        image = try CoreMLLoader.load(modelsDir.appendingPathComponent("mobileclip_\(variant.rawValue)_image.mlpackage"), computeUnits: computeUnits)
        if withText {
            text = try CoreMLLoader.load(modelsDir.appendingPathComponent("mobileclip_\(variant.rawValue)_text.mlpackage"), computeUnits: computeUnits)
            tokenizer = try CLIPTokenizer(directory: modelsDir.appendingPathComponent("clip-tokenizer"))
        } else {
            text = nil
            tokenizer = nil
        }
        side = image.modelDescription.inputDescriptionsByName["image"]?.imageConstraint?.pixelsWide ?? 256
    }

    public var hasText: Bool { text != nil }

    public func embed(image cg: CGImage) throws -> [Float] {
        guard let square = Self.letterbox(cg, side: side) else { throw ShelfError.InvalidInput(msg: "MobileCLIP: cannot draw the crop") }
        let input = try MLDictionaryFeatureProvider(dictionary: ["image": MLFeatureValue(cgImage: square, pixelsWide: side, pixelsHigh: side,
                                                                                         pixelFormatType: kCVPixelFormatType_32BGRA, options: nil)])
        lock.lock()
        defer { lock.unlock() }
        return try Self.unit(image.prediction(from: input))
    }

    public func embed(text s: String) throws -> [Float] {
        guard let text, let tokenizer else { throw ShelfError.InvalidInput(msg: "MobileCLIP: text tower not loaded") }
        let ids = tokenizer.tokenize(s)
        let arr = try MLMultiArray(shape: [1, NSNumber(value: ids.count)], dataType: .int32)
        for (i, v) in ids.enumerated() { arr[i] = NSNumber(value: v) }
        lock.lock()
        defer { lock.unlock() }
        return try Self.unit(text.prediction(from: MLDictionaryFeatureProvider(dictionary: ["text": MLFeatureValue(multiArray: arr)])))
    }

    private static func unit(_ out: MLFeatureProvider) throws -> [Float] {
        guard let name = out.featureNames.first, let a = out.featureValue(for: name)?.multiArrayValue else {
            throw ShelfError.Backend(backend: "mobileclip", msg: "no embedding in the output")
        }
        var v = [Float](repeating: 0, count: a.count)
        for i in 0..<a.count { v[i] = a[i].floatValue }
        let n = v.reduce(0) { $0 + $1 * $1 }.squareRoot()
        return n > 0 ? v.map { $0 / n } : v
    }

    /// Aspect-fit onto a white square (masked crops are already on white, so the padding is invisible to the model).
    static func letterbox(_ cg: CGImage, side: Int) -> CGImage? {
        guard let ctx = CGContext(data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: side * 4,
                                  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: side, height: side))
        let s = CGFloat(side) / CGFloat(max(cg.width, cg.height))
        let w = CGFloat(cg.width) * s, h = CGFloat(cg.height) * s
        ctx.interpolationQuality = .high
        ctx.draw(cg, in: CGRect(x: (CGFloat(side) - w) / 2, y: (CGFloat(side) - h) / 2, width: w, height: h))
        return ctx.makeImage()
    }
}

/// Embedder over MobileCLIP's image tower (512-d unit vectors). Expect the engine's masked crop (`cutoutOnWhite`).
public final class MobileCLIPEmbedder: Embedder {
    private let clip: MobileCLIP
    public init(clip: MobileCLIP) { self.clip = clip }

    public func embed(image: Frame) throws -> [Float] {
        guard let cg = image.cgImage() else { throw ShelfError.InvalidInput(msg: "MobileCLIPEmbedder: unsupported pixel format") }
        return try clip.embed(image: cg)
    }
}

/// Zero-shot "what is it": cosine between the crop and "a photo of a <noun>" for a product vocabulary, softmax at CLIP's
/// logit scale (100). Never certain: the confidence is the softmax probability over this vocabulary only, so it means
/// "best of these words", not "correct". Rejected titles are skipped, as the state machine requires.
public final class CLIPZeroShotGuesser: ProductGuesser {
    private let clip: MobileCLIP
    public let vocabulary: [String]
    private let textVectors: [[Float]]
    private let template: String

    public init(clip: MobileCLIP, vocabulary: [String] = ProductVocabulary.default, template: String = "a photo of a {}") throws {
        self.clip = clip
        self.vocabulary = vocabulary
        self.template = template
        textVectors = try vocabulary.map { try clip.embed(text: template.replacingOccurrences(of: "{}", with: $0)) }
    }

    /// (noun, probability) best first.
    public func rank(_ imageVector: [Float]) -> [(String, Float)] {
        let logits = textVectors.map { t in 100 * zip(t, imageVector).reduce(0) { $0 + $1.0 * $1.1 } }
        let m = logits.max() ?? 0
        let e = logits.map { exp($0 - m) }
        let z = e.reduce(0, +)
        return zip(vocabulary, e.map { $0 / z }).sorted { $0.1 > $1.1 }
    }

    public func rank(image: Frame) throws -> [(String, Float)] {
        guard let cg = image.cgImage() else { throw ShelfError.InvalidInput(msg: "CLIPZeroShotGuesser: unsupported pixel format") }
        return rank(try clip.embed(image: cg))
    }

    public func guess(request: GuessRequest) throws -> ProductGuess {
        try guess(request: request, ranked: try rank(image: request.image))
    }

    func guess(request: GuessRequest, ranked: [(String, Float)]) throws -> ProductGuess {
        let colour = request.dominantColors.first
        func title(_ noun: String) -> String { colour.map { "\($0.capitalized) \(noun)" } ?? noun.capitalized }
        let excluded = Set(request.excludedTitles.map { $0.lowercased() })
        guard let pick = ranked.first(where: { !excluded.contains(title($0.0).lowercased()) }) else {
            return ProductGuess(title: "Unidentified product #\(request.excludedTitles.count + 1)", brand: nil, category: nil, attributes: [], confidence: 0)
        }
        let alternatives = ranked.prefix(4).filter { $0.0 != pick.0 }.map { "\($0.0) \(Int(($0.1 * 100).rounded()))%" }.joined(separator: ", ")
        var attributes = request.dominantColors.prefix(2).map { Attribute(name: "Color", value: $0) }
        attributes.append(Attribute(name: "Other guesses", value: alternatives))
        return ProductGuess(title: title(pick.0), brand: nil, category: nil, attributes: attributes, confidence: pick.1)
    }
}

/// A starter vocabulary of things people sell from a table. Editable per store (the demo has a field for extra words).
public enum ProductVocabulary {
    public static let `default`: [String] = [
        // people and non-merchandise, so they win over a product noun when that is what the crop shows
        "person", "hand", "face", "pet", "houseplant", "television", "computer monitor", "wall", "curtain", "table surface",
        // kitchen and home
        "mug", "coffee cup", "drinking glass", "wine glass", "water bottle", "glass bottle", "plastic bottle", "jar", "bowl", "plate",
        "teapot", "kettle", "cutting board", "kitchen knife", "spoon", "fork", "spatula", "pan", "pot", "food container",
        "candle", "vase", "picture frame", "clock", "lamp", "cushion", "blanket", "towel", "basket", "storage box",
        "soap bar", "bottle of shampoo", "bottle of lotion", "toothbrush", "toothpaste", "perfume bottle", "makeup palette", "lipstick", "nail polish", "hairbrush",
        // food and drink packaging
        "cereal box", "snack bag", "bag of chips", "chocolate bar", "candy", "cookie package", "can of soda", "beer can", "beer bottle", "wine bottle",
        "juice carton", "milk carton", "coffee beans bag", "tea box", "spice jar", "sauce bottle", "canned food", "pasta package", "box of crackers", "energy drink",
        // clothing and accessories
        "t-shirt", "sweater", "hoodie", "jacket", "dress", "pair of jeans", "scarf", "hat", "cap", "beanie",
        "sock", "glove", "shoe", "sneaker", "boot", "sandal", "handbag", "backpack", "wallet", "belt",
        "sunglasses", "watch", "necklace", "bracelet", "ring", "earrings", "keychain", "tote bag", "umbrella", "tie",
        // electronics
        "smartphone", "phone case", "headphones", "earbuds", "charger", "cable", "power bank", "speaker", "laptop", "tablet",
        "computer mouse", "keyboard", "game controller", "video game", "camera", "remote control", "light bulb", "battery pack", "usb stick", "smartwatch",
        // books, paper, toys, crafts
        "book", "notebook", "magazine", "greeting card", "poster", "pen", "pencil", "marker", "sticker sheet", "calendar",
        "toy", "stuffed animal", "doll", "action figure", "board game", "puzzle", "toy car", "building blocks", "ball", "kite",
        "yarn", "fabric", "sewing kit", "paint tube", "paintbrush", "ceramic figurine", "handmade pottery", "wooden carving", "knitted item", "jewelry box",
        // tools and other
        "screwdriver", "hammer", "wrench", "tape measure", "flashlight", "roll of tape", "glue", "scissors", "dog toy", "pet food bag",
        "plant pot", "seed packet", "garden tool", "first aid kit", "pill bottle", "vitamin bottle", "sports bottle", "yoga mat", "dumbbell", "tennis ball",
    ]
}
