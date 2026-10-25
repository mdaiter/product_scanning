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
    var embedder = EmbedderChoice.featurePrint
    var scanner = ScannerChoice.vision
    // frames
    var analysisFps: Float = 10
    var analysisMaxSide = 960
    var noveltyThreshold: Float = 0.6
    var blurGateRatio: Float = 0.25
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
    static let defaultDETRPath = URL(fileURLWithPath: #filePath)
        .appendingPathComponent("../../../../../../models/DETRResnet50SemanticSegmentationF16.mlpackage").standardized.path

    func detectorPlugin() throws -> Detector {
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
        return "\(detector.rawValue)\(extra) / \(embedder.rawValue) / \(scanner.rawValue)"
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
