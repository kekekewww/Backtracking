import AppKit
import Foundation

// MARK: - Theme & Contrast Constants (Packages 8 & 9, matching backtracking-ui/src/theme.rs)

enum WaveformTheme {
    static let defaultAccentRGB: UInt32 = 0x67E7B5 // (103, 231, 181)
    static let legacyMacDefaultAccentRGB: UInt32 = 0x52E3D1
    static let plotBackgroundRGB: UInt32 = 0x161A1D // (22, 26, 29)
    static let panelBackgroundRGB: UInt32 = 0x1B1F22 // dark window chrome

    static let plotBackground = srgb(22, 26, 29)
    static let panelBackground = srgb(27, 31, 34)
    static let centerLine = srgb(47, 60, 64)
    static let rowLabel = NSColor(srgbRed: 180 / 255, green: 180 / 255, blue: 180 / 255, alpha: 1)
    static let flagAndFade = srgb(255, 201, 109)
    static let playhead = NSColor(srgbRed: 245 / 255, green: 245 / 255, blue: 245 / 255, alpha: 1)
    static let pairBacking = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 7 / 255)
    static let pairDivider = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 22 / 255)
    static let laneDivider = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 36 / 255)
    static let scrollbarTrack = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 18 / 255)
    static let scrollbarThumb = NSColor(srgbRed: 128 / 255, green: 128 / 255, blue: 128 / 255, alpha: 1)
    static let scrollbarThumbActive = NSColor(srgbRed: 200 / 255, green: 200 / 255, blue: 200 / 255, alpha: 1)

    static let statusUnavailable = srgb(246, 196, 118)
    static let statusPaused = srgb(54, 57, 60)
    static let statusWaiting = srgb(76, 86, 88)
    static let statusRecordingBright = srgb(249, 78, 88)
    static let statusRecordingDim = srgb(103, 44, 48)
    static let compactPaused = NSColor(srgbRed: 214 / 255, green: 214 / 255, blue: 214 / 255, alpha: 1)
    static let compactWaiting = NSColor(srgbRed: 132 / 255, green: 132 / 255, blue: 132 / 255, alpha: 1)

    static let barGuideAlpha: CGFloat = 104 / 255
    static let secondsGuideAlpha: CGFloat = 52 / 255
    static let envelopeFillAlpha: CGFloat = 0.4
    static let envelopeAmplitudeScale: CGFloat = 0.47
    static let cornerRadius: CGFloat = 6

    static func srgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, alpha: CGFloat = 1) -> NSColor {
        NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: alpha)
    }

    static func color(_ rgb: UInt32, alpha: CGFloat = 1) -> NSColor {
        NSColor(
            srgbRed: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: alpha
        )
    }

    /// Complement of `rgb`, falling back to white when no channel differs by at least 80.
    static func selectionRGB(for rgb: UInt32) -> UInt32 {
        let comp = rgb ^ 0xFFFFFF
        let dr = abs(Int((rgb >> 16) & 0xFF) - Int((comp >> 16) & 0xFF))
        let dg = abs(Int((rgb >> 8) & 0xFF) - Int((comp >> 8) & 0xFF))
        let db = abs(Int(rgb & 0xFF) - Int(comp & 0xFF))
        return max(dr, max(dg, db)) >= 80 ? comp : 0xFFFFFF
    }

    static func relativeLuminance(_ rgb: UInt32) -> Double {
        func linear(_ channel: UInt32) -> Double {
            let v = Double(channel) / 255.0
            return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * linear((rgb >> 16) & 0xFF)
            + 0.7152 * linear((rgb >> 8) & 0xFF)
            + 0.0722 * linear(rgb & 0xFF)
    }

    static func contrastRatio(_ a: UInt32, _ b: UInt32) -> Double {
        let la = relativeLuminance(a)
        let lb = relativeLuminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }

    static func isReadableGradientStop(_ rgb: UInt32) -> Bool {
        contrastRatio(rgb, plotBackgroundRGB) >= 2.0 && contrastRatio(rgb, panelBackgroundRGB) >= 2.0
    }

    static func hexString(_ rgb: UInt32) -> String {
        String(format: "#%06X", rgb & 0xFFFFFF)
    }

    static func parseHexRGB(_ text: String) -> UInt32? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("#"), trimmed.count == 7 else { return nil }
        return UInt32(trimmed.dropFirst(), radix: 16)
    }
}

// MARK: - Row Identity & Cross-Row Selection (Package 1)

