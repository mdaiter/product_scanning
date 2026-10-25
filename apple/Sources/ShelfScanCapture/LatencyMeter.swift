import CoreMedia
import Foundation

/// Milliseconds on the host clock, the clock `CameraSource` stamps frames with. Compare `Overlay.timestampMs` against it
/// for the glass-to-box latency (docs/events.md, "Measuring latency").
public func hostClockMs() -> UInt64 {
    UInt64(max(0, CMClockGetTime(CMClockGetHostTimeClock()).seconds * 1000))
}

/// Thread-safe sample window with percentiles. Cheap enough to call from the engine's listener thread.
public final class LatencyMeter: @unchecked Sendable {
    private let lock = NSLock()
    private var samples: [Double] = []
    private var next = 0
    private let capacity: Int
    public private(set) var rejected = 0

    public init(capacity: Int = 5000) { self.capacity = capacity }

    /// Samples outside 0...10 s mean the two timestamps are on different clocks; they are counted, not kept.
    public func add(_ ms: Double) {
        lock.lock()
        defer { lock.unlock() }
        guard ms >= 0, ms < 10_000 else { rejected += 1; return }
        if samples.count < capacity { samples.append(ms) } else { samples[next] = ms; next = (next + 1) % capacity }
    }

    public func percentiles() -> (p50: Double, p95: Double, max: Double, n: Int) {
        lock.lock()
        let s = samples.sorted()
        lock.unlock()
        guard !s.isEmpty else { return (0, 0, 0, 0) }
        func p(_ q: Double) -> Double { s[min(s.count - 1, Int((Double(s.count - 1) * q).rounded()))] }
        return (p(0.5), p(0.95), s.last!, s.count)
    }

    public func reset() {
        lock.lock()
        samples.removeAll()
        next = 0
        rejected = 0
        lock.unlock()
    }
}
