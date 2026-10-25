import AppKit
import AVFoundation
import Foundation
import ShelfScan
import ShelfScanCapture
import ShelfScanVision

enum DetectorChoice: String, CaseIterable, Identifiable, Codable {
    case blob = "Blob (Rust)", vision = "Vision foreground", detr = "CoreML DETR (semantic segmentation)"
    var id: String { rawValue }
}

enum EmbedderChoice: String, CaseIterable, Identifiable, Codable {
    case color = "Color (Rust)", featurePrint = "Vision feature print"
    case mobileclipS0 = "MobileCLIP S0 (research licence)", mobileclipS2 = "MobileCLIP S2 (research licence)"
    var id: String { rawValue }
    var clipVariant: MobileCLIP.Variant? { self == .mobileclipS0 ? .s0 : self == .mobileclipS2 ? .s2 : nil }
}

enum GuesserChoice: String, CaseIterable, Identifiable, Codable {
    case placeholder = "Placeholder (offline, no model)"
    case clipS0 = "MobileCLIP S0 zero-shot (research licence)", clipS2 = "MobileCLIP S2 zero-shot (research licence)"
    case clipClaude = "MobileCLIP S2 + Claude (cloud: masked crops)"
    var id: String { rawValue }
    var clipVariant: MobileCLIP.Variant? { self == .clipS0 ? .s0 : self == .placeholder ? nil : .s2 }
    var usesCloud: Bool { self == .clipClaude }
}

enum ScannerChoice: String, CaseIterable, Identifiable, Codable {
    case rxing = "rxing (Rust)", vision = "Vision barcode"
    var id: String { rawValue }
    var plugin: BarcodeScanner { self == .rxing ? RxingScanner() : VisionBarcodeScanner() }
}

/// Owns the engine and the camera; everything the UI does goes through here. Engine calls are async (UniFFI futures run
/// on the engine's runtime), so nothing here blocks the main thread except `wantsFrame`/`pushFrame`, which run on the camera queue.
@MainActor
final class DemoController: ObservableObject {
    /// What the running engine was built with.
    @Published private(set) var settings = DemoSettings.load()
    /// What the settings panel is editing; `apply()` makes it current.
    @Published var draft = DemoSettings.load()
    @Published var items: [CatalogItem] = []
    @Published var running = false
    @Published var status = "Press Start"
    @Published var session: AVCaptureSession?
    @Published var frameSize = CGSize(width: 16, height: 9)
    @Published var stats = ScanSummary(framesIn: 0, framesDropped: 0, framesSkipped: 0, framesProcessed: 0, detections: 0, tracksPromoted: 0, items: 0)
    @Published var latencyText = "latency: -"
    @Published var hitchText = "UI hitches: -"
    @Published var focused: String?

    let model = ScanModel()
    private(set) var engine: Engine?
    private var camera: CameraSource?
    private var listener: ModelListener?
    private var refreshPending = false
    private var ticker: Timer?
    private let hitches = HitchMeter()

    init() {
        model.onItemsChanged = { [weak self] in self?.scheduleRefresh() }
    }

    var pluginSummary: String { settings.summary }
    var draftChanged: Bool { draft != settings }

    /// Make the edited settings current and rebuild the engine if a scan is running.
    func apply() async {
        settings = draft
        settings.save()
        if running { await restart() } else { status = "Settings saved: \(pluginSummary)" }
    }

    func setInitial(_ s: DemoSettings) {
        settings = s
        draft = s
    }