enum RowIdentity: Hashable, Sendable {
    case whole(lane: Int)
    case stereoSide(lane: Int, side: Int) // 0 = L, 1 = R
    case pair(lane: Int, side: Int)       // 0 = L (view 67), 1 = R (view 68)
    case channel(lane: Int, index: Int)   // 0-based native channel (view 3 + index)

    var lane: Int {
        switch self {
        case .whole(let lane), .stereoSide(let lane, _), .pair(let lane, _), .channel(let lane, _):
            return lane
        }
    }

    var viewCode: UInt32 {
        switch self {
        case .whole: return 0
        case .stereoSide(_, let side): return UInt32(side + 1)
        case .pair(_, let side): return UInt32(side + 67)
        case .channel(_, let index): return UInt32(index + 3)
        }
    }

    var isPair: Bool {
        if case .pair = self { return true }
        return false
    }

    var isWhole: Bool {
        if case .whole = self { return true }
        return false
    }

    func sameSelectionUnit(as other: RowIdentity) -> Bool {
        if self == other { return true }
        if case .pair(let a, _) = self, case .pair(let b, _) = other { return a == b }
        return false
    }
}

struct WaveformRowSpec: Equatable, Sendable {
    let identity: RowIdentity
    let lane: Int
    let view: UInt32
    let shortLabel: String?
    let fullLabel: String
    let laneChannelCount: Int
    let channelNumberOneBased: Int?

    var isPair: Bool { identity.isPair }
}

// MARK: - Hide Silent Channels (Package 2)

struct ChannelActivityTracker: Equatable, Sendable {
    static let pollInterval: TimeInterval = 0.25
    static let quietAnswersRequired: UInt8 = 2

    private(set) var shown: UInt64?
    private var quietCounts: [UInt8] = Array(repeating: 0, count: 64)

    mutating func reset() {
        shown = nil
        quietCounts = Array(repeating: 0, count: 64)
    }

    mutating func recordAnswer(_ active: UInt64) {
        let previous = shown ?? 0
        var next = previous | active
        for ch in 0..<64 {
            let bit: UInt64 = 1 << UInt64(ch)
            if (active & bit) != 0 || (previous & bit) == 0 {
                quietCounts[ch] = 0
                continue
            }
            quietCounts[ch] &+= 1
            if quietCounts[ch] >= Self.quietAnswersRequired {
                next &= ~bit
                quietCounts[ch] = 0
            }
        }
        shown = next
    }

    /// Returns the 0-based channel indices to display below the L/R pair.
    /// `activeMask == nil` means show all channels (hiding disabled or add-on/sender source).
    /// When hiding is enabled and no channel other than FL/FR/L/R is active, returns `[]` so only
    /// the L/R pair is displayed.
    static func shownChannels(width: Int, labels: [String], activeMask: UInt64?) -> [Int] {
        guard let mask = activeMask else { return Array(0..<width) }
        func isActive(_ ch: Int) -> Bool { ch < 64 && (mask & (1 << UInt64(ch))) != 0 }
        func isFront(_ ch: Int) -> Bool {
            guard labels.indices.contains(ch) else { return false }
            let lbl = labels[ch]
            return lbl == "FL" || lbl == "FR" || lbl == "L" || lbl == "R"
        }
        let hasNonFront = (0..<width).contains { isActive($0) && !isFront($0) }
        guard hasNonFront else { return [] }
        return (0..<width).filter(isActive)
    }
}

// MARK: - Multi-Row Stack Layout & Scroll (Package 4, ported from backtracking-ui/src/stack.rs)

struct LaneLayoutInput: Equatable, Sendable {
    let rows: Int
    let stacked: Bool
}

struct LaneLayoutBox: Equatable, Sendable {
    let top: CGFloat       // Top offset in points from top of plot audio area
    let height: CGFloat    // Visible height in points
    let rows: Int          // Total rows in this lane
    let first: Int         // First visible row index (0-based)
    let shown: Int         // Number of visible rows
    let rowHeight: CGFloat // Uniform height per row in points

    var scrolls: Bool { shown < rows }
    var visibleRange: Range<Int> { first..<(first + shown) }

    func rowTop(_ row: Int) -> CGFloat? {
        guard visibleRange.contains(row) else { return nil }
        return top + CGFloat(row - first) * rowHeight
    }

    func thumb() -> ClosedRange<CGFloat> {
        guard scrolls else { return top...(top + height) }
        let length = min(height, max(RowStackLayout.minThumb, height * CGFloat(shown) / CGFloat(max(1, rows))))
        let travel = max(0, height - length)
        let denom = CGFloat(max(1, rows - shown))
        let start = top + travel * CGFloat(first) / denom
        return start...(start + length)
    }

