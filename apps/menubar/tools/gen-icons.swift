// Generates the Edgee app icon (.iconset PNGs → .icns) and a template menubar
// PDF from the official Edgee mark (the three-bar "flag" glyph). The mark's SVG
// paths are lifted from the CLI's browser-auth page (crates/cli/src/commands/
// auth/login.rs) so the app art matches Edgee's brand.
//
// Usage: swift gen-icons.swift <output-dir>
//   → <output-dir>/Edgee.iconset/*.png  and  <output-dir>/MenuBarIcon.pdf
// (run `iconutil -c icns Edgee.iconset` afterwards to produce Edgee.icns)

import AppKit
import CoreGraphics
import Foundation

// The three sub-paths of the Edgee mark, in its native 24.6 × 28 coordinate box.
let markPaths = [
    "M11.9146 0.628193C12.0839 0.246001 12.4596 0 12.8739 0H23.4087C24.1705 0 24.6793 0.793671 24.3679 1.49647L23.0569 4.45582C22.8876 4.83801 22.5119 5.08401 22.0976 5.08401H11.5629C10.801 5.08401 10.2922 4.29034 10.6036 3.58754L11.9146 0.628193Z",
    "M1.404 12.1621C1.57331 11.7799 1.94897 11.5339 2.36328 11.5339H15.0752C15.8371 11.5339 16.3459 12.3275 16.0345 13.0303L14.7235 15.9897C14.5542 16.3719 14.1785 16.6179 13.7642 16.6179H1.05226C0.290392 16.6179 -0.218379 15.8242 0.0929685 15.1214L1.404 12.1621Z",
    "M1.404 23.5442C1.57331 23.162 1.94897 22.916 2.36328 22.916H19.8801C20.6419 22.916 21.1507 23.7097 20.8394 24.4125L19.5283 27.3718C19.359 27.754 18.9834 28 18.5691 28H1.05226C0.290392 28 -0.218379 27.2063 0.0929685 26.5035L1.404 23.5442Z",
]

// MARK: - Minimal SVG path parser (absolute M, C, L, H, V, Z — all these paths use).

func tokenize(_ d: String) -> [String] {
    var tokens: [String] = []
    var num = ""
    func flush() {
        if !num.isEmpty { tokens.append(num); num = "" }
    }
    for ch in d {
        if ch.isLetter {
            flush()
            tokens.append(String(ch))
        } else if ch == " " || ch == "," {
            flush()
        } else if ch == "-" || ch == "+" {
            // A sign starts a new number unless it's an exponent sign.
            if !num.isEmpty && !num.hasSuffix("e") && !num.hasSuffix("E") { flush() }
            num.append(ch)
        } else if ch == "." {
            if num.contains(".") { flush() }
            num.append(ch)
        } else {
            num.append(ch)
        }
    }
    flush()
    return tokens
}

func buildPath(_ d: String) -> CGPath {
    let path = CGMutablePath()
    let tokens = tokenize(d)
    var i = 0
    var cur = CGPoint.zero
    func nextNum() -> CGFloat { defer { i += 1 }; return CGFloat(Double(tokens[i]) ?? 0) }
    while i < tokens.count {
        let cmd = tokens[i]; i += 1
        switch cmd {
        case "M":
            cur = CGPoint(x: nextNum(), y: nextNum()); path.move(to: cur)
        case "L":
            cur = CGPoint(x: nextNum(), y: nextNum()); path.addLine(to: cur)
        case "H":
            cur = CGPoint(x: nextNum(), y: cur.y); path.addLine(to: cur)
        case "V":
            cur = CGPoint(x: cur.x, y: nextNum()); path.addLine(to: cur)
        case "C":
            let c1 = CGPoint(x: nextNum(), y: nextNum())
            let c2 = CGPoint(x: nextNum(), y: nextNum())
            cur = CGPoint(x: nextNum(), y: nextNum())
            path.addCurve(to: cur, control1: c1, control2: c2)
        case "Z", "z":
            path.closeSubpath()
        default:
            break  // numbers already consumed by their command
        }
    }
    return path
}

