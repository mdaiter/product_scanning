// Compiles on macOS 26 / Xcode 26; unit-tested on synthetic fixtures only (docs/results/phase1-vision-unit.md).
import Accelerate
import CoreGraphics
import Foundation
import ShelfScan

extension Frame {
    /// CGImage for Vision. Handles `.bgra8` (the camera format), `.rgba8` and `.rgb8`; planar formats return nil.
    public func cgImage() -> CGImage? {
        let w = Int(width), h = Int(height)
        let rowBytes: Int
        let info: CGBitmapInfo
        var pixels = data
        switch format {
        case .bgra8:
            rowBytes = stride == 0 ? w * 4 : Int(stride)
            info = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        case .rgba8:
            rowBytes = stride == 0 ? w * 4 : Int(stride)
            info = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue)
        case .rgb8:
            // vImage (vectorised): a per-pixel Swift loop took hundreds of ms per 1280x720 frame in debug builds,
            // which starved the barcode path (the engine hands plugins RGB frames).
            let src = stride == 0 ? w * 3 : Int(stride)
            var out = Data(count: w * h * 4)
            let ok: Bool = data.withUnsafeBytes { (s: UnsafeRawBufferPointer) in
                out.withUnsafeMutableBytes { (d: UnsafeMutableRawBufferPointer) in
                    var sb = vImage_Buffer(data: UnsafeMutableRawPointer(mutating: s.baseAddress!), height: vImagePixelCount(h),
                                           width: vImagePixelCount(w), rowBytes: src)
                    var db = vImage_Buffer(data: d.baseAddress!, height: vImagePixelCount(h), width: vImagePixelCount(w), rowBytes: w * 4)
                    return vImageConvert_RGB888toRGBA8888(&sb, nil, 255, &db, false, vImage_Flags(kvImageNoFlags)) == kvImageNoError
                }
            }
            guard ok else { return nil }
            pixels = out
            rowBytes = w * 4
            info = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue)
        default:
            return nil
        }
        guard let provider = CGDataProvider(data: pixels as CFData) else { return nil }
        return CGImage(
            width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: rowBytes,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: info, provider: provider,
            decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }
}
