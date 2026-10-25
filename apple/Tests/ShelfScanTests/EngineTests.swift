import Foundation
import ShelfScan
import XCTest

/// Collects events from the engine thread. Fulfils `flushed` on `ScanFlushed` (docs/events.md: wait for it before asserting).
final class Collector: EventListener, @unchecked Sendable {
    private let lock = NSLock()
    private var events: [Event] = []
    let flushed = XCTestExpectation(description: "ScanFlushed")

    func onEvent(event: Event) {
        lock.lock()
        events.append(event)
        lock.unlock()
        if case .scanFlushed = event { flushed.fulfill() }
    }

    var all: [Event] {
        lock.lock()
        defer { lock.unlock() }
        return events
    }

    var discovered: [String] {
        all.compactMap { if case .itemDiscovered(let id, _) = $0 { return id } else { return nil } }
    }
}

/// Waits for a credit like the CLI's `feed`, then pushes. Replayed frames must not be dropped.
func feed(_ engine: Engine, _ frame: Frame) {
    var tries = 0
    while !engine.wantsFrame(timestampMs: frame.timestampMs) && tries < 5000 {
        usleep(2000)
        tries += 1
    }
    _ = engine.pushFrame(frame: frame)
}

/// Replayed frames repeat, so the novelty and blur gates are off (as in the CLI's offline sources).
func replayConfig() -> EngineConfig {
    var cfg = EngineConfig(defaultMode: .barcodeFirst)
    cfg.noveltyThreshold = 0
    cfg.blurGateRatio = 0
    return cfg
}

final class EngineTests: XCTestCase {
    func testSyntheticSceneDiscoversThreeItems() async throws {
        let m = try Fixtures.manifest()
        let collector = Collector()
        let engine = try EngineBuilder(config: replayConfig()).listener(l: collector).build()
        defer { engine.shutdown() }

        for f in m.scene.frames {
            feed(engine, try Fixtures.frame(f.file, timestampMs: f.timestamp_ms))
        }
        let summary = try await engine.flush()
        await fulfillment(of: [collector.flushed], timeout: 10)

        XCTAssertEqual(summary.framesProcessed, UInt64(m.scene.frames.count))
        XCTAssertEqual(Set(collector.discovered).count, m.scene.products)
        let items = try await engine.catalog(includeHidden: false)
        XCTAssertEqual(items.count, m.scene.products)
    }
}
