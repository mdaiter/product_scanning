import AppKit
import ShelfScan
import SwiftUI

/// Command-line options. `--measure N` starts scanning at launch, runs N seconds, writes the measurement JSON and quits.
struct Options {
    var measureSeconds: Double?
    var out = "macdemo-measure.json"
    var detector: DetectorChoice?
    var embedder: EmbedderChoice?
    var scanner: ScannerChoice?
    var masks: Bool?
    var defaults = false
    var overlayTest = false

    static func parse(_ args: [String]) -> Options {
        var o = Options()
        var it = args.dropFirst().makeIterator()
        while let a = it.next() {
            switch a {
            case "--measure": o.measureSeconds = it.next().flatMap(Double.init)
            case "--out": o.out = it.next() ?? o.out
            case "--detector":
                switch it.next() { case "vision": o.detector = .vision; case "detr": o.detector = .detr; default: o.detector = .blob }
            case "--embedder": o.embedder = it.next() == "featureprint" ? .featurePrint : .color
            case "--scanner": o.scanner = it.next() == "vision" ? .vision : .rxing
            case "--no-masks": o.masks = false
            case "--defaults": o.defaults = true   // ignore saved settings
            case "--overlay-test": o.overlayTest = true   // no camera: draw one known box to check overlay geometry
            default: break
            }
        }
        return o
    }
}

/// Launch work lives here, not in a view's `.task`: a window is not guaranteed to exist at launch
/// (measure mode once hung with no window and so never started).
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let controller = DemoController()
    let options = Options.parse(CommandLine.arguments)

    func applicationDidFinishLaunching(_ notification: Notification) {
        // An unbundled SwiftPM executable starts as a background process; make it a normal app with a window and menu.
        NSApplication.shared.setActivationPolicy(.regular)
        NSApplication.shared.activate(ignoringOtherApps: true)
        Task { @MainActor in await launch() }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { options.measureSeconds == nil }

    @MainActor
    private func launch() async {
        var s = options.defaults ? DemoSettings() : DemoSettings.load()
        if let d = options.detector { s.detector = d }
        if let e = options.embedder { s.embedder = e }
        if let b = options.scanner { s.scanner = b }
        if let m = options.masks { s.visionMasks = m }
        controller.setInitial(s)
        if options.overlayTest {
            controller.frameSize = CGSize(width: 1920, height: 1080)
            controller.model.boxes = [OverlayBox(trackId: 1, bbox: BBox(x: 0.05, y: 0.05, w: 0.30, h: 0.20), itemId: nil, state: nil,
                                                 role: .product, label: "top-left 5%,5% 30x20%", needsBarcode: false, focused: true)]
            if CommandLine.arguments.contains("--selftest") { overlaySelfTest(); NSApplication.shared.terminate(nil) }
            return
        }
        guard let seconds = options.measureSeconds else { return }
        await controller.start()
        try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
        await controller.stop()
        var m = controller.measurement()
        m["seconds"] = seconds
        m["date"] = ISO8601DateFormatter().string(from: Date())
        if let data = try? JSONSerialization.data(withJSONObject: m, options: [.prettyPrinted, .sortedKeys]) {
            FileManager.default.createFile(atPath: options.out, contents: data)
            print(String(decoding: data, as: UTF8.self))
        }
        NSApplication.shared.terminate(nil)
    }
}

@main
struct MacDemoApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup("ShelfScan Mac Demo") {
            ContentView(controller: delegate.controller, model: delegate.controller.model)
                .frame(minWidth: 1000, minHeight: 620)
        }
    }
}

/// Renders a PreviewNSView with one focused (yellow) box to a bitmap and prints where the box landed, in view pixels
/// with a top-left origin. Expected for bbox (5%, 5%, 30%, 20%) in a 1600x1000 view showing a 16:9 frame:
/// video rect y 50...950 (letterboxed), so the box should start near x=80, y=95.
@MainActor
func overlaySelfTest() {
    let v = PreviewNSView(frame: NSRect(x: 0, y: 0, width: 1600, height: 1000))
    v.frameSize = CGSize(width: 1920, height: 1080)
    v.layout()
    v.boxes = [OverlayBox(trackId: 1, bbox: BBox(x: 0.05, y: 0.05, w: 0.30, h: 0.20), itemId: nil, state: nil,
                          role: .product, label: "", needsBarcode: false, focused: true)]
    guard let rep = v.bitmapImageRepForCachingDisplay(in: v.bounds) else { print("SELFTEST no bitmap"); return }
    v.cacheDisplay(in: v.bounds, to: rep)
    var x0 = Int.max, y0 = Int.max, x1 = -1, y1 = -1
    for y in 0..<rep.pixelsHigh {
        for x in 0..<rep.pixelsWide {
            guard let c = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
            if c.redComponent > 0.8 && c.greenComponent > 0.6 && c.blueComponent < 0.3 {
                x0 = min(x0, x); y0 = min(y0, y); x1 = max(x1, x); y1 = max(y1, y)
            }
        }
    }
    let scale = Double(rep.pixelsWide) / 1600
    print(String(format: "SELFTEST yellow box (view points, top-left origin): x %.0f...%.0f  y %.0f...%.0f   expected x 80...560  y 95...275",
                 Double(x0) / scale, Double(x1) / scale, Double(y0) / scale, Double(y1) / scale))
    let ok = abs(Double(x0) / scale - 80) < 8 && abs(Double(y0) / scale - 95) < 8 && abs(Double(y1) / scale - 275) < 8
    print(ok ? "SELFTEST OK" : "SELFTEST FAILED: overlay boxes are misplaced")
    if !ok { exit(1) }
}