    func start() async {
        guard !running else { return }
        guard await AVCaptureDevice.requestAccess(for: .video) else {
            status = "Camera access denied (System Settings > Privacy & Security > Camera)"
            return
        }
        do {
            let s = settings
            let listener = ModelListener(model: model)
            status = "Loading models for \(pluginSummary)…"
            // Model loading and the describer's text embeddings take seconds: build off the main thread.
            let engine = try await Task.detached(priority: .userInitiated) {
                try EngineBuilder(config: s.engineConfig)
                    .detector(d: try s.detectorPlugin())
                    .embedder(e: try s.embedderPlugin())
                    .guesser(g: try s.guesserPlugin())
                    .barcodeScanner(s: s.scanner.plugin)
                    .listener(l: listener)
                    .build()
            }.value
            let camera = CameraSource(engine: engine)
            camera.onFrameSize = { [weak self] w, h in
                Task { @MainActor in
                    self?.frameSize = CGSize(width: w, height: h)
                    self?.hitches.start()   // armed at the first frame: opening the camera is not a scanning hitch
                }
            }
            try camera.start()
            self.engine = engine
            self.listener = listener
            self.camera = camera
            session = camera.session
            running = true
            status = "Scanning with \(pluginSummary) on \(camera.deviceName ?? "camera")"
            ticker = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.tick() }
            }
        } catch {
            status = "Could not start: \(error)"
        }
    }

    func stop() async {
        guard running, let engine else { return }
        camera?.stop()
        running = false
        ticker?.invalidate()
        hitches.stop()
        _ = try? await engine.flush()
        tick()
        await refresh()
        status = "Stopped. \(latencyText)"
    }

    /// Plugin change: a new engine (plugins are fixed at build time). Items of the old engine are dropped.
    func restart() async {
        await stop()
        engine?.shutdown()
        engine = nil
        items = []
        model.boxes = []
        await start()
    }

    private func tick() {
        guard let engine, let listener else { return }
        stats = engine.stats()
        let l = listener.latency.percentiles(), u = listener.uiLatency.percentiles()
        latencyText = String(format: "glass-to-box p50 %.0f ms  p95 %.0f ms  (UI p95 %.0f ms, n=%d)", l.p50, l.p95, u.p95, l.n)
        let h = hitches.snapshot()
        hitchText = String(format: "UI hitches >100 ms: %d  (worst %.0f ms)", h.count, h.worstMs)
    }

    private func scheduleRefresh() {
        guard !refreshPending else { return }
        refreshPending = true
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 200_000_000)
            refreshPending = false
            await refresh()
        }
    }

    func refresh() async {
        guard let engine else { return }
        if let list = try? await engine.catalog(includeHidden: false) { items = list }
        focused = engine.focusedItem()
    }

    // ───────────── review actions ─────────────

    private func act(_ what: String, _ op: @escaping (Engine) async throws -> Void) {
        guard let engine else { return }
        Task { @MainActor in
            do { try await op(engine) } catch { status = "\(what) failed: \(error)" }
            await refresh()
        }
    }

    func confirm(_ id: String) { act("confirm") { _ = try await $0.confirmItem(itemId: id) } }
    func reject(_ id: String, title: String?) { act("reject") { _ = try await $0.rejectItem(itemId: id, correction: title) } }
    func hide(_ id: String) { act("hide") { _ = try await $0.setRole(itemId: id, role: .excluded, similar: true) } }
    func focus(_ id: String) {
        act("focus") { engine in
            if engine.focusedItem() == id { try await engine.clearFocus() } else { try await engine.focusItem(itemId: id) }
        }
    }

    func draftListings() {
        act("draft") { [weak self] engine in
            let listings = try await engine.generateListings(includeUnconfirmed: true)
            await MainActor.run { self?.status = "\(listings.count) draft listing(s) ready" }
        }
    }

    func exportCSV() {
        act("export") { [weak self] engine in
            let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Documents/shelfscan", isDirectory: true)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let path = dir.appendingPathComponent("products.csv")
            var listings = try await engine.listings()
            if listings.isEmpty { listings = try await engine.generateListings(includeUnconfirmed: true) }
            let receipts = try await engine.publish(listingIds: listings.map(\.id), storefront: CsvStorefront(path: path.path, pushInventory: false))
            let ok = receipts.filter(\.ok).count
            await MainActor.run {
                self?.status = "\(ok)/\(receipts.count) draft row(s) written to \(path.path)"
                NSWorkspace.shared.activateFileViewerSelecting([path])
            }
        }
    }

    // ───────────── headless measurement (--measure) ─────────────

    func measurement() -> [String: Any] {
        let none: (p50: Double, p95: Double, max: Double, n: Int) = (0, 0, 0, 0)
        let l = listener?.latency.percentiles() ?? none
        let u = listener?.uiLatency.percentiles() ?? none
        let h = hitches.snapshot()
        return [
            "plugins": ["detector": settings.detector.rawValue, "vision_masks": settings.visionMasks, "embedder": settings.embedder.rawValue, "scanner": settings.scanner.rawValue,
                        "sam_refine": settings.samRefine, "guesser": settings.guesser.rawValue],
            "settings": (try? JSONSerialization.jsonObject(with: JSONEncoder().encode(settings))) ?? [:],
            "frame": ["width": Int(frameSize.width), "height": Int(frameSize.height)],
            "camera": camera?.deviceName ?? "",
            "latency_ms": ["p50": l.p50, "p95": l.p95, "max": l.max, "n": l.n, "rejected_clock_mismatch": listener?.latency.rejected ?? 0],
            "ui_latency_ms": ["p50": u.p50, "p95": u.p95, "max": u.max, "n": u.n],
            "ui_hitches": ["over_100ms": h.count, "worst_ms": h.worstMs],
            "item_titles": items.map { "\($0.id) x\($0.quantity) \($0.guess.title.isEmpty ? $0.label : $0.guess.title)" },
            "stats": ["frames_in": stats.framesIn, "dropped": stats.framesDropped, "skipped": stats.framesSkipped,
                      "processed": stats.framesProcessed, "detections": stats.detections, "items": stats.items],
        ]
    }
}

/// Main-thread stall detector: a 60 Hz timer; any gap over 100 ms between ticks is a hitch the user would see.
final class HitchMeter {
    private var timer: Timer?
    private var last = CACurrentMediaTime()
    private var count = 0
    private var worst = 0.0

    func start() {
        last = CACurrentMediaTime()
        count = 0
        worst = 0
        let t = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.fire() }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop() { timer?.invalidate(); timer = nil }

    private func fire() {
        let now = CACurrentMediaTime(), gap = (now - last) * 1000
        last = now
        if gap > 100 { count += 1 }
        worst = max(worst, gap)
    }

    func snapshot() -> (count: Int, worstMs: Double) { (count, worst) }
}
