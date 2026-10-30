// Draws the icon of the macOS app into apple/App/Assets.xcassets/AppIcon.appiconset.
// Usage: swift scripts/make-icon.swift (from the root of the repository).
import AppKit

let sizes = [16, 32, 64, 128, 256, 512, 1024]
let set = URL(fileURLWithPath: "apple/App/Assets.xcassets/AppIcon.appiconset", isDirectory: true)
try FileManager.default.createDirectory(at: set, withIntermediateDirectories: true)

func draw(_ side: Int) -> Data {
    let image = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8, samplesPerPixel: 4,
                                 hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: image)
    let s = CGFloat(side)
    let tile = NSRect(x: s * 0.1, y: s * 0.1, width: s * 0.8, height: s * 0.8)
    NSGradient(starting: NSColor(calibratedRed: 0.98, green: 0.78, blue: 0.30, alpha: 1), ending: NSColor(calibratedRed: 0.93, green: 0.55, blue: 0.20, alpha: 1))?
        .draw(in: NSBezierPath(roundedRect: tile, xRadius: s * 0.18, yRadius: s * 0.18), angle: -90)
    let page = NSRect(x: s * 0.27, y: s * 0.2, width: s * 0.46, height: s * 0.6)
    NSColor.white.setFill()
    NSBezierPath(roundedRect: page, xRadius: s * 0.04, yRadius: s * 0.04).fill()
    NSColor(calibratedWhite: 0.55, alpha: 1).setFill()
    for (index, width) in [0.32, 0.32, 0.24].enumerated() {
        let y = s * (0.64 - CGFloat(index) * 0.11)
        NSBezierPath(roundedRect: NSRect(x: s * 0.34, y: y, width: s * width, height: s * 0.035), xRadius: s * 0.012, yRadius: s * 0.012).fill()
    }
    NSGraphicsContext.current = nil
    return image.representation(using: .png, properties: [:])!
}

var images: [[String: String]] = []
for side in sizes {
    let name = "icon_\(side).png"
    try draw(side).write(to: set.appendingPathComponent(name))
    if side <= 512 {
        images.append(["idiom": "mac", "scale": "1x", "size": "\(side)x\(side)", "filename": name])
    }
    if side >= 32 {
        images.append(["idiom": "mac", "scale": "2x", "size": "\(side / 2)x\(side / 2)", "filename": name])
    }
}
let contents: [String: Any] = ["images": images, "info": ["author": "xcode", "version": 1]]
try JSONSerialization.data(withJSONObject: contents, options: [.prettyPrinted, .sortedKeys]).write(to: set.appendingPathComponent("Contents.json"))
