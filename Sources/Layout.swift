import AppKit

struct WorkspaceSizing {
    let compactNavigation: Bool
    let navigationWidth: CGFloat
    let contentInset: CGFloat
    let inspectorWidth: CGFloat

    init(windowWidth: CGFloat, sidebarVisible: Bool, sidebarWidth: CGFloat,
         inspectorVisible: Bool, inspectorWidth: CGFloat) {
        compactNavigation = inspectorVisible && windowWidth < 1180
        navigationWidth = compactNavigation ? 52 : min(320, max(190, sidebarWidth))
        contentInset = sidebarVisible ? navigationWidth : 0
        self.inspectorWidth = min(min(620, max(300, inspectorWidth)), max(300, windowWidth - contentInset - 480))
    }
}

final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

final class InspectorRootView: NSView {
    var onLayout: (() -> Void)?
    override func layout() { super.layout(); onLayout?() }
}

// Pages with a flexible body fill the window. Forms and the overview keep
// their natural height and scroll instead of stretching their rows.
func makePage(in root: NSView, scrolling: Bool = false, inset: CGFloat = 16) -> NSStackView {
    let stack = NSStackView()
    stack.orientation = .vertical
    stack.distribution = .fill
    stack.setHuggingPriority(.defaultLow, for: .horizontal)
    stack.alignment = .leading
    stack.spacing = 12
    stack.translatesAutoresizingMaskIntoConstraints = false
    let container: NSView
    if scrolling {
        let scroll = NSScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        let document = FlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = document
        root.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: root.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            document.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            document.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])
        container = document
    } else {
        container = root
    }
    container.addSubview(stack)
    NSLayoutConstraint.activate([
        stack.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: inset),
        stack.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -inset),
        stack.topAnchor.constraint(equalTo: container.topAnchor, constant: inset),
        stack.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -inset),
    ])
    return stack
}

func pinColumns(in container: NSView, left: NSView, right: NSView,
                leftFraction: CGFloat, minimum: CGFloat, maximum: CGFloat) {
    left.translatesAutoresizingMaskIntoConstraints = false
    right.translatesAutoresizingMaskIntoConstraints = false
    container.addSubview(left)
    container.addSubview(right)
    let preferred = left.widthAnchor.constraint(equalTo: container.widthAnchor, multiplier: leftFraction)
    // Beat content hugging while preserving the window's current-size preference.
    preferred.priority = NSLayoutConstraint.Priority(498)
    let resized = left.widthAnchor.constraint(equalToConstant: 300)
    resized.priority = NSLayoutConstraint.Priority(499)
    let divider = WorkspaceDivider()
    divider.translatesAutoresizingMaskIntoConstraints = false
    container.addSubview(divider)
    divider.onDrag = { delta in
        let upper = max(minimum, min(maximum, container.bounds.width - 240))
        resized.constant = min(upper, max(minimum, left.frame.width + delta))
        resized.isActive = true
    }
    divider.onReset = { resized.isActive = false }
    NSLayoutConstraint.activate([
        left.leadingAnchor.constraint(equalTo: container.leadingAnchor),
        left.topAnchor.constraint(equalTo: container.topAnchor),
        left.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        right.leadingAnchor.constraint(equalTo: left.trailingAnchor, constant: 8),
        right.trailingAnchor.constraint(equalTo: container.trailingAnchor),
        right.topAnchor.constraint(equalTo: container.topAnchor),
        right.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        right.widthAnchor.constraint(greaterThanOrEqualToConstant: 200),
        left.widthAnchor.constraint(greaterThanOrEqualToConstant: minimum),
        left.widthAnchor.constraint(lessThanOrEqualToConstant: maximum),
        preferred,
        divider.centerXAnchor.constraint(equalTo: left.trailingAnchor, constant: 4),
        divider.widthAnchor.constraint(equalToConstant: 6),
        divider.topAnchor.constraint(equalTo: container.topAnchor),
        divider.bottomAnchor.constraint(equalTo: container.bottomAnchor),
    ])
}

// A table document must resize with its clip view before AppKit can resize
// its flexible columns. Merely constraining the scroll view leaves it wider.
final class FittingTableScrollView: NSScrollView {
    override var documentView: NSView? {
        didSet {
            guard let table = documentView as? NSTableView else { return }
            table.style = .plain
            table.usesAlternatingRowBackgroundColors = false
            table.backgroundColor = Theme.base
            table.intercellSpacing = NSSize(width: 0, height: 2)
            table.autoresizingMask = [.width]
            table.columnAutoresizingStyle = .noColumnAutoresizing
            table.rowHeight = 28
        }
    }

    override func layout() {
        super.layout()
        guard let table = documentView as? NSTableView, contentSize.width > 0 else { return }
        let columns = table.tableColumns
        guard !columns.isEmpty else { return }
        table.columnAutoresizingStyle = .noColumnAutoresizing
        let targetWidth = floor(contentSize.width)
        let flexible = columns.filter { $0.resizingMask.contains(.autoresizingMask) }
        let fixed = columns.filter { !$0.resizingMask.contains(.autoresizingMask) }
        let minimum = flexible.reduce(CGFloat(0)) { $0 + $1.minWidth }
        let fixedWidth = fixed.reduce(CGFloat(0)) { $0 + $1.width }
        let remaining = max(0, targetWidth - fixedWidth - minimum)
        let weights = flexible.map { max(1, $0.width - $0.minWidth) }
        let totalWeight = weights.reduce(0, +)
        var assigned = fixedWidth
        for (index, column) in flexible.enumerated() {
            let width = index == flexible.count - 1
                ? max(column.minWidth, targetWidth - assigned)
                : floor(column.minWidth + remaining * weights[index] / max(1, totalWeight))
            if abs(column.width - width) > 0.5 { column.width = min(column.maxWidth, width) }
            assigned += column.width
        }
        if abs(table.frame.width - targetWidth) > 0.5 {
            table.setFrameSize(NSSize(width: targetWidth, height: table.frame.height))
        }
        table.tile()
    }
}

final class WrappingTextScrollView: NSScrollView {
    override func layout() {
        super.layout()
        guard let text = documentView as? NSTextView, contentSize.width > 0 else { return }
        let width = floor(contentSize.width)
        text.isHorizontallyResizable = false
        text.textContainer?.widthTracksTextView = true
        text.minSize = NSSize(width: width, height: 0)
        text.maxSize = NSSize(width: width, height: CGFloat.greatestFiniteMagnitude)
        if abs(text.frame.width - width) > 0.5 {
            text.setFrameSize(NSSize(width: width, height: text.frame.height))
            text.layoutManager?.ensureLayout(for: text.textContainer!)
            text.sizeToFit()
        }
    }
}

final class EmptyStateView: NSView {
    private let heading: NSTextField
    private let detail: NSTextField
    init(title: String, message: String) {
        heading = headerLabel(title, size: 14, weight: .semibold)
        detail = NSTextField(wrappingLabelWithString: message)
        super.init(frame: .zero)
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.distribution = .fill
        stack.spacing = 6
        stack.translatesAutoresizingMaskIntoConstraints = false
        detail.font = Theme.uiFont(12)
        detail.textColor = Theme.textMuted
        detail.alignment = .center
        stack.addArrangedSubview(heading)
        stack.addArrangedSubview(detail)
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            stack.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -20),
            stack.widthAnchor.constraint(lessThanOrEqualToConstant: 360),
        ])
    }
    func set(title: String, message: String) {
        heading.stringValue = title
        detail.stringValue = message
    }
    required init?(coder: NSCoder) { fatalError() }
}
