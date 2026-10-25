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

    func makeNSView(context: Context) -> PreviewNSView { PreviewNSView() }

    func updateNSView(_ view: PreviewNSView, context: Context) { apply(to: view) }

    /// What SwiftUI's update does; separate so the self-test can drive it without a SwiftUI context.
    func apply(to view: PreviewNSView) {
        view.setSession(session)
        view.frameSize = frameSize
        view.showLabels = showLabels
        view.boxes = showBoxes ? boxes : []
    }
}

final class PreviewNSView: NSView {
    private let preview = AVCaptureVideoPreviewLayer()
    private let overlay = CALayer()
    var frameSize = CGSize(width: 16, height: 9) { didSet { if frameSize != oldValue { needsLayout = true } } }
    var boxes: [OverlayBox] = [] { didSet { drawBoxes() } }
    var showLabels = true
    var overlayLayerCount: Int { overlay.sublayers?.count ?? 0 }

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
        CATransaction.commit()
        drawBoxes()
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