    func firstForThumb(_ thumbTop: CGFloat) -> Int {
        let t = thumb()
        let travel = height - (t.upperBound - t.lowerBound)
        guard scrolls, travel > 0 else { return 0 }
        let fraction = min(1, max(0, (thumbTop - top) / travel))
        return Int((fraction * CGFloat(rows - shown)).rounded())
    }
}

enum RowStackLayout {
    static let minRowHeight: CGFloat = 14
    static let comfortableRowHeight: CGFloat = 20
    static let labelledRowHeight: CGFloat = 9
    static let minThumb: CGFloat = 10
    static let scrollbarHitWidth: CGFloat = 8
    static let scrollbarVisualWidth: CGFloat = 3

    static func shortLabel(_ label: String) -> String {
        let trimmed = label.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.count <= 4 { return trimmed }
        var digits = ""
        for ch in trimmed.reversed() {
            if ch.isASCII && ch.isNumber {
                digits.append(ch)
            } else {
                break
            }
        }
        if (1...4).contains(digits.count) {
            return String(digits.reversed())
        }
        return String(trimmed.prefix(4))
    }

    static func labelGutterWidth(laneCount: Int, hasStacked: Bool) -> CGFloat {
        if laneCount > 2 {
            return hasStacked ? 36 : 16
        }
        return hasStacked ? 28 : 0
    }

    static func comfortableAudioHeight(lanes: [LaneLayoutInput]) -> CGFloat? {
        guard lanes.count >= 3 || lanes.contains(where: \.stacked) else { return nil }
        let total = lanes.reduce(0) { $0 + $1.rows }
        guard total > 0 else { return nil }
        return CGFloat(total) * comfortableRowHeight
    }

    /// 1–2 lanes: mono/stereo stay in place, stacked lanes share remaining row budget proportionally.
    static func layOut(height: CGFloat, lanes: [LaneLayoutInput], first: [Int]) -> [LaneLayoutBox] {
        let h = max(0, height)
        let total = lanes.reduce(0) { $0 + $1.rows }
        var shown = lanes.map(\.rows)
        let stackedCount = lanes.filter { $0.stacked && $0.rows > 0 }.count
        if stackedCount > 0 && CGFloat(total) * minRowHeight > h {
            let fixed = lanes.filter { !$0.stacked }.reduce(0) { $0 + $1.rows }
            let fitting = Int(floor(h / minRowHeight))
            var budget = max(stackedCount, max(0, fitting - fixed))
            for i in lanes.indices where lanes[i].stacked && lanes[i].rows > 0 {
                shown[i] = 1
                budget -= 1
            }
            while budget > 0 {
                var bestIndex: Int?
                var bestRatio = CGFloat.greatestFiniteMagnitude
                for i in lanes.indices where lanes[i].stacked && shown[i] < lanes[i].rows {
                    let ratio = CGFloat(shown[i]) / CGFloat(lanes[i].rows)
                    if ratio < bestRatio {
                        bestRatio = ratio
                        bestIndex = i
                    }
                }
                guard let idx = bestIndex else { break }
                shown[idx] += 1
                budget -= 1
            }
        }
        let onScreen = shown.reduce(0, +)
        let rowHeight: CGFloat = onScreen == 0 ? 0 : h / CGFloat(onScreen)
        var top: CGFloat = 0
        return lanes.indices.map { i in
            let req = first.indices.contains(i) ? first[i] : 0
            let clampedFirst = min(max(0, req), max(0, lanes[i].rows - shown[i]))
            let box = LaneLayoutBox(
                top: top, height: CGFloat(shown[i]) * rowHeight,
                rows: lanes[i].rows, first: clampedFirst, shown: shown[i], rowHeight: rowHeight
            )
            top += box.height
            return box
        }
    }

