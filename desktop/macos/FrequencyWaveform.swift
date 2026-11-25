import AppKit

struct FrequencyGradient: Equatable, Sendable {
    struct Stop: Equatable, Sendable {
        var position: Double
        var rgb: UInt32
    }

    static let standard = FrequencyGradient(stops: [
        Stop(position: 0, rgb: 0xE8302C),
        Stop(position: 1, rgb: 0x2C68EC)
    ])
    static let bandCount = 6
    var stops: [Stop]

    init(stops: [Stop]) {
        self.stops = stops
    }

    init?(settings: [[String: Any]]) {
        guard (2...8).contains(settings.count) else { return nil }
        let stops = settings.compactMap { item -> Stop? in
            guard let position = item["position"] as? Double, (0...1).contains(position),
                  let number = item["rgb"] as? NSNumber,
                  let rgb = UInt32(exactly: number.doubleValue), rgb <= 0xFFFFFF else { return nil }
            return Stop(position: position, rgb: rgb)
        }
        guard stops.count == settings.count else { return nil }
        self.stops = stops
    }

    var settings: [[String: Any]] {
        stops.map { ["position": $0.position, "rgb": Int($0.rgb)] }
    }

    private static func linear(_ value: Double) -> Double {
        value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4)
    }

    private static func linearRGB(_ rgb: UInt32) -> SIMD3<Double> {
        SIMD3(linear(Double((rgb >> 16) & 0xFF) / 255),
            linear(Double((rgb >> 8) & 0xFF) / 255), linear(Double(rgb & 0xFF) / 255))
    }

    private static func rgb(_ linear: SIMD3<Double>) -> UInt32 {
        func channel(_ value: Double) -> UInt32 {
            let encoded = value <= 0.0031308 ? value * 12.92 : 1.055 * pow(value, 1 / 2.4) - 0.055
            return UInt32((min(1, max(0, encoded)) * 255).rounded())
        }
        return (channel(linear.x) << 16) | (channel(linear.y) << 8) | channel(linear.z)
    }

    private func color(at position: Double) -> SIMD3<Double> {
        let ordered = stops.sorted { $0.position < $1.position }
        guard let after = ordered.firstIndex(where: { $0.position > position }) else {
            return Self.linearRGB(ordered[ordered.count - 1].rgb)
        }
        guard after > 0 else { return Self.linearRGB(ordered[0].rgb) }
        let before = ordered[after - 1]
        let next = ordered[after]
        let ratio = (position - before.position) / (next.position - before.position)
        return Self.linearRGB(before.rgb) * (1 - ratio) + Self.linearRGB(next.rgb) * ratio
    }

    func addingStop() -> FrequencyGradient {
        guard stops.count < 8 else { return self }
        let positions = [0.0] + stops.map(\.position).sorted() + [1.0]
        let gaps = zip(positions, positions.dropFirst())
        let (low, high) = gaps.max { $0.1 - $0.0 < $1.1 - $1.0 } ?? (0, 1)
        let position = (low + high) / 2
        return FrequencyGradient(stops: stops + [Stop(position: position, rgb: Self.rgb(color(at: position)))])
    }

    func colors(for energies: [Float]) -> [UInt32?] {
        let palette = (0..<Self.bandCount).map {
            Self.linearRGB(Self.rgb(color(at: Double($0) / Double(Self.bandCount - 1))))
        }
        return stride(from: 0, to: energies.count, by: Self.bandCount).map { first in
            var total = 0.0
            var sum = SIMD3<Double>(repeating: 0)
            for (band, value) in energies[first..<min(energies.count, first + Self.bandCount)].enumerated()
                where value.isFinite && value > 0 {
                total += Double(value)
                sum += palette[band] * Double(value)
            }
            guard total > 0 else { return nil }
            return Self.rgb(sum / total)
        }
    }

    var previewColors: [UInt32?] {
        (0...96).map { Self.rgb(color(at: Double($0) / 96)) }
    }

    func rgbAt(position: Double) -> UInt32 {
        Self.rgb(color(at: position))
    }

    var hasUnreadableStops: Bool {
        stops.contains { !WaveformTheme.isReadableGradientStop($0.rgb) }
    }

    func readableForDrawing() -> FrequencyGradient {
        guard hasUnreadableStops else { return self }
        let adjusted = stops.map { stop -> Stop in
            if WaveformTheme.isReadableGradientStop(stop.rgb) { return stop }
            return Stop(position: stop.position, rgb: Self.standard.rgbAt(position: stop.position))
        }
        return FrequencyGradient(stops: adjusted)
    }

    static func dim(_ rgb: UInt32) -> UInt32 {
        let red = (rgb >> 16) & 255
        let green = (rgb >> 8) & 255
        let blue = rgb & 255
        let gray = (red * 54 + green * 183 + blue * 19) / 256
        func dim(_ color: UInt32) -> UInt32 {
            UInt32((Double((color + gray) / 2) * 0.76).rounded())
        }
        return (dim(red) << 16) | (dim(green) << 8) | dim(blue)
    }
}

