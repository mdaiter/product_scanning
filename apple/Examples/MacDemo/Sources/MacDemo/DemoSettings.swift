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

    var detectorPlugin: Detector {
        switch detector {
        case .blob: return BlobDetector.withParams(workSide: UInt32(blobWorkSide), minThreshold: blobThreshold)
        case .vision: return VisionForegroundDetector(minPixels: visionMinPixels, emitMasks: visionMasks)
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

    var summary: String { "\(detector.rawValue)\(detector == .vision && !visionMasks ? " (no masks)" : "") / \(embedder.rawValue) / \(scanner.rawValue)" }

    private static let key = "shelfscan.macdemo.settings"

    static func load() -> DemoSettings {
        guard let d = UserDefaults.standard.data(forKey: key), let s = try? JSONDecoder().decode(DemoSettings.self, from: d) else { return DemoSettings() }
        return s
    }

    func save() {
        if let d = try? JSONEncoder().encode(self) { UserDefaults.standard.set(d, forKey: Self.key) }
    }
}
