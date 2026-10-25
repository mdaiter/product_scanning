import CoreML
import Foundation
import ShelfScan
import ShelfScanVision

/// Everything the settings panel can change. Plugins and `EngineConfig` are fixed when an engine is built, so applying
/// settings rebuilds the engine (the current scan's items are dropped). Saved in UserDefaults between launches.
struct DemoSettings: Codable, Equatable {
    // models
    var detector = DetectorChoice.vision
    var visionMasks = true
    var visionMinPixels = 64
    var blobWorkSide = 160
    var blobThreshold: Float = 30
    var detrModelPath = DemoSettings.defaultDETRPath
    var detrUnits = ComputeChoice.cpuAndNeuralEngine
    var detrMinPixels = 150
    var detrIncludeStuff = false
    /// Comma-separated class names the DETR detector never reports (e.g. "person, tv").
    var detrSkipLabels = ""
    var samRefine = false
    var samMaxPrompts = 6
    var samMinQuality: Float = 0.5
    var embedder = EmbedderChoice.featurePrint
    var guesser = GuesserChoice.placeholder
    /// Extra words for the zero-shot vocabulary (comma-separated), added to the built-in list.
    var extraVocabulary = ""
    /// Claude model id for the cloud describer; empty = the library default (cloud/anthropic.rs DEFAULT_MODEL).
    var claudeModel = ""
    /// Write every cloud request (images as files, never headers / keys) here: the 1.11 privacy audit.
    var dumpRequests = true
    var dumpDir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Documents/shelfscan/requests").path
    var scanner = ScannerChoice.vision
    // frames
    var analysisFps: Float = 10
    var analysisMaxSide = 1920   // crops are cut from the analysis frame; DETR / SAM resize to their own input anyway
    var noveltyThreshold: Float = 0.6
    var blurGateRatio: Float = 0.25
    /// Frames in flight. Latency is roughly credits x per-frame cost once the pipeline is saturated.
    var frameCredits = 4
    // detection
    var minScore: Float = 0.35
    var nmsIou: Float = 0.5
    var minBoxArea: Float = 0.002
    var backgroundArea: Float = 0.5
    var maxDetections = 300
    // tracking
    var minTrackHits = 3
    var maxTrackAge = 8
    // clustering
    var skuSimilarity: Float = 0.92
    var fixtureSimilarity: Float = 0.95
    var duplicateSimilarity: Float = 0.97
    // crops and identity
    var cutoutOnWhite = true
    var maxImagesPerItem = 4
    var jpegQuality = 85
    var describeMode = false
    var barcodeVotes = 2
    var guessRetries = 2

    /// Repo checkout's models/ (scripts/fetch_models.sh), found relative to this source file.
    static let modelsDir = URL(fileURLWithPath: #filePath).appendingPathComponent("../../../../../../models").standardized
    static let defaultDETRPath = modelsDir.appendingPathComponent("DETRResnet50SemanticSegmentationF16.mlpackage").path

    func detectorPlugin() throws -> Detector {
        let base = try baseDetector()
        guard samRefine else { return base }
        return SAMRefinedDetector(base: base, sam: try ModelCache.shared.sam(), maxPrompts: samMaxPrompts, minQuality: samMinQuality)
    }

    func embedderPlugin() throws -> Embedder {
        switch embedder {
        case .color: return ColorEmbedder()
        case .featurePrint: return VisionFeaturePrintEmbedder()
        case .mobileclipS0, .mobileclipS2: return MobileCLIPEmbedder(clip: try ModelCache.shared.clip(embedder.clipVariant!))
        }
    }

    func guesserPlugin() throws -> ProductGuesser {
        guard let v = guesser.clipVariant else { return PlaceholderGuesser() }
        let extra = extraVocabulary.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.filter { !$0.isEmpty }
        var seen = Set<String>()
        let words = (ProductVocabulary.default + extra).filter { seen.insert($0).inserted }
        let local = try ModelCache.shared.guesser(v, vocabulary: words)
        guard guesser.usesCloud else { return local }
        guard let key = Self.anthropicKey() else {
            throw ShelfError.InvalidInput(msg: "the Claude describer needs ANTHROPIC_API_KEY in the environment or a key in \(Self.keyFile.path) (mode 600)")
        }
        // Read by the Rust HTTP client when the guesser is built (cloud/http.rs DUMP_ENV).
        if dumpRequests { setenv("SHELFSCAN_DUMP_REQUESTS", dumpDir, 1) } else { unsetenv("SHELFSCAN_DUMP_REQUESTS") }
        let cloud = AnthropicGuesser(apiKey: key, model: claudeModel.isEmpty ? nil : claudeModel)
        return HybridGuesser(local: local, cloud: cloud)
    }

    /// Outside the repo, readable only by the user. The key is never logged or shown.
    static let keyFile = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config/shelfscan/anthropic_key")

    static func anthropicKey() -> String? {
        if let k = ProcessInfo.processInfo.environment["ANTHROPIC_API_KEY"], !k.isEmpty { return k }
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: keyFile.path),
              let perms = attrs[.posixPermissions] as? Int, perms & 0o077 == 0,   // refuse a key file others can read
              let k = try? String(contentsOf: keyFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), !k.isEmpty else { return nil }
        return k
    }

