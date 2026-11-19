import AppKit

final class LineChartView: NSView {
    struct Series {
        var name: String
        var color: NSColor
        var values: [CGFloat]
        var visible: Bool = true
    }
    struct Selection: Equatable {
        let bucketIndex: Int
        let seriesName: String
        let anchor: NSRect
    }

    var series: [Series] = [] { didSet { dataChanged() } }
    var bucketLabels: [String] = [] { didSet { updateAccessibility(); needsDisplay = true } }
    var onSelect: ((Int, String, NSRect) -> Void)?
    private var hover: Selection?
    private var focusedSelection: Selection?
    private var tracking: NSTrackingArea?
    var selectedBucketIndex: Int? { focusedSelection?.bucketIndex }
    var selectedSeriesName: String? { focusedSelection?.seriesName }
    override var acceptsFirstResponder: Bool { !visibleSeries.isEmpty }
    private var visibleSeries: [Series] { series.filter { $0.visible && !$0.values.isEmpty } }
    private var bucketCount = 0
    private var maximum: CGFloat = 1

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        setAccessibilityLabel("Activity chart")
        setAccessibilityHelp("Use Left and Right to choose a time, Up and Down to choose a series, then Return or Space to inspect its commits and runs.")
        updateAccessibility()
    }
    required init?(coder: NSCoder) { fatalError() }

    private func point(index: Int, value: CGFloat) -> NSPoint {
        let safeValue = value.isFinite ? max(0, min(value, maximum)) : 0
        let x = bucketCount > 1 ? bounds.width * CGFloat(index) / CGFloat(bucketCount - 1) : bounds.width / 2
        return NSPoint(x: x, y: max(0, bounds.height - 18) * safeValue / maximum + min(9, bounds.height / 2))
    }

    private func selection(index: Int, item: Series) -> Selection? {
        guard !item.values.isEmpty, bounds.width > 0, bounds.height > 0 else { return nil }
        let index = min(max(0, index), item.values.count - 1)
        let point = point(index: index, value: item.values[index])
        let anchor = NSRect(x: point.x - 6, y: point.y - 6, width: 12, height: 12).intersection(bounds)
        return Selection(bucketIndex: index, seriesName: item.name, anchor: anchor)
    }

    /// Resolves any point to the nearest visible series at its time bucket.
    /// Clicking a zero-value baseline is intentionally a valid inspection.
    func selection(for location: NSPoint) -> Selection? {
        guard bucketCount > 0, bounds.width > 0, bounds.height > 0,
              location.x.isFinite, location.y.isFinite else { return nil }
        let fraction = min(1, max(0, location.x / bounds.width))
        let index = bucketCount > 1 ? Int((fraction * CGFloat(bucketCount - 1)).rounded()) : 0
        let nearest = visibleSeries.min { lhs, rhs in
            let left = point(index: min(index, lhs.values.count - 1), value: lhs.values[min(index, lhs.values.count - 1)])
            let right = point(index: min(index, rhs.values.count - 1), value: rhs.values[min(index, rhs.values.count - 1)])
            let leftDistance = hypot(left.x - location.x, left.y - location.y)
            let rightDistance = hypot(right.x - location.x, right.y - location.y)
            return leftDistance < rightDistance
        }
        return nearest.flatMap { selection(index: index, item: $0) }
    }

    /// Selects and opens a bucket. Unknown/hidden series fall back to the first
    /// visible series; indices clamp to that series rather than indexing blindly.
    @discardableResult
    func selectBucket(_ index: Int, seriesName: String? = nil) -> Selection? {
        guard let item = visibleSeries.first(where: { $0.name == seriesName }) ?? visibleSeries.first,
              let selected = selection(index: index, item: item) else { return nil }
        focusedSelection = selected
        updateAccessibility()
        needsDisplay = true
        onSelect?(selected.bucketIndex, selected.seriesName, selected.anchor)
        return selected
    }

    private func dataChanged() {
        let visible = visibleSeries
        bucketCount = visible.map { $0.values.count }.max() ?? 0
        maximum = max(1, visible.flatMap { $0.values }.filter { $0.isFinite }.max() ?? 1)
        func refreshed(_ old: Selection?) -> Selection? {
            guard let old, let item = visibleSeries.first(where: { $0.name == old.seriesName }) else { return nil }
            return selection(index: old.bucketIndex, item: item)
        }
        focusedSelection = refreshed(focusedSelection)
        hover = refreshed(hover)
        window?.invalidateCursorRects(for: self)
        updateAccessibility()
        needsDisplay = true
    }

    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(area)
        tracking = area
        super.updateTrackingAreas()
    }
    override func resetCursorRects() {
        if !visibleSeries.isEmpty { addCursorRect(bounds, cursor: .pointingHand) }
    }
    override func mouseMoved(with event: NSEvent) {
        hover = selection(for: convert(event.locationInWindow, from: nil))
        toolTip = hover.map(description(for:))
        needsDisplay = true
    }
    override func mouseEntered(with event: NSEvent) { mouseMoved(with: event) }
    override func mouseExited(with event: NSEvent) { hover = nil; toolTip = nil; needsDisplay = true }
    override func mouseDown(with event: NSEvent) {
        guard let selected = selection(for: convert(event.locationInWindow, from: nil)) else { return }
        window?.makeFirstResponder(self)
        selectBucket(selected.bucketIndex, seriesName: selected.seriesName)
    }
    override func becomeFirstResponder() -> Bool {
        if focusedSelection == nil, let item = visibleSeries.first {
            focusedSelection = selection(index: item.values.count - 1, item: item)
        }
        updateAccessibility()
        needsDisplay = true
        return true
    }
    override func resignFirstResponder() -> Bool { needsDisplay = true; return true }
    override func keyDown(with event: NSEvent) {
        guard let item = visibleSeries.first(where: { $0.name == focusedSelection?.seriesName }) ?? visibleSeries.first else { super.keyDown(with: event); return }
        let index = focusedSelection?.bucketIndex ?? item.values.count - 1
        switch event.keyCode {
        case 123, 124: // Left, Right
            focusedSelection = selection(index: index + (event.keyCode == 123 ? -1 : 1), item: item)
        case 125, 126: // Down, Up
            let visible = visibleSeries
            let current = visible.firstIndex(where: { $0.name == item.name }) ?? 0
            let next = min(visible.count - 1, max(0, current + (event.keyCode == 125 ? 1 : -1)))
            focusedSelection = selection(index: index, item: visible[next])
        case 36, 49, 76: // Return, Space, keypad Enter
            selectBucket(index, seriesName: item.name)
        default: super.keyDown(with: event); return
        }
        hover = nil
        updateAccessibility()
        needsDisplay = true
    }
    override func accessibilityPerformPress() -> Bool {
        guard let item = visibleSeries.first(where: { $0.name == focusedSelection?.seriesName }) ?? visibleSeries.first else { return false }
        return selectBucket(focusedSelection?.bucketIndex ?? item.values.count - 1, seriesName: item.name) != nil
    }

    private func description(for selected: Selection) -> String {
        let date = bucketLabels.indices.contains(selected.bucketIndex) ? bucketLabels[selected.bucketIndex] : "Time bucket \(selected.bucketIndex + 1)"
        let item = visibleSeries.first(where: { $0.name == selected.seriesName })
        let value = item.flatMap { $0.values.indices.contains(selected.bucketIndex) ? $0.values[selected.bucketIndex] : nil } ?? 0
        return "\(date) · \(selected.seriesName): \(String(format: "%.0f", value.isFinite ? value : 0))"
    }
    private func updateAccessibility() {
        let selected = focusedSelection ?? visibleSeries.first.flatMap { selection(index: $0.values.count - 1, item: $0) }
        setAccessibilityValue(selected.map(description(for:)) ?? "No activity series available")
        setAccessibilityEnabled(!visibleSeries.isEmpty)
    }

    override func draw(_ dirtyRect: NSRect) {
        let w = bounds.width
        let h = bounds.height
        guard w > 20, h > 20 else { return }
        for row in 0...3 {
            let y = h * CGFloat(row) / 3
            Theme.border.setStroke()
            let path = NSBezierPath()
            path.move(to: NSPoint(x: 0, y: y))
            path.line(to: NSPoint(x: w, y: y))
            path.lineWidth = 1
            path.stroke()
        }
        guard bucketCount > 0 else { return }
        let labelAttributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 10), .foregroundColor: Theme.textMuted]
        String(format: "%.0f", maximum).draw(at: NSPoint(x: 4, y: h - 14), withAttributes: labelAttributes)
        "0".draw(at: NSPoint(x: 4, y: 4), withAttributes: labelAttributes)
        for item in visibleSeries {
            let points = item.values.enumerated().map { point(index: $0.offset, value: $0.element) }
            guard let first = points.first, let last = points.last else { continue }
            let line = NSBezierPath()
            line.move(to: first)
            if points.count > 1 {
                for i in 0..<(points.count - 1) {
                    let p0 = i == 0 ? first : points[i - 1]
                    let p1 = points[i]
                    let p2 = points[i + 1]
                    let p3 = i + 2 < points.count ? points[i + 2] : p2
                    let clampY = { (value: CGFloat) -> CGFloat in min(h - 2, max(2, value)) }
                    let c1 = NSPoint(x: p1.x + (p2.x - p0.x) / 6, y: clampY(p1.y + (p2.y - p0.y) / 6))
                    let c2 = NSPoint(x: p2.x - (p3.x - p1.x) / 6, y: clampY(p2.y - (p3.y - p1.y) / 6))
                    line.curve(to: p2, controlPoint1: c1, controlPoint2: c2)
                }
                let fill = line.copy() as! NSBezierPath
                fill.line(to: NSPoint(x: last.x, y: 0))
                fill.line(to: NSPoint(x: first.x, y: 0))
                fill.close()
                NSGradient(colors: [item.color.withAlphaComponent(0.02), item.color.withAlphaComponent(0.30)])?.draw(in: fill, angle: 90)
                item.color.setStroke()
                line.lineWidth = 2
                line.stroke()
            }
            NSColor.white.setStroke()
            item.color.setFill()
            let dot = NSBezierPath(ovalIn: NSRect(x: last.x - 5, y: last.y - 5, width: 10, height: 10))
            dot.fill()
            dot.lineWidth = 2
            dot.stroke()
        }
        let active = hover ?? (window?.firstResponder === self ? focusedSelection : nil)
        if let active, let item = visibleSeries.first(where: { $0.name == active.seriesName }), item.values.indices.contains(active.bucketIndex) {
            let selectedPoint = point(index: active.bucketIndex, value: item.values[active.bucketIndex])
            Theme.textMuted.withAlphaComponent(0.55).setStroke()
            let crosshair = NSBezierPath()
            crosshair.move(to: NSPoint(x: selectedPoint.x, y: 0))
            crosshair.line(to: NSPoint(x: selectedPoint.x, y: h))
            crosshair.setLineDash([3, 3], count: 2, phase: 0)
            crosshair.lineWidth = 1
            crosshair.stroke()
            item.color.setFill()
            NSBezierPath(ovalIn: NSRect(x: selectedPoint.x - 5, y: selectedPoint.y - 5, width: 10, height: 10)).fill()
            let text = description(for: active)
            let attributes: [NSAttributedString.Key: Any] = [.font: Theme.uiFont(11), .foregroundColor: Theme.textBase]
            let textWidth = min(w - 20, (text as NSString).size(withAttributes: attributes).width)
            let rect = NSRect(x: min(w - textWidth - 16, max(4, selectedPoint.x - textWidth / 2 - 6)), y: selectedPoint.y > h - 42 ? selectedPoint.y - 34 : selectedPoint.y + 12, width: textWidth + 12, height: 24)
            Theme.layer01.setFill()
            NSBezierPath(roundedRect: rect, xRadius: 4, yRadius: 4).fill()
            (text as NSString).draw(in: rect.insetBy(dx: 6, dy: 5), withAttributes: attributes)
        }
        if window?.firstResponder === self {
            Theme.accent.withAlphaComponent(0.8).setStroke()
            let ring = NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1))
            ring.lineWidth = 1
            ring.stroke()
        }
    }
}

