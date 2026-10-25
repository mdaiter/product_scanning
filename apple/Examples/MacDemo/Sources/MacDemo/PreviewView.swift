import AppKit
import AVFoundation
import ShelfScan
import SwiftUI

/// Camera preview (aspect-fit) with the engine's boxes drawn on top. Box geometry is computed from the size of the frames
/// the engine received, so it does not depend on how AVFoundation maps metadata rects; mirroring is switched off so the
/// preview and the analysed frames agree.
struct PreviewView: NSViewRepresentable {
    let session: AVCaptureSession?
    let frameSize: CGSize
    let boxes: [OverlayBox]
    var showBoxes = true
    var showLabels = true
    /// Barcode mode: dim everything but the codes, outline each code; with none in view, a centred aiming square.
    var lensActive = false
    var lensQuads: [[CGPoint]] = []

    func makeNSView(context: Context) -> PreviewNSView { PreviewNSView() }

    func updateNSView(_ view: PreviewNSView, context: Context) { apply(to: view) }

    /// What SwiftUI's update does; separate so the self-test can drive it without a SwiftUI context.
    func apply(to view: PreviewNSView) {
        view.setSession(session)
        view.frameSize = frameSize
        view.showLabels = showLabels
        view.lensActive = lensActive
        view.lensQuads = lensQuads
        view.boxes = showBoxes && !lensActive ? boxes : []
    }
}

final class PreviewNSView: NSView {
    private let preview = AVCaptureVideoPreviewLayer()
    private let overlay = CALayer()
    var frameSize = CGSize(width: 16, height: 9) { didSet { if frameSize != oldValue { needsLayout = true } } }
    var boxes: [OverlayBox] = [] { didSet { drawBoxes() } }
    var showLabels = true
    var overlayLayerCount: Int { overlay.sublayers?.count ?? 0 }
    private let lensLayer = CALayer()
    var lensActive = false { didSet { if lensActive != oldValue { drawLens() } } }
    var lensQuads: [[CGPoint]] = [] { didSet { drawLens() } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        let root = CALayer()
        root.backgroundColor = NSColor.black.cgColor
        root.isGeometryFlipped = true   // top-left origin, like the engine's normalised boxes
        layer = root
        preview.videoGravity = .resizeAspect
        root.addSublayer(preview)
        // isGeometryFlipped is not inherited: without this the boxes inside `overlay` use AppKit's bottom-left origin and
        // come out mirrored vertically (seen on the first live run: the box covered the top of the frame, the person the bottom).
        overlay.isGeometryFlipped = true
        root.addSublayer(overlay)
        lensLayer.isGeometryFlipped = true
        root.addSublayer(lensLayer)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func setSession(_ s: AVCaptureSession?) {
        guard preview.session !== s else { return }
        preview.session = s
        if let c = preview.connection, c.isVideoMirroringSupported {
            c.automaticallyAdjustsVideoMirroring = false
            c.isVideoMirrored = false
        }
    }

    /// The rectangle the aspect-fit preview occupies inside the view.
    var videoRect: CGRect { AVMakeRect(aspectRatio: frameSize, insideRect: bounds) }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        preview.frame = bounds
        overlay.frame = videoRect
        lensLayer.frame = videoRect
        CATransaction.commit()
        drawBoxes()
        drawLens()
    }

    private func drawBoxes() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        overlay.sublayers?.forEach { $0.removeFromSuperlayer() }
        let size = overlay.bounds.size
        for b in boxes {
            let r = CGRect(x: CGFloat(b.bbox.x) * size.width, y: CGFloat(b.bbox.y) * size.height,
                           width: CGFloat(b.bbox.w) * size.width, height: CGFloat(b.bbox.h) * size.height)
            let shape = CAShapeLayer()
            shape.path = CGPath(rect: r, transform: nil)
            shape.fillColor = nil
            shape.lineWidth = b.focused ? 3 : 2
            shape.strokeColor = color(b).cgColor
            overlay.addSublayer(shape)
            guard showLabels, !b.label.isEmpty else { continue }
            let text = CATextLayer()
            text.string = b.label
            text.fontSize = 11
            text.foregroundColor = NSColor.white.cgColor
            text.backgroundColor = NSColor.black.withAlphaComponent(0.6).cgColor
            text.contentsScale = window?.backingScaleFactor ?? 2
            text.frame = CGRect(x: r.minX, y: max(0, r.minY - 16), width: max(r.width, 160), height: 15)
            overlay.addSublayer(text)
        }
        CATransaction.commit()
    }

    private func drawLens() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        lensLayer.sublayers?.forEach { $0.removeFromSuperlayer() }
        defer { CATransaction.commit() }
        guard lensActive else { return }
        let size = lensLayer.bounds.size
        let quads = lensQuads.map { q in q.map { CGPoint(x: $0.x * size.width, y: $0.y * size.height) } }
        // dim everything outside the codes (even-odd: the codes are holes in the veil)
        let veil = CGMutablePath()
        veil.addRect(CGRect(origin: .zero, size: size))
        if quads.isEmpty {
            let side = min(size.width, size.height) * 0.45
            veil.addRect(CGRect(x: (size.width - side) / 2, y: (size.height - side) / 2, width: side, height: side))
        }
        for q in quads where q.count == 4 { veil.addLines(between: q); veil.closeSubpath() }
        let dim = CAShapeLayer()
        dim.path = veil
        dim.fillRule = .evenOdd
        dim.fillColor = NSColor.black.withAlphaComponent(quads.isEmpty ? 0.35 : 0.55).cgColor
        lensLayer.addSublayer(dim)
        if quads.isEmpty {
            let side = min(size.width, size.height) * 0.45
            let aim = CAShapeLayer()
            aim.path = CGPath(rect: CGRect(x: (size.width - side) / 2, y: (size.height - side) / 2, width: side, height: side), transform: nil)
            aim.fillColor = nil
            aim.strokeColor = NSColor.white.withAlphaComponent(0.8).cgColor
            aim.lineDashPattern = [8, 6]
            aim.lineWidth = 2
            lensLayer.addSublayer(aim)
            return
        }
        for q in quads where q.count == 4 {
            let outline = CGMutablePath()
            outline.addLines(between: q)
            outline.closeSubpath()
            let shape = CAShapeLayer()
            shape.path = outline
            shape.fillColor = nil
            shape.strokeColor = NSColor.systemGreen.cgColor
            shape.lineWidth = 3
            shape.lineJoin = .round
            lensLayer.addSublayer(shape)
        }
    }

    private func color(_ b: OverlayBox) -> NSColor {
        if b.focused { return .systemYellow }
        if b.role != .product { return .systemGray }
        switch b.state {
        case .confirmed?: return .systemGreen
        case .rejected?: return .systemRed
        case .guessed?: return b.needsBarcode ? .systemOrange : .systemBlue
        default: return .white
        }
    }
}