    private func baseDetector() throws -> Detector {
        switch detector {
        case .blob: return BlobDetector.withParams(workSide: UInt32(blobWorkSide), minThreshold: blobThreshold)
        case .vision: return VisionForegroundDetector(minPixels: visionMinPixels, emitMasks: visionMasks)
        case .detr:
            guard FileManager.default.fileExists(atPath: detrModelPath) else {
                throw ShelfError.InvalidInput(msg: "DETR model not found at \(detrModelPath). Run scripts/fetch_models.sh")
            }
            let skip = Set(detrSkipLabels.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.filter { !$0.isEmpty })
            return try CoreMLSegmentationDetector(modelURL: URL(fileURLWithPath: detrModelPath), computeUnits: detrUnits.units,
                                                  minPixels: detrMinPixels, includeStuff: detrIncludeStuff, skipLabels: skip)
        }
    }

    var engineConfig: EngineConfig {
        var c = EngineConfig(defaultMode: describeMode ? .describeFirst : .barcodeFirst)
        c.analysisFps = analysisFps
        c.analysisMaxSide = UInt32(analysisMaxSide)
        c.noveltyThreshold = noveltyThreshold
        c.blurGateRatio = blurGateRatio
        c.frameCredits = UInt32(frameCredits)
        c.minScore = minScore
        c.nmsIou = nmsIou
        c.minBoxArea = minBoxArea
        c.backgroundArea = backgroundArea
        c.maxDetections = UInt32(maxDetections)
        c.minTrackHits = UInt32(minTrackHits)
        c.maxTrackAge = UInt32(maxTrackAge)
        c.skuSimilarity = skuSimilarity
        c.fixtureSimilarity = fixtureSimilarity
        c.duplicateSimilarity = duplicateSimilarity
        c.cutoutOnWhite = cutoutOnWhite
        c.maxImagesPerItem = UInt32(maxImagesPerItem)
        c.jpegQuality = UInt8(jpegQuality)
        c.barcodeVotes = UInt32(barcodeVotes)
        c.guessRetries = UInt32(guessRetries)
        return c
    }

    var summary: String {
        let extra = detector == .vision && !visionMasks ? " (no masks)" : detector == .detr ? " (\(detrUnits.rawValue))" : ""
        return "\(detector.rawValue)\(extra)\(samRefine ? " + SAM" : "") / \(embedder.rawValue) / \(guesser.rawValue) / \(scanner.rawValue)"
    }

    private static let key = "shelfscan.macdemo.settings"

    /// Saved values over the defaults, so settings saved by an older build (fewer keys) still load.
    static func load() -> DemoSettings {
        guard let saved = UserDefaults.standard.data(forKey: key),
              let savedDict = try? JSONSerialization.jsonObject(with: saved) as? [String: Any],
              let defaults = try? JSONSerialization.jsonObject(with: JSONEncoder().encode(DemoSettings())) as? [String: Any],
              let merged = try? JSONSerialization.data(withJSONObject: defaults.merging(savedDict) { _, new in new }),
              let s = try? JSONDecoder().decode(DemoSettings.self, from: merged) else { return DemoSettings() }
        return s
    }

    func save() {
        if let d = try? JSONEncoder().encode(self) { UserDefaults.standard.set(d, forKey: Self.key) }
    }
}

enum ComputeChoice: String, CaseIterable, Identifiable, Codable {
    case cpuAndNeuralEngine = "CPU + Neural Engine", cpuAndGPU = "CPU + GPU", all = "All", cpuOnly = "CPU only"
    var id: String { rawValue }
    var units: MLComputeUnits {
        switch self {
        case .cpuAndNeuralEngine: return .cpuAndNeuralEngine
        case .cpuAndGPU: return .cpuAndGPU
        case .all: return .all
        case .cpuOnly: return .cpuOnly
        }
    }
}

/// Models are big and slow to load; keep them across engine rebuilds (Apply). The zero-shot guesser also caches its text
/// embeddings per vocabulary.
final class ModelCache: @unchecked Sendable {
    static let shared = ModelCache()
    private let lock = NSLock()
    private var sam2: SAM2Tiny?
    private var clips: [MobileCLIP.Variant: MobileCLIP] = [:]
    private var guessers: [String: CLIPZeroShotGuesser] = [:]

    func sam() throws -> SAM2Tiny {
        lock.lock(); defer { lock.unlock() }
        if let s = sam2 { return s }
        let s = try SAM2Tiny(modelsDir: DemoSettings.modelsDir)
        sam2 = s
        return s
    }

    func clip(_ v: MobileCLIP.Variant) throws -> MobileCLIP {
        lock.lock(); defer { lock.unlock() }
        if let c = clips[v] { return c }
        let c = try MobileCLIP(modelsDir: DemoSettings.modelsDir, variant: v)
        clips[v] = c
        return c
    }

    func guesser(_ v: MobileCLIP.Variant, vocabulary: [String]) throws -> CLIPZeroShotGuesser {
        let key = "\(v.rawValue)|\(vocabulary.joined(separator: ","))"
        lock.lock()
        if let g = guessers[key] { lock.unlock(); return g }
        lock.unlock()
        let g = try CLIPZeroShotGuesser(clip: try clip(v), vocabulary: vocabulary)
        lock.lock(); guessers[key] = g; lock.unlock()
        return g
    }
}
