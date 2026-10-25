import Foundation
import ShelfScan

/// Local first, cloud second. MobileCLIP ranks the masked crop on the device; if the best word is a non-product
/// (a person, a hand, a screen...) the crop is NOT sent and the local answer is returned, which keeps people out of cloud
/// requests (CLAUDE.md invariant 2: masked product crops only). Otherwise the cloud guesser (e.g. `AnthropicGuesser`)
/// names the product, with the local top words passed as a hint in `detectorLabel`.
/// A cloud failure falls back to the local guess and says so in the attributes; it is never silent.
public final class HybridGuesser: ProductGuesser {
    private let local: CLIPZeroShotGuesser
    private let cloud: ProductGuesser
    public let notProducts: Set<String>
    private let gate: Float
    private let hints: Int

    /// `gate`: the probability the top non-product word needs before the crop is kept local.
    public init(local: CLIPZeroShotGuesser, cloud: ProductGuesser,
                notProducts: Set<String> = ["person", "hand", "face", "pet", "television", "computer monitor", "wall", "curtain", "table surface"],
                gate: Float = 0.4, hints: Int = 3) {
        self.local = local
        self.cloud = cloud
        self.notProducts = notProducts
        self.gate = gate
        self.hints = hints
    }

    /// Decision for one ranked crop: `nil` = send to the cloud, otherwise the non-product word that keeps it local.
    public func keepLocal(_ ranked: [(String, Float)]) -> String? {
        guard let top = ranked.first, notProducts.contains(top.0), top.1 >= gate else { return nil }
        return top.0
    }

    public func guess(request: GuessRequest) throws -> ProductGuess {
        let ranked = try local.rank(image: request.image)
        let hint = ranked.prefix(hints).map { "\($0.0) \(Int(($0.1 * 100).rounded()))%" }.joined(separator: ", ")
        if let word = keepLocal(ranked) {
            var g = try local.guess(request: request, ranked: ranked)
            g.attributes.append(Attribute(name: "Cloud", value: "not sent: looks like a \(word) (on-device: \(hint))"))
            return g
        }
        var req = request
        req.detectorLabel = "\(request.detectorLabel); an on-device image model suggests: \(hint) (a hint, may be wrong)"
        do {
            var g = try cloud.guess(request: req)
            g.attributes.append(Attribute(name: "On-device guess", value: hint))
            return g
        } catch {
            var g = try local.guess(request: request, ranked: ranked)
            g.attributes.append(Attribute(name: "Cloud", value: "failed, showing the on-device guess: \(error)"))
            return g
        }
    }
}
