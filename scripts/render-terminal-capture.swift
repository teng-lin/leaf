#!/usr/bin/env swift
// Rasterize a real `tmux capture-pane -p -e` capture for visual review on macOS.
// Usage: swift scripts/render-terminal-capture.swift capture.ansi output.png [columns]
// This does not lay out diagrams: it paints the captured terminal cells and SGR colors.
import AppKit
import Foundation

guard CommandLine.arguments.count >= 3 else {
    fatalError("Usage: render-terminal-capture.swift input.ansi output.png [columns]")
}
let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
var rows = source.components(separatedBy: "\n")
if rows.last == "" { rows.removeLast() }
let sgr = try NSRegularExpression(pattern: "\u{1b}\\[[0-9;]*m")
let plain = sgr.stringByReplacingMatches(in: source, range: NSRange(source.startIndex..., in: source), withTemplate: "")
let columns = CommandLine.arguments.count > 3 ? Int(CommandLine.arguments[3])! : plain.split(separator: "\n", omittingEmptySubsequences: false).map(\.count).max()!
let font = NSFont(name: "Menlo-Regular", size: 14)!
let boldFont = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
let cellWidth = ceil(("M" as NSString).size(withAttributes: [.font: font]).width)
let cellHeight: CGFloat = 18
let padding: CGFloat = 12
let width = Int(CGFloat(columns) * cellWidth + padding * 2)
let height = Int(CGFloat(rows.count) * cellHeight + padding * 2)
let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)

func rgb(_ r: Int, _ g: Int, _ b: Int) -> NSColor {
    NSColor(srgbRed: CGFloat(r)/255, green: CGFloat(g)/255, blue: CGFloat(b)/255, alpha: 1)
}
let defaultFG = rgb(208, 210, 218)
let defaultBG = rgb(17, 19, 27)
var foreground = defaultFG
var background = defaultBG
var bold = false
var inverse = false
defaultBG.setFill()
NSRect(x: 0, y: 0, width: width, height: height).fill()

func indexed(_ value: Int) -> NSColor {
    let base = [0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0,
                0x808080, 0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff, 0x00ffff, 0xffffff]
    if value < 16 {
        let color = base[max(0, value)]
        return rgb((color >> 16) & 255, (color >> 8) & 255, color & 255)
    }
    if value >= 232 {
        let gray = 8 + 10 * (min(value, 255) - 232)
        return rgb(gray, gray, gray)
    }
    let cube = [0, 95, 135, 175, 215, 255]
    let n = value - 16
    return rgb(cube[n / 36], cube[(n / 6) % 6], cube[n % 6])
}

func style(_ codes: [Int]) {
    var i = 0
    while i < codes.count {
        let code = codes[i]
        switch code {
        case 0: foreground = defaultFG; background = defaultBG; bold = false; inverse = false
        case 1: bold = true
        case 22: bold = false
        case 7: inverse = true
        case 27: inverse = false
        case 30...37: foreground = indexed(code - 30)
        case 40...47: background = indexed(code - 40)
        case 90...97: foreground = indexed(code - 90 + 8)
        case 100...107: background = indexed(code - 100 + 8)
        case 39: foreground = defaultFG
        case 49: background = defaultBG
        case 38, 48:
            var color: NSColor?
            if i + 4 < codes.count && codes[i+1] == 2 {
                color = rgb(codes[i+2], codes[i+3], codes[i+4]); i += 4
            } else if i + 2 < codes.count && codes[i+1] == 5 {
                color = indexed(codes[i+2]); i += 2
            }
            if let color = color {
                if code == 38 { foreground = color } else { background = color }
            }
        default: break
        }
        i += 1
    }
}

for (rowNumber, row) in rows.enumerated() {
    var column = 0
    var cursor = row.startIndex
    let y = CGFloat(height) - padding - CGFloat(rowNumber + 1) * cellHeight
    while cursor < row.endIndex {
        if row[cursor] == "\u{1b}", let end = row[cursor...].firstIndex(of: "m") {
            let codeStart = row.index(cursor, offsetBy: 2)
            let codes = row[codeStart..<end].split(separator: ";", omittingEmptySubsequences: false).map { Int($0) ?? 0 }
            style(codes.isEmpty ? [0] : codes)
            cursor = row.index(after: end)
            continue
        }
        let glyph = String(row[cursor])
        let actualFont = bold ? boldFont : font
        let cells = max(1, Int(round((glyph as NSString).size(withAttributes: [.font: actualFont]).width / cellWidth)))
        let fg = inverse ? background : foreground
        let bg = inverse ? foreground : background
        bg.setFill()
        NSRect(x: padding + CGFloat(column) * cellWidth, y: y,
               width: CGFloat(cells) * cellWidth, height: cellHeight).fill()
        (glyph as NSString).draw(at: NSPoint(x: padding + CGFloat(column) * cellWidth, y: y),
                                withAttributes: [.font: actualFont, .foregroundColor: fg])
        column += cells
        cursor = row.index(after: cursor)
    }
    background.setFill()
    NSRect(x: padding + CGFloat(column) * cellWidth, y: y,
           width: CGFloat(max(0, columns - column)) * cellWidth, height: cellHeight).fill()
}
NSGraphicsContext.restoreGraphicsState()
try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
print(CommandLine.arguments[2])