    /// >2 lanes: whole plot scrolls as one uniform column.
    static func layOutColumn(height: CGFloat, lanes: [LaneLayoutInput], first: Int) -> (boxes: [LaneLayoutBox], column: LaneLayoutBox) {
        let h = max(0, height)
        let total = lanes.reduce(0) { $0 + $1.rows }
        let fitting = Int(floor(h / minRowHeight))
        if total <= fitting {
            let boxes = layOut(height: h, lanes: lanes, first: [])
            let rowHeight = boxes.first?.rowHeight ?? 0
            let column = LaneLayoutBox(top: 0, height: h, rows: total, first: 0, shown: total, rowHeight: rowHeight)
            return (boxes, column)
        }
        let shown = min(total, max(1, fitting))
        let rowHeight = shown == 0 ? 0 : h / CGFloat(shown)
        let clampedFirst = min(max(0, first), max(0, total - shown))
        let end = clampedFirst + shown
        var start = 0
        let boxes: [LaneLayoutBox] = lanes.map { lane in
            let laneEnd = start + lane.rows
            let from = max(start, clampedFirst)
            let to = min(laneEnd, end)
            let box: LaneLayoutBox
            if from < to {
                box = LaneLayoutBox(
                    top: CGFloat(from - clampedFirst) * rowHeight,
                    height: CGFloat(to - from) * rowHeight,
                    rows: lane.rows, first: from - start, shown: to - from, rowHeight: rowHeight
                )
            } else {
                let above = laneEnd <= clampedFirst
                box = LaneLayoutBox(
                    top: above ? 0 : h, height: 0,
                    rows: lane.rows, first: above ? lane.rows : 0, shown: 0, rowHeight: rowHeight
                )
            }
            start = laneEnd
            return box
        }
        let column = LaneLayoutBox(top: 0, height: h, rows: total, first: clampedFirst, shown: shown, rowHeight: rowHeight)
        return (boxes, column)
    }

    static func rowAt(boxes: [LaneLayoutBox], yFromTop: CGFloat) -> (lane: Int, row: Int)? {
        for (index, box) in boxes.enumerated() where box.shown > 0 {
            let offset = yFromTop - box.top
            if offset >= 0 && offset < box.height {
                let rel = min(box.shown - 1, max(0, Int(offset / max(0.001, box.rowHeight))))
                return (index, box.first + rel)
            }
        }
        return nil
    }

    /// Nearest visible row when pointer is above the first or below the last visible row.
    static func nearestVisibleRow(boxes: [LaneLayoutBox], yFromTop: CGFloat) -> (lane: Int, row: Int)? {
        if let exact = rowAt(boxes: boxes, yFromTop: yFromTop) { return exact }
        let visible = boxes.enumerated().filter { $0.element.shown > 0 }
        guard let first = visible.first, let last = visible.last else { return nil }
        if yFromTop < first.element.top {
            return (first.offset, first.element.first)
        }
        return (last.offset, last.element.first + last.element.shown - 1)
    }

    static func rowsBetween(allRows: [WaveformRowSpec], anchor: RowIdentity, cursor: RowIdentity) -> [RowIdentity] {
        func unitRange(for index: Int) -> Range<Int> {
            let id = allRows[index].identity
            guard id.isPair else { return index..<(index + 1) }
            var lo = index
            var hi = index + 1
            while lo > 0 && allRows[lo - 1].identity.sameSelectionUnit(as: id) { lo -= 1 }
            while hi < allRows.count && allRows[hi].identity.sameSelectionUnit(as: id) { hi += 1 }
            return lo..<hi
        }
        let a = allRows.firstIndex { $0.identity == anchor }
        let b = allRows.firstIndex { $0.identity == cursor }
        switch (a, b) {
        case (let ia?, let ib?):
            let ra = unitRange(for: ia)
            let rb = unitRange(for: ib)
            let start = min(ra.lowerBound, rb.lowerBound)
            let end = max(ra.upperBound, rb.upperBound)
            return allRows[start..<end].map(\.identity)
        case (let found?, nil), (nil, let found?):
            return allRows[unitRange(for: found)].map(\.identity)
        case (nil, nil):
            return [anchor]
        }
    }

    /// Formats selected row identities for status line and length label (matches Windows `rows_label`).
    static func rowsLabel(_ rows: [RowIdentity], dual: Bool, channelLabels: [[String]]) -> String {
        func short(_ row: RowIdentity) -> String {
            switch row {
            case .whole(let lane):
                return captureLaneLetter(lane)
            case .stereoSide(_, let side):
                return side == 0 ? "L" : "R"
            case .pair:
                return "L/R"
            case .channel(let lane, let index):
                let name = channelLabels.indices.contains(lane) && channelLabels[lane].indices.contains(index)
                    ? channelLabels[lane][index] : "Channel \(index + 1)"
                return shortLabel(name)
            }
        }
        func named(_ row: RowIdentity) -> String {
            switch row {
            case .channel(let lane, _), .pair(let lane, _):
                return dual ? "\(captureLaneLetter(lane)) \(short(row))" : short(row)
            default:
                return short(row)
            }
        }
        guard let first = rows.first else { return "" }
        if rows.count == 1 { return named(first) }
        if rows.count == 2, case .pair(let a, _) = rows[0], case .pair(let b, _) = rows[1], a == b {
            return named(first)
        }
        let last = rows[rows.count - 1]
        if first.lane == last.lane {
            return "\(named(first))–\(short(last))"
        }
        return "\(named(first))–\(named(last))"
    }
}