struct FrequencyWaveform: Sendable {
    let range: Range<UInt64>
    let rows: [[Float]]

    func colors(
        for row: Int, in visible: Range<UInt64>, bins: Int,
        gradient: FrequencyGradient, liveFrames: UInt64
    ) -> [UInt32?] {
        guard rows.indices.contains(row), !range.isEmpty, !visible.isEmpty, bins > 0 else { return [] }
        let available = gradient.colors(for: rows[row])
        guard !available.isEmpty else { return [] }
        func middle(_ index: Int) -> UInt64 {
            visible.lowerBound + UInt64(Double(visible.count) * (Double(index) + 0.5) / Double(bins))
        }
        var colors = (0..<bins).map { index -> UInt32? in
            let frame = middle(index)
            guard range.contains(frame) else { return nil }
            let bin = Int(Double(frame - range.lowerBound) / Double(range.count) * Double(available.count))
            return available[min(available.count - 1, bin)]
        }
        var last: Int?
        for index in colors.indices where colors[index] != nil {
            if let last, index > last + 1 {
                for gap in (last + 1)..<index {
                    colors[gap] = gap - last < index - gap ? colors[last] : colors[index]
                }
            }
            last = index
        }
        if let last, liveFrames > 0, visible.upperBound > range.upperBound,
           visible.upperBound - range.upperBound <= liveFrames,
           range.upperBound - middle(last) <= liveFrames {
            for index in colors.indices where middle(index) >= range.upperBound {
                colors[index] = colors[last]
            }
        }
        return colors
    }

    static func stripeImage(_ colors: [UInt32?], fallback: UInt32, dimmed: Bool) -> CGImage? {
        guard !colors.isEmpty else { return nil }
        let colors = colors.flatMap { color -> [UInt8] in
            let rgb = color ?? fallback
            let drawn = dimmed ? FrequencyGradient.dim(rgb) : rgb
            return [UInt8(drawn >> 16), UInt8((drawn >> 8) & 255), UInt8(drawn & 255), dimmed ? 128 : 255]
        }
        guard let data = CGDataProvider(data: Data(colors) as CFData) else { return nil }
        return CGImage(width: colors.count / 4, height: 1, bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: colors.count, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: data, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }
}

final class FrequencyGradientPreview: NSView {
    var gradient = FrequencyGradient.standard {
        didSet { needsDisplay = true }
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let drawn = gradient.readableForDrawing()
        guard let context = NSGraphicsContext.current?.cgContext,
              let image = FrequencyWaveform.stripeImage(drawn.previewColors, fallback: 0, dimmed: false) else { return }
        context.interpolationQuality = .none
        context.draw(image, in: bounds)
        NSColor.white.withAlphaComponent(0.85).setStroke()
        for stop in drawn.stops {
            let x = min(bounds.maxX - 0.5, max(bounds.minX + 0.5, bounds.width * stop.position))
            let tick = NSBezierPath()
            tick.lineWidth = 2
            tick.move(to: NSPoint(x: x, y: bounds.minY))
            tick.line(to: NSPoint(x: x, y: bounds.minY + 4))
            tick.stroke()
        }
        NSColor(white: 1, alpha: 0.2).setStroke()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 2, yRadius: 2).stroke()
    }
}
