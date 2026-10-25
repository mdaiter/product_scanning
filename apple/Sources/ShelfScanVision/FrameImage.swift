// UNCOMPILED SKETCH: written without a Mac. Expect small compile fixes.
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
            let src = stride == 0 ? w * 3 : Int(stride)
            var out = Data(count: w * h * 4)
            data.withUnsafeBytes { (s: UnsafeRawBufferPointer) in
                out.withUnsafeMutableBytes { (d: UnsafeMutableRawBufferPointer) in
                    for y in 0..<h {
                        for x in 0..<w {
                            let si = y * src + x * 3, di = (y * w + x) * 4
                            d[di] = s[si]; d[di + 1] = s[si + 1]; d[di + 2] = s[si + 2]; d[di + 3] = 255
                        }
                    }
                }
            }
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
