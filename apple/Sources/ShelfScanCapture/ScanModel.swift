// UNCOMPILED SKETCH: written without a Mac. Expect compile fixes (Swift 6 concurrency checking in particular:
// UniFFI's Swift 6 support is partial and async calls are not Sendable yet, see uniffi-rs issue 2448).
import Foundation
import ShelfScan
import SwiftUI

/// UI state fed by engine events. The engine calls the listener on its own thread, so every update hops to the main actor.
@MainActor
public final class ScanModel: ObservableObject {
    @Published public var boxes: [OverlayBox] = []
    @Published public var hiddenCount: UInt32 = 0
    @Published public var summary = ScanSummary(framesIn: 0, framesDropped: 0, framesSkipped: 0, framesProcessed: 0, detections: 0, tracksPromoted: 0, items: 0)
    @Published public var log: [String] = []
    @Published public var barcodeHint: (itemId: String, hint: String)?

    public init() {}

    func apply(_ e: Event) {
        switch e {
        case .overlay(_, _, let b, let hidden): boxes = b; hiddenCount = hidden
        case .stats(let s, _): summary = s
        case .barcodeRequested(let id, let hint): barcodeHint = (id, hint)
        case .barcodeRead(let id, let gtin): log.append("barcode \(gtin) for \(id)"); if barcodeHint?.itemId == id { barcodeHint = nil }
        case .guessReady(let id, let title, _): log.append("\(id): \(title)")
        case .itemDiscovered(let id, _): log.append("found \(id)")
        case .warning(let src, let msg), .error(let src, let msg): log.append("\(src): \(msg)")
        default: break
        }
    }
}

public final class ModelListener: EventListener {
    private let model: ScanModel
    public init(model: ScanModel) { self.model = model }
    public func onEvent(event: Event) {
        Task { @MainActor [model] in model.apply(event) }
    }
}

/// Draws normalised boxes over an aspect-FIT preview (aspect-fill would need the crop mapped back, a Phase 2 task).
public struct OverlayView: View {
    let boxes: [OverlayBox]
    public init(boxes: [OverlayBox]) { self.boxes = boxes }

    public var body: some View {
        GeometryReader { g in
            ForEach(boxes, id: \.trackId) { b in
                let r = CGRect(x: CGFloat(b.bbox.x) * g.size.width, y: CGFloat(b.bbox.y) * g.size.height,
                               width: CGFloat(b.bbox.w) * g.size.width, height: CGFloat(b.bbox.h) * g.size.height)
                Rectangle().stroke(b.focused ? Color.yellow : (b.needsBarcode ? Color.orange : Color.green), lineWidth: 2)
                    .frame(width: r.width, height: r.height)
                    .overlay(alignment: .topLeading) {
                        Text(b.label).font(.caption2).padding(2).background(.black.opacity(0.6)).foregroundColor(.white).offset(y: -18)
                    }
                    .position(x: r.midX, y: r.midY)
            }
        }
        .allowsHitTesting(false)
    }
}