struct ChartInspectionItem {
    enum Kind { case commit, run, note }
    let date: Date
    let title: String
    let detail: String
    let kind: Kind
    let identifier: String
}

/// `title` is the selected bucket's timestamp/range. The caller owns the
/// popover and decides where a commit/run button navigates.
final class ChartInspectorController: NSViewController {
    var onClose: (() -> Void)?
    let inspectionTitle: String
    let items: [ChartInspectionItem]
    let onSelect: (ChartInspectionItem) -> Void

    init(title: String, items: [ChartInspectionItem], onSelect: @escaping (ChartInspectionItem) -> Void) {
        inspectionTitle = title
        self.items = items.sorted { $0.date > $1.date }
        self.onSelect = onSelect
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let root = NSView()
        root.wantsLayer = true
        root.layer?.backgroundColor = Theme.layer01.themeCGColor
        let heading = headerLabel(inspectionTitle, size: 13, weight: .semibold)
        heading.lineBreakMode = .byTruncatingTail
        heading.toolTip = inspectionTitle
        heading.translatesAutoresizingMaskIntoConstraints = false
        let close = NSButton(image: NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close activity inspector")!, target: self, action: #selector(closeInspector))
        close.isBordered = false
        close.toolTip = "Close activity inspector"
        close.setAccessibilityLabel("Close activity inspector")
        close.translatesAutoresizingMaskIntoConstraints = false
        let count = headerLabel(items.isEmpty ? "No events in this time bucket" : "\(items.count) event\(items.count == 1 ? "" : "s") · Select one to inspect", size: 11, color: Theme.textMuted)
        count.translatesAutoresizingMaskIntoConstraints = false
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        let document = FlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = document
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.distribution = .fill
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        root.addSubview(heading)
        root.addSubview(close)
        root.addSubview(count)
        root.addSubview(scroll)
        NSLayoutConstraint.activate([
            heading.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            heading.trailingAnchor.constraint(equalTo: close.leadingAnchor, constant: -8),
            heading.topAnchor.constraint(equalTo: root.topAnchor, constant: 14),
            close.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            close.centerYAnchor.constraint(equalTo: heading.centerYAnchor),
            close.widthAnchor.constraint(equalToConstant: 24),
            close.heightAnchor.constraint(equalToConstant: 24),
            count.leadingAnchor.constraint(equalTo: heading.leadingAnchor),
            count.trailingAnchor.constraint(equalTo: heading.trailingAnchor),
            count.topAnchor.constraint(equalTo: heading.bottomAnchor, constant: 5),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            scroll.topAnchor.constraint(equalTo: count.bottomAnchor, constant: 12),
            scroll.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -12),
            document.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            document.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            stack.topAnchor.constraint(equalTo: document.topAnchor),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor),
        ])
        if items.isEmpty {
            let empty = NSTextField(wrappingLabelWithString: "No commits or runs were recorded at this time. Choose another line point or spike to inspect its activity.")
            empty.font = Theme.uiFont(12)
            empty.textColor = Theme.textMuted
            stack.addArrangedSubview(empty)
            empty.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        } else {
            for (index, item) in items.enumerated() {
                let button = NSButton(title: "", target: self, action: #selector(openItem(_:)))
                button.tag = index
                button.isBordered = false
                button.alignment = .left
                button.lineBreakMode = .byTruncatingTail
                button.cell?.wraps = true
                let kind = item.kind == .commit ? "Commit" : (item.kind == .run ? "Run" : "Note")
                let time = DateFormatter.localizedString(from: item.date, dateStyle: .none, timeStyle: .medium)
                let paragraph = NSMutableParagraphStyle()
                paragraph.lineBreakMode = .byTruncatingTail
                let title = NSMutableAttributedString(string: "\(kind) · \(time)  \(item.title)\n", attributes: [.font: Theme.uiFont(12, weight: .semibold), .foregroundColor: Theme.textBase, .paragraphStyle: paragraph])
                title.append(NSAttributedString(string: item.detail, attributes: [.font: Theme.uiFont(11), .foregroundColor: Theme.textMuted, .paragraphStyle: paragraph]))
                button.attributedTitle = title
                button.toolTip = "\(kind) · \(item.title)\n\(item.detail)"
                button.setAccessibilityLabel("Inspect \(kind.lowercased()): \(item.title), \(time). \(item.detail)")
                stack.addArrangedSubview(button)
                button.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
                button.heightAnchor.constraint(equalToConstant: 64).isActive = true
            }
        }
        view = root
    }

    @objc private func openItem(_ sender: NSButton) {
        guard items.indices.contains(sender.tag) else { return }
        onSelect(items[sender.tag])
    }
    @objc private func closeInspector() { onClose?() }
}
