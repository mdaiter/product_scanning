// UNCOMPILED SKETCH: written without a Mac or iPhone. Expect compile fixes and orientation work.
// Add NSCameraUsageDescription to the app's Info.plist (iOS and macOS).
import AVFoundation
import Foundation
import ShelfScan

/// Feeds camera frames to the engine. Frames the engine cannot take are skipped BEFORE copying.
public final class CameraSource: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    public let session = AVCaptureSession()
    private let output = AVCaptureVideoDataOutput()
    private let queue = DispatchQueue(label: "shelfscan.camera", qos: .userInitiated)
    private let engine: Engine

    public init(engine: Engine) {
        self.engine = engine
        super.init()
    }

    public func start(position: AVCaptureDevice.Position = .back) throws {
        session.beginConfiguration()
        session.sessionPreset = .hd1280x720
        guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: position)
                ?? AVCaptureDevice.default(for: .video) else {
            session.commitConfiguration()
            throw ShelfError.Io(msg: "no camera available")
        }
        let input = try AVCaptureDeviceInput(device: device)
        if session.canAddInput(input) { session.addInput(input) }
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
        output.alwaysDiscardsLateVideoFrames = true
        output.setSampleBufferDelegate(self, queue: queue)
        if session.canAddOutput(output) { session.addOutput(output) }
        // TODO(phase 2, iPhone): rotate to portrait, e.g. connection.videoRotationAngle = 90 (iOS 17), and keep the
        // overlay's coordinate mapping consistent with the rotated frame.
        session.commitConfiguration()
        queue.async { [session] in session.startRunning() }
    }

    public func stop() { session.stopRunning() }

    public func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        let seconds = CMSampleBufferGetPresentationTimeStamp(sampleBuffer).seconds
        guard seconds.isFinite else { return }
        let ms = UInt64(max(1, seconds * 1000))
        guard engine.wantsFrame(timestampMs: ms) else { return }   // free drop: no copy, camera never blocks
        guard let pb = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(pb) else { return }
        let stride = CVPixelBufferGetBytesPerRow(pb), w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb)
        let data = Data(bytes: base, count: stride * h)           // the one copy per admitted frame
        _ = engine.pushFrame(frame: Frame(width: UInt32(w), height: UInt32(h), format: .bgra8, data: data,
                                          stride: UInt32(stride), timestampMs: ms))
    }
}
