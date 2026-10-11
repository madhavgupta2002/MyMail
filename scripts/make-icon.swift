// Renders the MyMail app icon and writes assets/AppIcon.icns.
// Usage: swift scripts/make-icon.swift
import AppKit

let root = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().deletingLastPathComponent()
let iconset = FileManager.default.temporaryDirectory.appendingPathComponent("AppIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

func render(_ px: Int) -> Data {
    let s = CGFloat(px)
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    // Rounded-square background with a blue gradient (macOS icon grid: ~80% of canvas).
    let inset = s * 0.1
    let bg = NSBezierPath(roundedRect: NSRect(x: inset, y: inset, width: s - 2 * inset, height: s - 2 * inset),
                          xRadius: s * 0.18, yRadius: s * 0.18)
    NSGradient(starting: NSColor(calibratedRed: 0.29, green: 0.56, blue: 1.0, alpha: 1),
               ending: NSColor(calibratedRed: 0.13, green: 0.30, blue: 0.86, alpha: 1))!.draw(in: bg, angle: -90)

    // Envelope.
    let w = s * 0.52, h = s * 0.36
    let env = NSRect(x: (s - w) / 2, y: (s - h) / 2 - s * 0.01, width: w, height: h)
    NSColor.white.setFill()
    NSBezierPath(roundedRect: env, xRadius: s * 0.035, yRadius: s * 0.035).fill()
    let flap = NSBezierPath()
    flap.move(to: NSPoint(x: env.minX + s * 0.02, y: env.maxY - s * 0.015))
    flap.line(to: NSPoint(x: env.midX, y: env.midY - s * 0.02))
    flap.line(to: NSPoint(x: env.maxX - s * 0.02, y: env.maxY - s * 0.015))
    flap.lineWidth = s * 0.028
    flap.lineCapStyle = .round
    flap.lineJoinStyle = .round
    NSColor(calibratedRed: 0.18, green: 0.40, blue: 0.93, alpha: 1).setStroke()
    flap.stroke()

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

for base in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let name = scale == 1 ? "icon_\(base)x\(base).png" : "icon_\(base)x\(base)@2x.png"
        try! render(base * scale).write(to: iconset.appendingPathComponent(name))
    }
}

let out = root.appendingPathComponent("assets/AppIcon.icns")
let p = Process()
p.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
p.arguments = ["-c", "icns", iconset.path, "-o", out.path]
try! p.run()
p.waitUntilExit()
print(p.terminationStatus == 0 ? "wrote \(out.path)" : "iconutil failed")