func markPath() -> CGPath {
    let combined = CGMutablePath()
    for d in markPaths { combined.addPath(buildPath(d)) }
    return combined
}

// Transform mapping the mark's SVG box into `dest` (canvas, y-up), flipping Y so
// the glyph renders upright, preserving aspect and centering.
func markTransform(into dest: CGRect) -> CGAffineTransform {
    let box = markPath().boundingBoxOfPath
    let scale = min(dest.width / box.width, dest.height / box.height)
    let drawnW = box.width * scale, drawnH = box.height * scale
    let ox = dest.minX + (dest.width - drawnW) / 2
    let oy = dest.minY + (dest.height - drawnH) / 2
    return CGAffineTransform(a: scale, b: 0, c: 0, d: -scale,
                             tx: ox - scale * box.minX,
                             ty: oy + scale * box.maxY)
}

func context(_ size: Int) -> CGContext {
    let cs = CGColorSpace(name: CGColorSpace.sRGB)!
    return CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
                     bytesPerRow: 0, space: cs,
                     bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
}

// MARK: - App icon (white mark on the Edgee purple-gradient squircle).

func renderAppIcon(_ size: Int) -> CGImage {
    let ctx = context(size)
    let s = CGFloat(size)
    let inset = s * 0.09
    let rect = CGRect(x: inset, y: inset, width: s - 2 * inset, height: s - 2 * inset)
    let radius = rect.width * 0.2237  // ~ macOS squircle-ish

    let cs = CGColorSpace(name: CGColorSpace.sRGB)!
    let top = CGColor(colorSpace: cs, components: [148 / 255, 0 / 255, 211 / 255, 1])!   // #9400D3
    let bottom = CGColor(colorSpace: cs, components: [61 / 255, 46 / 255, 179 / 255, 1])! // #3D2EB3
    let grad = CGGradient(colorsSpace: cs, colors: [top, bottom] as CFArray, locations: [0, 1])!

    ctx.saveGState()
    ctx.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
    ctx.clip()
    ctx.drawLinearGradient(grad, start: CGPoint(x: s / 2, y: s), end: CGPoint(x: s / 2, y: 0), options: [])
    ctx.restoreGState()

    // Mark fills ~52% of the canvas, centered.
    let markRegion = CGRect(x: s * 0.24, y: s * 0.24, width: s * 0.52, height: s * 0.52)
    let t = markTransform(into: markRegion)
    ctx.addPath(markPath().copy(using: [t])!)
    ctx.setFillColor(CGColor(gray: 1, alpha: 1))
    ctx.fillPath()

    return ctx.makeImage()!
}

func writePNG(_ image: CGImage, to url: URL) {
    let rep = NSBitmapImageRep(cgImage: image)
    let data = rep.representation(using: .png, properties: [:])!
    try! data.write(to: url)
}

// MARK: - Menubar template PDF (black mark, transparent; macOS tints it).

func renderMenuBarPDF(to url: URL) {
    let height: CGFloat = 16
    let box = markPath().boundingBoxOfPath
    let width = height * (box.width / box.height)
    var media = CGRect(x: 0, y: 0, width: width, height: height)
    let ctx = CGContext(url as CFURL, mediaBox: &media, nil)!
    ctx.beginPDFPage(nil)
    // Small vertical padding so the glyph doesn't touch the menubar edges.
    let region = media.insetBy(dx: 0, dy: height * 0.06)
    let t = markTransform(into: region)
    ctx.addPath(markPath().copy(using: [t])!)
    ctx.setFillColor(CGColor(gray: 0, alpha: 1))
    ctx.fillPath()
    ctx.endPDFPage()
    ctx.closePDF()
}

// MARK: - Main

let outDir = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".")
let iconset = outDir.appendingPathComponent("Edgee.iconset")
try? FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

// Standard iconset entries: (filename, pixel size).
let entries: [(String, Int)] = [
    ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024),
]
for (name, size) in entries {
    writePNG(renderAppIcon(size), to: iconset.appendingPathComponent(name))
}

renderMenuBarPDF(to: outDir.appendingPathComponent("MenuBarIcon.pdf"))

print("wrote \(iconset.path) and MenuBarIcon.pdf")
