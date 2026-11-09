// Synthetic HEIC gain-map fixtures, written by macOS's ImageIO as an iPhone's
// photos are.
//
// Usage: swift scripts/make-heic-gain-map-fixtures.swift tests/data/formats
// Needs macOS 15 or later. Every image is drawn here; nothing comes from a camera
// or a photo.
//
// The base of each is 64x48 8-bit Display P3: red ramps left to right, green top to
// bottom, blue is half. Apple's maps are 32x24 grey, Rec. 709-encoded, as Apple's
// documentation describes them: a ramp left to right plus a little top to bottom, so
// a turned or mirrored map is caught.
// - apple-gain-map.heic: Apple's auxiliary map with its HDRGainMapVersion XMP, and
//   maker-note tags 33 = 1.01 and 48 = 0.0089 (2.377 stops of headroom).
// - apple-gain-map-turned.heic: the same, its EXIF orientation 6 (written as `irot`).
// - apple-gain-map-no-headroom.heic: no maker note.
// - apple-gain-map-unversioned.heic: no HDRGainMapVersion XMP.
// - iso-gain-map.heic: ISO 21496-1 (`tmap`), as ImageIO encodes an HDR ramp to 4x
//   SDR white (`kCGImageDestinationEncodeToISOGainmap`).
import CoreGraphics
import CoreVideo
import Foundation
import ImageIO

let out = URL(fileURLWithPath: CommandLine.arguments[1])
let (w, h) = (64, 48)
let p3 = CGColorSpace(name: CGColorSpace.displayP3)!

func base() -> CGImage {
    var px = [UInt8](repeating: 0, count: w * h * 4)
    for y in 0..<h {
        for x in 0..<w {
            let o = (y * w + x) * 4
            px[o] = UInt8(x * 255 / (w - 1))
            px[o + 1] = UInt8(y * 255 / (h - 1))
            px[o + 2] = 128
            px[o + 3] = 255
        }
    }
    return CGImage(
        width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
        space: p3, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
        provider: CGDataProvider(data: Data(px) as CFData)!, decode: nil,
        shouldInterpolate: false, intent: .defaultIntent)!
}

func apple(_ name: String, orientation: Int, maker: Bool, version: Bool) {
    let (mw, mh) = (w / 2, h / 2)
    var map = [UInt8](repeating: 0, count: mw * mh)
    for y in 0..<mh {
        for x in 0..<mw { map[y * mw + x] = UInt8(min(255, x * 220 / (mw - 1) + y)) }
    }
    let dest = CGImageDestinationCreateWithURL(
        out.appendingPathComponent(name) as CFURL, "public.heic" as CFString, 1, nil)!
    var props: [CFString: Any] = [
        kCGImageDestinationLossyCompressionQuality: 1.0,
        kCGImagePropertyOrientation: orientation,
    ]
    if maker {
        props[kCGImagePropertyMakerAppleDictionary] = ["33": 1.01, "48": 0.0089]
    }
    CGImageDestinationAddImage(dest, base(), props as CFDictionary)
    let meta = CGImageMetadataCreateMutable()
    if version {
        CGImageMetadataRegisterNamespaceForPrefix(
            meta, "http://ns.apple.com/HDRGainMap/1.0/" as CFString, "HDRGainMap" as CFString, nil)
        CGImageMetadataSetValueWithPath(
            meta, nil, "HDRGainMap:HDRGainMapVersion" as CFString, 65536 as CFNumber)
    }
    let description: [CFString: Any] = [
        kCGImagePropertyWidth: mw, kCGImagePropertyHeight: mh, kCGImagePropertyBytesPerRow: mw,
        kCGImagePropertyPixelFormat: kCVPixelFormatType_OneComponent8,
    ]
    let info: [CFString: Any] = [
        kCGImageAuxiliaryDataInfoData: Data(map) as CFData,
        kCGImageAuxiliaryDataInfoDataDescription: description,
        kCGImageAuxiliaryDataInfoMetadata: meta,
    ]
    CGImageDestinationAddAuxiliaryDataInfo(dest, kCGImageAuxiliaryDataTypeHDRGainMap, info as CFDictionary)
    precondition(CGImageDestinationFinalize(dest))
}

func iso(_ name: String) {
    var px = [Float](repeating: 0, count: w * h * 4)
    for y in 0..<h {
        for x in 0..<w {
            let o = (y * w + x) * 4
            let t = Float(x) / Float(w - 1)
            let l = 0.02 + 3.98 * t * t * t
            px[o] = l
            px[o + 1] = l * (0.6 + 0.4 * Float(y) / Float(h - 1))
            px[o + 2] = l * 0.5
            px[o + 3] = 1
        }
    }
    let image = CGImage(
        width: w, height: h, bitsPerComponent: 32, bitsPerPixel: 128, bytesPerRow: w * 16,
        space: CGColorSpace(name: CGColorSpace.extendedLinearDisplayP3)!,
        bitmapInfo: CGBitmapInfo(
            rawValue: CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.floatComponents.rawValue
                | CGBitmapInfo.byteOrder32Little.rawValue),
        provider: CGDataProvider(data: Data(bytes: px, count: px.count * 4) as CFData)!,
        decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
    let dest = CGImageDestinationCreateWithURL(
        out.appendingPathComponent(name) as CFURL, "public.heic" as CFString, 1, nil)!
    let props: [CFString: Any] = [
        kCGImageDestinationLossyCompressionQuality: 1.0,
        kCGImageDestinationEncodeRequest: kCGImageDestinationEncodeToISOGainmap,
    ]
    CGImageDestinationAddImage(dest, CGImageCreateCopyWithContentHeadroom(4, image)!, props as CFDictionary)
    precondition(CGImageDestinationFinalize(dest))
}

apple("apple-gain-map.heic", orientation: 1, maker: true, version: true)
apple("apple-gain-map-turned.heic", orientation: 6, maker: true, version: true)
apple("apple-gain-map-no-headroom.heic", orientation: 1, maker: false, version: true)
apple("apple-gain-map-unversioned.heic", orientation: 1, maker: true, version: false)
iso("iso-gain-map.heic")
print(ProcessInfo.processInfo.operatingSystemVersionString)
