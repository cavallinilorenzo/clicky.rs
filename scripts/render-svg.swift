// Rasterises an SVG into a square sRGB PNG with CoreSVG (through NSImage).
// Usage: render-svg <in.svg> <out.png> <pixels>
import AppKit
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard args.count == 4, let pixels = Int(args[3]), let image = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("usage: render-svg <in.svg> <out.png> <pixels>\n".data(using: .utf8)!)
    exit(2)
}
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let cg = CGContext(data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0,
                   space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
NSGraphicsContext.current = NSGraphicsContext(cgContext: cg, flipped: false)
image.draw(in: NSRect(x: 0, y: 0, width: pixels, height: pixels))
NSGraphicsContext.current = nil
let url = URL(fileURLWithPath: args[2]) as CFURL
let destination = CGImageDestinationCreateWithURL(url, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(destination, cg.makeImage()!, nil)
guard CGImageDestinationFinalize(destination) else { exit(1) }
