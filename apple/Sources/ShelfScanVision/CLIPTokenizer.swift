import Foundation

/// OpenAI CLIP byte-level BPE tokenizer (the one MobileCLIP's text tower expects), a port of `clip/simple_tokenizer.py`
/// (MIT). Reads `vocab.json` and `merges.txt` (scripts/fetch_models.sh puts them in models/clip-tokenizer).
/// Simplification: no ftfy / HTML unescaping; text is lowercased and whitespace collapsed, which covers product nouns.
public final class CLIPTokenizer {
    public static let startToken: Int32 = 49406
    public static let endToken: Int32 = 49407
    public let contextLength = 77

    private let encoder: [String: Int32]
    private let ranks: [String: Int]   // "a b" -> merge priority
    private let byteToChar: [UInt8: Character]
    private let pattern: NSRegularExpression
    private var cache: [String: [String]] = [:]
    private let lock = NSLock()

    public init(directory: URL) throws {
        let vocab = try JSONDecoder().decode([String: Int32].self, from: Data(contentsOf: directory.appendingPathComponent("vocab.json")))
        encoder = vocab
        let merges = try String(contentsOf: directory.appendingPathComponent("merges.txt"), encoding: .utf8)
            .split(separator: "\n").filter { !$0.hasPrefix("#version") }
        var r: [String: Int] = [:]
        for (i, m) in merges.enumerated() { r[String(m)] = i }
        ranks = r
        byteToChar = Self.bytesToUnicode()
        pattern = try NSRegularExpression(
            pattern: #"<\|startoftext\|>|<\|endoftext\|>|'s|'t|'re|'ve|'m|'ll|'d|[\p{L}]+|[\p{N}]|[^\s\p{L}\p{N}]+"#,
            options: [.caseInsensitive])
    }

    /// GPT-2 / CLIP mapping of every byte to a printable unicode character.
    private static func bytesToUnicode() -> [UInt8: Character] {
        var bs: [Int] = Array(33...126) + Array(161...172) + Array(174...255)
        var cs = bs
        var n = 0
        for b in 0..<256 where !bs.contains(b) {
            bs.append(b)
            cs.append(256 + n)
            n += 1
        }
        var out: [UInt8: Character] = [:]
        for (b, c) in zip(bs, cs) { out[UInt8(b)] = Character(UnicodeScalar(c)!) }
        return out
    }

    private func bpe(_ token: String) -> [String] {
        lock.lock()
        if let hit = cache[token] { lock.unlock(); return hit }
        lock.unlock()
        var word = token.map { String($0) }
        guard !word.isEmpty else { return [] }
        word[word.count - 1] += "</w>"
        while word.count > 1 {
            var best: (rank: Int, i: Int)?
            for i in 0..<(word.count - 1) {
                if let r = ranks["\(word[i]) \(word[i + 1])"], r < (best?.rank ?? Int.max) { best = (r, i) }
            }
            guard let b = best else { break }
            let first = word[b.i], second = word[b.i + 1]
            var merged: [String] = []
            var i = 0
            while i < word.count {
                if i < word.count - 1 && word[i] == first && word[i + 1] == second {
                    merged.append(first + second)
                    i += 2
                } else {
                    merged.append(word[i])
                    i += 1
                }
            }
            word = merged
        }
        lock.lock()
        cache[token] = word
        lock.unlock()
        return word
    }

    /// Token ids without start / end markers.
    public func encode(_ text: String) -> [Int32] {
        let clean = text.lowercased().split(whereSeparator: \.isWhitespace).joined(separator: " ")
        let ns = clean as NSString
        var ids: [Int32] = []
        for m in pattern.matches(in: clean, range: NSRange(location: 0, length: ns.length)) {
            let piece = ns.substring(with: m.range)
            let mapped = String(piece.utf8.map { byteToChar[$0]! })
            for t in bpe(mapped) { if let id = encoder[t] { ids.append(id) } }
        }
        return ids
    }

    /// `[start] + tokens + [end]`, truncated / zero-padded to 77 (CLIP's context length).
    public func tokenize(_ text: String) -> [Int32] {
        var ids = [Self.startToken] + encode(text).prefix(contextLength - 2) + [Self.endToken]
        ids += [Int32](repeating: 0, count: contextLength - ids.count)
        return ids
    }
}
