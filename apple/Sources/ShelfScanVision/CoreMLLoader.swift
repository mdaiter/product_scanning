import CoreML
import Foundation

/// Loads `.mlpackage` / `.mlmodel` (compiled once, cached) or `.mlmodelc` files.
public enum CoreMLLoader {
    public static func load(_ url: URL, computeUnits: MLComputeUnits) throws -> MLModel {
        let cfg = MLModelConfiguration()
        cfg.computeUnits = computeUnits
        let compiled = url.pathExtension == "mlmodelc" ? url : try compiledURL(for: url)
        return try MLModel(contentsOf: compiled, configuration: cfg)
    }

    /// Compiling an .mlpackage takes seconds; keep the result in the user's caches, keyed by path and modification time.
    static func compiledURL(for src: URL) throws -> URL {
        guard FileManager.default.fileExists(atPath: src.path) else {
            throw NSError(domain: "CoreMLLoader", code: 1, userInfo: [NSLocalizedDescriptionKey: "model not found: \(src.path) (run scripts/fetch_models.sh)"])
        }
        let probe = src.pathExtension == "mlpackage" ? src.appendingPathComponent("Manifest.json") : src
        let stamp = Int(((try? FileManager.default.attributesOfItem(atPath: probe.path))?[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0)
        let key = src.path.utf8.reduce(UInt64(1469598103934665603)) { ($0 ^ UInt64($1)) &* 1099511628211 }   // FNV-1a: stable across launches
        let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("shelfscan-coreml/\(String(key, radix: 16))-\(stamp)", isDirectory: true)
        let dst = cache.appendingPathComponent(src.deletingPathExtension().lastPathComponent + ".mlmodelc")
        if FileManager.default.fileExists(atPath: dst.path) { return dst }
        let tmp = try MLModel.compileModel(at: src)
        try FileManager.default.createDirectory(at: cache, withIntermediateDirectories: true)
        try? FileManager.default.removeItem(at: dst)
        try FileManager.default.moveItem(at: tmp, to: dst)
        return dst
    }
}
