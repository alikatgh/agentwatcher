import AppKit

// These presentation values contain only source-recorded data. In particular,
// a launch record is not evidence of a child's private tool history or thoughts.
struct AgentWorkDetail {
    struct Section { var title: String; var text: String }
    var title: String
    var subtitle: String
    var sections: [Section]
    var linkedSessionID: String?
    var fullText: String {
        ([title, subtitle] + sections.map { "\($0.title)\n\($0.text)" }).joined(separator: "\n\n")
    }
}

struct AgentWorkTrajectoryItem {
    var id: String
    var title: String
    var summary: String
    var time: Date?
    var isError: Bool
    var searchableText: String
    var isInfrastructure = false
    var detail: () -> AgentWorkDetail
}

enum AgentWorkPresentation {
    static func validSessionID(_ value: String?) -> String? {
        guard let value, value.count <= 160 else { return nil }
        guard UUID(uuidString: value) != nil || value.range(of: #"^(ses_[A-Za-z0-9_-]+|session-[A-Za-z0-9_-]+)$"#, options: .regularExpression) != nil else { return nil }
        return value
    }

    static func pretty(_ raw: String) -> String {
        guard let value = try? JSONSerialization.jsonObject(with: Data(raw.utf8), options: .fragmentsAllowed) else { return raw }
        return json(value)
    }

    static func json(_ value: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed, .withoutEscapingSlashes]) else { return String(describing: value) }
        return String(decoding: data, as: UTF8.self)
    }

    static func encoded<T: Encodable>(_ value: T) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        encoder.dateEncodingStrategy = .iso8601
        encoder.nonConformingFloatEncodingStrategy = .convertToString(positiveInfinity: "Infinity", negativeInfinity: "-Infinity", nan: "NaN")
        guard let data = try? encoder.encode(value) else { return "Could not format the recorded value." }
        return String(decoding: data, as: UTF8.self)
    }

    static func date(_ date: Date?, timeOnly: Bool = false) -> String {
        guard let date else { return "Not recorded" }
        let formatter = DateFormatter()
        formatter.dateStyle = timeOnly ? .none : .medium
        formatter.timeStyle = .medium
        return formatter.string(from: date)
    }

    static func session(_ snapshot: TelemetrySnapshot) -> AgentWorkDetail {
        let children = snapshot.subagents.map { "\($0.title)\nSession: \($0.sessionID ?? "Not recorded")\nStatus: \($0.status ?? "Not recorded")" }.joined(separator: "\n\n")
        let hierarchy = "Current session: \(snapshot.sessionID)\nParent session: \(snapshot.parentSessionID ?? "Not recorded")\n\n" + (snapshot.subagents.isEmpty ? "No subagent launches recorded." : children)
        let notices = ([snapshot.error].compactMap { $0 } + snapshot.warnings).joined(separator: "\n\n")
        let sections: [AgentWorkDetail.Section] = [
                .init(title: "Summary", text: "Agent: \(snapshot.agent ?? "Not recorded")\nModel: \(snapshot.model ?? "Not recorded")\nProvider: \(snapshot.provider ?? "Not recorded")\nTools: \(snapshot.toolEvents.count) recorded\nMessages: \(snapshot.messages.count) recorded\nSubagent launches: \(snapshot.subagents.count) recorded\nWorkspace: \(snapshot.project)"),
                .init(title: "Task", text: snapshot.taskPrompt ?? "Not recorded"),
                .init(title: "Hierarchy", text: hierarchy),
                .init(title: "Source", text: "\(snapshot.source)\nCollected: \(date(snapshot.collectedAt))\n\n" + notices),
                .init(title: "Recorded data", text: encoded(snapshot))
              ]
        return .init(title: snapshot.title.isEmpty ? "Recorded session" : snapshot.title, subtitle: snapshot.sessionID, sections: sections, linkedSessionID: nil)
    }

    static func child(_ child: TelemetrySubagent, snapshot: TelemetrySnapshot) -> AgentWorkDetail {
        .init(title: child.title, subtitle: "Recorded subagent", sections: [
            .init(title: "Summary", text: "Agent: \(child.agent ?? "Not recorded")\nStatus: \(child.status ?? "Not recorded")\nLaunch ID: \(child.id)\n\nThis is the recorded launch. Open the linked session to read its own tool and message history."),
            .init(title: "Task", text: child.prompt ?? "Not recorded"),
            .init(title: "Hierarchy", text: "Launched from: \(snapshot.sessionID)\nChild session: \(child.sessionID ?? "Not recorded")"),
            .init(title: "Recorded data", text: pretty(child.rawJSON))
        ], linkedSessionID: validSessionID(child.sessionID))
    }

    static func parent(_ snapshot: TelemetrySnapshot) -> AgentWorkDetail {
        .init(title: "Recorded parent", subtitle: snapshot.parentSessionID ?? "Not recorded", sections: [
            .init(title: "Hierarchy", text: "Parent session: \(snapshot.parentSessionID ?? "Not recorded")\nCurrent session: \(snapshot.sessionID)\n\nThe current source records this parent link. Open the parent to read its recorded task and history."),
            .init(title: "Recorded data", text: pretty(snapshot.rawInfoJSON))
        ], linkedSessionID: validSessionID(snapshot.parentSessionID))
    }

    static func tool(_ event: TelemetryToolEvent, snapshot: TelemetrySnapshot) -> AgentWorkDetail {
        let raw = (try? JSONSerialization.jsonObject(with: Data(event.rawJSON.utf8))) as? [String: Any] ?? [:]
        func field(_ key: String) -> String { raw[key].map { $0 is NSNull ? "Not recorded" : json($0) } ?? "Not recorded" }
        let child = snapshot.subagents.first { $0.id == event.id }
        return .init(title: event.name, subtitle: event.status, sections: [
            .init(title: "Summary", text: "Tool: \(event.name)\nStatus: \(event.status)\nCall ID: \(event.id)\nError: \(event.error ?? "None reported")"),
            .init(title: "Hierarchy", text: "Session: \(snapshot.sessionID)\nParent: \(snapshot.parentSessionID ?? "Not recorded")\nTurn: \(field("turn"))\nStep: \(field("step"))\nAgent: \(field("agent"))\nChild session: \(child?.sessionID ?? raw["subagentSessionID"] as? String ?? "Not recorded")"),
            .init(title: "Payload", text: pretty(event.inputJSON)),
            .init(title: "Result", text: event.output ?? "Not recorded"),
            .init(title: "Schema", text: field("toolSchema")),
            .init(title: "Timing", text: "Started: \(date(event.startedAt))\nCompleted: \(date(event.completedAt))\nDuration: \(event.duration.map { String(format: "%.3f s", $0) } ?? "Not recorded")"),
            .init(title: "Recorded data", text: pretty(event.rawJSON))
        ], linkedSessionID: validSessionID(child?.sessionID ?? raw["subagentSessionID"] as? String))
    }

    static func trajectory(_ snapshot: TelemetrySnapshot) -> [AgentWorkTrajectoryItem] {
        var rows = snapshot.messages.enumerated().map { index, message in
            AgentWorkTrajectoryItem(id: "message:\(index):\(message.id)", title: message.type.capitalized + " message", summary: message.text,
                                    time: message.timestamp, isError: false,
                                    searchableText: [message.id, message.type, message.text, message.rawJSON].joined(separator: "\n"),
                                    isInfrastructure: ["system", "developer"].contains(message.type.lowercased()), detail: {
                .init(title: message.type.capitalized + " message", subtitle: date(message.timestamp), sections: [
                    .init(title: "Message", text: message.text.isEmpty ? "No text recorded. Inspect Recorded data for the retained fields." : message.text),
                    .init(title: "Hierarchy", text: "Session: \(snapshot.sessionID)\nMessage ID: \(message.id)\nRole: \(message.type)"),
                    .init(title: "Recorded data", text: pretty(message.rawJSON))
                ], linkedSessionID: nil)
            })
        }
        rows += snapshot.toolEvents.enumerated().map { index, event in
            AgentWorkTrajectoryItem(id: "tool:\(index):\(event.id)", title: event.name, summary: "\(event.status) · \(event.error ?? event.inputJSON)",
                                    time: event.startedAt ?? event.completedAt,
                                    isError: event.error != nil || ["error", "failed"].contains(event.status.lowercased()),
                                    searchableText: [event.id, event.name, event.status, event.inputJSON, event.output ?? "", event.error ?? "", event.rawJSON].joined(separator: "\n"),
                                    detail: { tool(event, snapshot: snapshot) })
        }
        return rows.enumerated().sorted { left, right in
            if let a = left.element.time, let b = right.element.time, a != b { return a < b }
            if left.element.time != nil && right.element.time == nil { return true }
            if left.element.time == nil && right.element.time != nil { return false }
            return left.offset < right.offset
        }.map(\.element)
    }

    static func context(_ snapshot: TelemetrySnapshot) -> AgentWorkDetail {
        var sections: [AgentWorkDetail.Section] = [.init(title: "Coverage", text: "Only context retained by the source is shown. Missing context and unavailable private reasoning cannot be reconstructed.\n\n\(snapshot.context.count) context records · \(snapshot.runtime.count) runtime records")]
        sections += snapshot.context.map { .init(title: $0.title, text: "\($0.value)\n\nRecorded data\n\(pretty($0.detailJSON))") }
        sections += snapshot.runtime.map { .init(title: "Runtime · \($0.title)", text: "\($0.value)\n\nRecorded data\n\(pretty($0.detailJSON))") }
        sections.append(.init(title: "Recorded data", text: encoded(snapshot.context) + "\n\nRuntime\n" + encoded(snapshot.runtime)))
        return .init(title: "Recorded context", subtitle: snapshot.sessionID, sections: sections, linkedSessionID: nil)
    }
}

// Embedded inspector: its width is owned by the application's inspector host.
// No preferredContentSize or minimum width can resize the containing window.
final class AgentWorkDetailController: NSViewController {
    var onOpenSession: ((String) -> Void)?
    private let heading = headerLabel("", size: 13, weight: .semibold)
    private let subtitle = headerLabel("", size: 11, color: Theme.textMuted)
    private let sectionPicker = NSPopUpButton(frame: .zero, pullsDown: false)
    private let openButton = NSButton(title: "Open session", target: nil, action: nil)
    private let text = NSTextView()
    private var detail: AgentWorkDetail

    init(_ detail: AgentWorkDetail) { self.detail = detail; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        view = NSView()
        heading.maximumNumberOfLines = 2
        heading.lineBreakMode = .byTruncatingTail
        subtitle.lineBreakMode = .byTruncatingMiddle
        let copy = NSButton(title: "Copy all", target: self, action: #selector(copyAll))
        for button in [copy, openButton] { button.controlSize = .small; button.bezelStyle = .rounded }
        openButton.target = self; openButton.action = #selector(openSession)
        let actions = NSStackView(views: [openButton, NSView(), copy])
        actions.orientation = .horizontal
        actions.arrangedSubviews[1].setContentHuggingPriority(.defaultLow, for: .horizontal)
        sectionPicker.controlSize = .small
        sectionPicker.target = self; sectionPicker.action = #selector(sectionChanged)
        sectionPicker.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        sectionPicker.setAccessibilityLabel("Recorded detail section")
        let scroll = WrappingTextScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        text.isEditable = false; text.isSelectable = true; text.isRichText = false
        text.usesFindBar = true
        text.font = Theme.codeFont(12)
        text.textColor = Theme.textBase; text.backgroundColor = Theme.deep
        text.textContainerInset = NSSize(width: 12, height: 12)
        text.isVerticallyResizable = true; text.isHorizontallyResizable = false
        text.autoresizingMask = [.width]
        text.textContainer?.widthTracksTextView = true
        scroll.documentView = text
        for child in [heading, subtitle, actions, sectionPicker, scroll] { child.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(child) }
        NSLayoutConstraint.activate([
            heading.topAnchor.constraint(equalTo: view.topAnchor, constant: 16), heading.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 14), heading.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -14), heading.heightAnchor.constraint(lessThanOrEqualToConstant: 38),
            subtitle.topAnchor.constraint(equalTo: heading.bottomAnchor, constant: 4), subtitle.leadingAnchor.constraint(equalTo: heading.leadingAnchor), subtitle.trailingAnchor.constraint(equalTo: heading.trailingAnchor), subtitle.heightAnchor.constraint(equalToConstant: 16),
            actions.topAnchor.constraint(equalTo: subtitle.bottomAnchor, constant: 10), actions.leadingAnchor.constraint(equalTo: heading.leadingAnchor), actions.trailingAnchor.constraint(equalTo: heading.trailingAnchor), actions.heightAnchor.constraint(equalToConstant: 24),
            sectionPicker.topAnchor.constraint(equalTo: actions.bottomAnchor, constant: 8), sectionPicker.leadingAnchor.constraint(equalTo: heading.leadingAnchor), sectionPicker.trailingAnchor.constraint(equalTo: heading.trailingAnchor), sectionPicker.heightAnchor.constraint(equalToConstant: 24),
            scroll.topAnchor.constraint(equalTo: sectionPicker.bottomAnchor, constant: 10), scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor), scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor), scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor)
        ])
        update(detail)
    }

    func update(_ detail: AgentWorkDetail) {
        self.detail = detail
        guard isViewLoaded else { return }
        let selected = sectionPicker.indexOfSelectedItem
        heading.stringValue = detail.title; heading.toolTip = detail.title
        subtitle.stringValue = detail.subtitle; subtitle.toolTip = detail.subtitle
        sectionPicker.removeAllItems()
        sectionPicker.addItem(withTitle: "All sections")
        detail.sections.forEach { sectionPicker.addItem(withTitle: $0.title) }
        sectionPicker.selectItem(at: max(0, min(selected, detail.sections.count)))
        openButton.isHidden = detail.linkedSessionID == nil
        openButton.toolTip = detail.linkedSessionID
        renderSection()
    }

    private func renderSection() {
        let index = sectionPicker.indexOfSelectedItem - 1
        text.string = detail.sections.indices.contains(index) ? "\(detail.sections[index].title)\n\n\(detail.sections[index].text)" : detail.fullText
        text.setSelectedRange(NSRange(location: 0, length: 0))
        text.scrollToBeginningOfDocument(nil)
    }
    @objc private func sectionChanged() { renderSection() }
    @objc private func copyAll() { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(detail.fullText, forType: .string) }
    @objc private func openSession() { if let id = detail.linkedSessionID { onOpenSession?(id) } }
}

private struct AgentWorkNode {
    var key: String
    var role: String
    var task: String
    var status: String
    var count: String
    var color: NSColor
}

private final class AgentWorkNodeButton: NSButton {
    var node: AgentWorkNode { didSet { refreshAccessibility(); needsDisplay = true } }
    var selected = false { didSet { needsDisplay = true } }
    var onOpen: (() -> Void)?
    private var hover = false
    private var tracking: NSTrackingArea?
    init(_ node: AgentWorkNode) {
        self.node = node
        super.init(frame: .zero)
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; action = #selector(open)
        refreshAccessibility()
    }
    private func refreshAccessibility() {
        setAccessibilityLabel("\(node.role): \(node.task). \(node.status). \(node.count)")
        setAccessibilityHelp("Inspect recorded agent; open its linked session when available")
        toolTip = "\(node.task)\n\(node.status)\n\(node.count)"
    }
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        let outline = NSBezierPath(rect: bounds.insetBy(dx: 0.5, dy: 0.5))
        (selected ? Theme.layer03 : hover ? Theme.layer02 : Theme.layer01).setFill(); outline.fill()
        (selected ? Theme.borderStrong : Theme.borderMuted).setStroke(); outline.lineWidth = 1; outline.stroke()
        func line(_ value: String, rect: NSRect, font: NSFont, color: NSColor) {
            let paragraph = NSMutableParagraphStyle()
            paragraph.lineBreakMode = .byTruncatingTail
            (value as NSString).draw(with: rect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine], attributes: [.font: font, .foregroundColor: color, .paragraphStyle: paragraph])
        }
        line(node.role, rect: NSRect(x: 9, y: 5, width: bounds.width - 18, height: 14), font: Theme.uiFont(11, weight: .semibold), color: node.color)
        line(node.task, rect: NSRect(x: 9, y: 21, width: bounds.width - 18, height: 16), font: Theme.uiFont(12, weight: .medium), color: Theme.textBase)
        line("\(node.status) · \(node.count)", rect: NSRect(x: 9, y: 38, width: bounds.width - 18, height: 14), font: Theme.uiFont(11), color: Theme.textMuted)
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.activeInKeyWindow, .mouseEnteredAndExited, .inVisibleRect], owner: self)
        addTrackingArea(area); tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }
    @objc private func open() { onOpen?() }
    required init?(coder: NSCoder) { fatalError() }
}

// Only nodes inside the scroll viewport have native button views. Every child
// remains reachable on the horizontal canvas, including large recorded trees.
private final class AgentWorkGraph: NSView {
    var onSelect: ((String) -> Void)?
    private var snapshot: TelemetrySnapshot?
    private var buttons: [String: AgentWorkNodeButton] = [:]
    private var observation: NSObjectProtocol?
    var selectedKey = "current" { didSet { buttons.forEach { $0.value.selected = $0.key == selectedKey } } }
    private let childPitch: CGFloat = 250
    private let childWidth: CGFloat = 236
    private var viewportRect: NSRect {
        guard let clip = enclosingScrollView?.contentView else { return bounds }
        return convert(clip.bounds, from: clip)
    }
    var naturalWidth: CGFloat { max(rootX + 276, 32 + CGFloat(snapshot?.subagents.count ?? 0) * childPitch - (snapshot?.subagents.isEmpty == false ? 14 : 0)) }
    var naturalHeight: CGFloat {
        guard let snapshot else { return 48 }
        return snapshot.subagents.isEmpty ? 76 : 148
    }
    private var rootX: CGFloat { snapshot?.parentSessionID == nil ? 16 : 232 }
    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: NSView.noIntrinsicMetric) }

    func update(_ snapshot: TelemetrySnapshot?) { self.snapshot = snapshot; synchronizeButtons(); needsDisplay = true }
    override func viewDidMoveToSuperview() {
        super.viewDidMoveToSuperview()
        if let observation { NotificationCenter.default.removeObserver(observation) }
        if let clip = enclosingScrollView?.contentView {
            clip.postsBoundsChangedNotifications = true
            observation = NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: clip, queue: .main) { [weak self] _ in self?.synchronizeButtons(); self?.needsDisplay = true }
        }
    }
    deinit { if let observation { NotificationCenter.default.removeObserver(observation) } }
    override func layout() { super.layout(); synchronizeButtons() }
    func synchronizeViewport() { synchronizeButtons(); needsDisplay = true }

    private func childIndices(in rect: NSRect) -> Range<Int> {
        let total = snapshot?.subagents.count ?? 0
        let first = max(0, Int(floor((rect.minX - 16) / childPitch)))
        let end = min(total, max(first, Int(ceil((rect.maxX - 16) / childPitch)) + 1))
        return min(first, total)..<end
    }
    private func synchronizeButtons() {
        guard let snapshot else { buttons.values.forEach { $0.removeFromSuperview() }; buttons.removeAll(); return }
        let visible = viewportRect.insetBy(dx: -32, dy: -16)
        var nodes: [(AgentWorkNode, NSRect)] = []
        let rootRect = NSRect(x: rootX, y: 6, width: 260, height: 56)
        if rootRect.intersects(visible) {
            let status = snapshot.error == nil ? "Recorded session" : "Collection issue"
            nodes.append((.init(key: "current", role: "CURRENT AGENT · \(snapshot.agent ?? "type not recorded")", task: snapshot.taskPrompt ?? snapshot.title,
                                status: status, count: "\(snapshot.toolEvents.count) tools", color: Theme.accent), rootRect))
        }
        if let parent = snapshot.parentSessionID {
            let rect = NSRect(x: 16, y: 6, width: 184, height: 56)
            if rect.intersects(visible) { nodes.append((.init(key: "parent", role: "RECORDED PARENT", task: parent, status: "Open parent", count: "tools not loaded", color: Theme.textMuted), rect)) }
        }
        for index in childIndices(in: visible) {
            let child = snapshot.subagents[index]
            let error = ["error", "failed"].contains(child.status?.lowercased() ?? "")
            nodes.append((.init(key: "child:\(child.id)", role: "SUBAGENT \(index + 1) · \(child.agent ?? "type not recorded")", task: child.title.isEmpty ? child.prompt ?? "Recorded subagent" : child.title,
                                status: child.status ?? "Status not recorded", count: "tools not loaded", color: error ? Theme.danger : Theme.success), NSRect(x: 16 + CGFloat(index) * childPitch, y: 84, width: childWidth, height: 56)))
        }
        let keys = Set(nodes.map { $0.0.key })
        for key in Array(buttons.keys) where !keys.contains(key) { buttons.removeValue(forKey: key)?.removeFromSuperview() }
        for (node, rect) in nodes {
            let button: AgentWorkNodeButton
            if let existing = buttons[node.key] { button = existing; button.node = node }
            else {
                button = AgentWorkNodeButton(node); buttons[node.key] = button; addSubview(button)
                button.onOpen = { [weak self] in self?.selectedKey = node.key; self?.onSelect?(node.key) }
            }
            button.frame = rect; button.selected = node.key == selectedKey
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        Theme.deep.setFill(); dirtyRect.fill()
        guard let snapshot else { return }
        Theme.borderStrong.setStroke()
        func path(_ points: [NSPoint]) {
            guard let first = points.first else { return }
            let line = NSBezierPath(); line.move(to: first); points.dropFirst().forEach { line.line(to: $0) }; line.lineWidth = 1; line.stroke()
        }
        func arrow(_ end: NSPoint, horizontal: Bool = false) {
            let triangle = NSBezierPath(); triangle.move(to: end)
            triangle.line(to: horizontal ? NSPoint(x: end.x - 6, y: end.y - 3.5) : NSPoint(x: end.x - 3.5, y: end.y - 6))
            triangle.line(to: horizontal ? NSPoint(x: end.x - 6, y: end.y + 3.5) : NSPoint(x: end.x + 3.5, y: end.y - 6))
            triangle.close(); Theme.textFaint.setFill(); triangle.fill()
        }
        if snapshot.parentSessionID != nil { path([NSPoint(x: 200, y: 34), NSPoint(x: rootX - 3, y: 34)]); arrow(NSPoint(x: rootX - 3, y: 34), horizontal: true) }
        if !snapshot.subagents.isEmpty {
            let rootCenter = rootX + 130
            let first = 16 + childWidth / 2
            let last = first + CGFloat(snapshot.subagents.count - 1) * childPitch
            path([NSPoint(x: rootCenter, y: 62), NSPoint(x: rootCenter, y: 73)])
            path([NSPoint(x: min(first, rootCenter), y: 73), NSPoint(x: max(last, rootCenter), y: 73)])
            for index in childIndices(in: dirtyRect) {
                let x = first + CGFloat(index) * childPitch
                path([NSPoint(x: x, y: 73), NSPoint(x: x, y: 82)]); arrow(NSPoint(x: x, y: 82))
            }
        }
    }
}

private final class AgentWorkGraphScroll: NSScrollView {
    override func reflectScrolledClipView(_ clipView: NSClipView) {
        super.reflectScrolledClipView(clipView)
        (documentView as? AgentWorkGraph)?.synchronizeViewport()
    }
    override func layout() {
        super.layout()
        guard let graph = documentView as? AgentWorkGraph else { return }
        let size = NSSize(width: max(contentSize.width, graph.naturalWidth), height: max(graph.naturalHeight, contentSize.height))
        if graph.frame.size != size { graph.setFrameSize(size) }
        graph.needsLayout = true
    }
}

final class AgentWorkController: NSViewController, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    var onOpenSession: ((String) -> Void)?
    var onInspectDetails: ((NSViewController) -> Void)?
    private var snapshot: TelemetrySnapshot?
    private let sessionLabel = headerLabel("Agent work", size: 13, weight: .semibold)
    private let taskLabel = headerLabel("Waiting for a recorded session…", size: 12, color: Theme.textMuted)
    private let sourceLabel = headerLabel("", size: 11, color: Theme.textFaint)
    private let countLabel = headerLabel("", size: 11, color: Theme.textMuted)
    private let graphHint = headerLabel("Recorded hierarchy", size: 12, weight: .semibold)
    private let contextButton = NSButton(title: "Context", target: nil, action: nil)
    private let sessionButton = NSButton(title: "Session details", target: nil, action: nil)
    private let graph = AgentWorkGraph()
    private let graphScroll = AgentWorkGraphScroll()
    private let search = NSSearchField()
    private let allRecords = NSButton(checkboxWithTitle: "All records", target: nil, action: nil)
    private let table = NSTableView()
    private let emptyLabel = headerLabel("No recorded trajectory", size: 12, color: Theme.textMuted)
    private var allRows: [AgentWorkTrajectoryItem] = []
    private var visibleRows: [AgentWorkTrajectoryItem] = []
    private var selectedRecord: String?
    private var selectedDetailKey: String?
    private weak var activeInspector: AgentWorkDetailController?
    private var reloading = false
    private var graphHeight: NSLayoutConstraint!
    private var taskHeight: NSLayoutConstraint!

    override func loadView() {
        view = NSView()
        sessionLabel.lineBreakMode = .byTruncatingTail
        sessionLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        taskLabel.maximumNumberOfLines = 1
        taskLabel.lineBreakMode = .byTruncatingTail
        sourceLabel.lineBreakMode = .byTruncatingMiddle
        for label in [taskLabel, sourceLabel, countLabel, graphHint] { label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal) }
        graphHint.lineBreakMode = .byTruncatingTail
        for button in [contextButton, sessionButton] { button.controlSize = .small; button.bezelStyle = .rounded; button.font = Theme.uiFont(12) }
        contextButton.target = self; contextButton.action = #selector(openContext)
        sessionButton.target = self; sessionButton.action = #selector(openSessionDetails)
        let header = NSStackView(views: [sessionLabel, NSView(), sessionButton, contextButton])
        header.orientation = .horizontal; header.spacing = 8
        header.arrangedSubviews[1].setContentHuggingPriority(.defaultLow, for: .horizontal)
        graphScroll.hasHorizontalScroller = true; graphScroll.hasVerticalScroller = false
        graphScroll.autohidesScrollers = true; graphScroll.drawsBackground = false
        graphScroll.documentView = graph
        graph.onSelect = { [weak self] key in self?.selectNode(key) }
        search.placeholderString = "Search all tools, messages, payloads and results"
        search.controlSize = .small
        search.font = Theme.uiFont(12)
        search.delegate = self
        search.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        search.setAccessibilityLabel("Search the full recorded trajectory")
        allRecords.controlSize = .small
        allRecords.font = Theme.uiFont(12)
        allRecords.state = .off
        allRecords.target = self; allRecords.action = #selector(recordScopeChanged)
        allRecords.toolTip = "Include system and developer messages. Search always covers every recorded message and tool event."
        let trajectoryHeading = headerLabel("Trajectory", size: 13, weight: .semibold)
        let trajectoryHeader = NSStackView(views: [trajectoryHeading, countLabel, NSView(), allRecords])
        trajectoryHeader.orientation = .horizontal; trajectoryHeader.spacing = 8
        trajectoryHeader.arrangedSubviews[2].setContentHuggingPriority(.defaultLow, for: .horizontal)
        let list = FittingTableScrollView()
        list.hasVerticalScroller = true; list.drawsBackground = false
        let time = NSTableColumn(identifier: .init("time")); time.title = "Time"; time.width = 88; time.minWidth = 88; time.maxWidth = 88; time.resizingMask = []
        let action = NSTableColumn(identifier: .init("action")); action.title = "Agent action"; action.width = 160; action.minWidth = 95; action.resizingMask = .autoresizingMask
        let summary = NSTableColumn(identifier: .init("summary")); summary.title = "Recorded details"; summary.width = 420; summary.minWidth = 100; summary.resizingMask = .autoresizingMask
        [time, action, summary].forEach { table.addTableColumn($0) }
        table.delegate = self; table.dataSource = self
        table.allowsEmptySelection = true; table.allowsMultipleSelection = false
        table.backgroundColor = Theme.base
        table.usesAlternatingRowBackgroundColors = false
        table.target = self; table.action = #selector(rowClicked)
        table.setAccessibilityLabel("Recorded agent trajectory")
        list.documentView = table
        table.rowHeight = 28
        table.intercellSpacing = .zero
        emptyLabel.alignment = .center
        for child in [header, taskLabel, sourceLabel, graphHint, graphScroll, trajectoryHeader, search, list, emptyLabel] { child.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(child) }
        graphHeight = graphScroll.heightAnchor.constraint(equalToConstant: graph.naturalHeight + 14)
        taskHeight = taskLabel.heightAnchor.constraint(equalToConstant: 18)
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: view.topAnchor, constant: 12), header.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 16), header.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -16), header.heightAnchor.constraint(equalToConstant: 26),
            taskLabel.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 4), taskLabel.leadingAnchor.constraint(equalTo: header.leadingAnchor), taskLabel.trailingAnchor.constraint(equalTo: header.trailingAnchor), taskHeight,
            sourceLabel.topAnchor.constraint(equalTo: taskLabel.bottomAnchor, constant: 2), sourceLabel.leadingAnchor.constraint(equalTo: header.leadingAnchor), sourceLabel.trailingAnchor.constraint(equalTo: header.trailingAnchor), sourceLabel.heightAnchor.constraint(equalToConstant: 15),
            graphHint.topAnchor.constraint(equalTo: sourceLabel.bottomAnchor, constant: 8), graphHint.leadingAnchor.constraint(equalTo: header.leadingAnchor), graphHint.trailingAnchor.constraint(equalTo: header.trailingAnchor), graphHint.heightAnchor.constraint(equalToConstant: 16),
            graphScroll.topAnchor.constraint(equalTo: graphHint.bottomAnchor, constant: 4), graphScroll.leadingAnchor.constraint(equalTo: view.leadingAnchor), graphScroll.trailingAnchor.constraint(equalTo: view.trailingAnchor), graphHeight,
            trajectoryHeader.topAnchor.constraint(equalTo: graphScroll.bottomAnchor, constant: 8), trajectoryHeader.leadingAnchor.constraint(equalTo: header.leadingAnchor), trajectoryHeader.trailingAnchor.constraint(equalTo: header.trailingAnchor), trajectoryHeader.heightAnchor.constraint(equalToConstant: 24),
            search.topAnchor.constraint(equalTo: trajectoryHeader.bottomAnchor, constant: 4), search.leadingAnchor.constraint(equalTo: header.leadingAnchor), search.trailingAnchor.constraint(equalTo: header.trailingAnchor), search.heightAnchor.constraint(equalToConstant: 24),
            list.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 6), list.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 10), list.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -10), list.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -10),
            emptyLabel.centerXAnchor.constraint(equalTo: list.centerXAnchor), emptyLabel.centerYAnchor.constraint(equalTo: list.centerYAnchor), emptyLabel.leadingAnchor.constraint(greaterThanOrEqualTo: list.leadingAnchor, constant: 10), emptyLabel.trailingAnchor.constraint(lessThanOrEqualTo: list.trailingAnchor, constant: -10)
        ])
        if let snapshot { update(snapshot) } else { showLoading(taskLabel.stringValue) }
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        if let snapshot {
            graphHint.stringValue = "Recorded hierarchy · \(snapshot.subagents.count) subagent launches" + (graph.naturalWidth > graphScroll.contentSize.width + 1 ? " · Scroll to browse →" : "")
        }
    }

    func update(_ snapshot: TelemetrySnapshot) {
        let changedSession = self.snapshot?.sessionID != snapshot.sessionID
        self.snapshot = snapshot
        guard isViewLoaded else { return }
        if changedSession { selectedRecord = nil; selectedDetailKey = "current"; graph.selectedKey = "current"; graphScroll.contentView.scroll(to: .zero) }
        sessionLabel.stringValue = snapshot.title.isEmpty ? "Recorded session" : snapshot.title
        sessionLabel.toolTip = snapshot.title
        let task = snapshot.error ?? snapshot.taskPrompt ?? "Task prompt not recorded"
        let repeatsTitle = snapshot.error == nil && task.trimmingCharacters(in: .whitespacesAndNewlines) == sessionLabel.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        taskLabel.stringValue = repeatsTitle ? "" : task
        taskLabel.isHidden = repeatsTitle
        taskHeight.constant = repeatsTitle ? 0 : 18
        taskLabel.textColor = snapshot.error == nil ? Theme.textMuted : Theme.warning
        taskLabel.toolTip = task
        sourceLabel.stringValue = "\(snapshot.source) · \(snapshot.sessionID) · Collected \(AgentWorkPresentation.date(snapshot.collectedAt, timeOnly: true))"
        sourceLabel.toolTip = sourceLabel.stringValue + "\n" + snapshot.warnings.joined(separator: "\n")
        sourceLabel.textColor = snapshot.error == nil ? Theme.textFaint : Theme.warning
        graphHint.stringValue = "Recorded hierarchy · \(snapshot.subagents.count) subagent launches" + (snapshot.subagents.count > 2 ? " · Scroll to browse →" : "")
        graphHint.toolTip = snapshot.subagents.isEmpty ? "No subagent launches recorded. This source has no child-agent launch records." : "Select an agent to inspect its recorded details. Scroll horizontally to reach every subagent."
        graph.update(snapshot); graphScroll.needsLayout = true
        graphHeight.constant = graph.naturalHeight + 14
        allRows = AgentWorkPresentation.trajectory(snapshot)
        contextButton.isEnabled = true; sessionButton.isEnabled = true; search.isEnabled = true; allRecords.isEnabled = true
        reloadRows()
        if let inspector = activeInspector, let detail = selectedDetail() { inspector.update(detail) }
    }

    func showLoading(_ text: String) { showState(text, error: false) }
    func showError(_ text: String) { showState(text, error: true) }
    private func showState(_ text: String, error: Bool) {
        guard isViewLoaded else { taskLabel.stringValue = text; return }
        snapshot = nil; allRows = []; visibleRows = []; selectedRecord = nil; selectedDetailKey = nil
        sessionLabel.stringValue = error ? "Recorded activity unavailable" : "Agent work"
        taskLabel.stringValue = text; taskLabel.toolTip = text
        taskLabel.isHidden = false; taskHeight.constant = 18
        taskLabel.textColor = error ? Theme.warning : Theme.textMuted
        sourceLabel.stringValue = "Read-only recorded session data"
        graphHint.stringValue = "Recorded hierarchy"
        graphHint.toolTip = nil
        graph.update(nil); graphScroll.needsLayout = true
        graphHeight.constant = graph.naturalHeight + 14
        contextButton.isEnabled = false; sessionButton.isEnabled = false; search.isEnabled = false; allRecords.isEnabled = false
        table.reloadData(); countLabel.stringValue = ""
        emptyLabel.stringValue = error ? "Recorded activity could not be loaded." : "Loading recorded activity…"
        emptyLabel.isHidden = false
    }

    func controlTextDidChange(_ obj: Notification) { reloadRows() }
    @objc private func recordScopeChanged() { reloadRows() }
    private func reloadRows() {
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        visibleRows = allRows.filter { item in
            if !query.isEmpty { return item.searchableText.localizedCaseInsensitiveContains(query) }
            return allRecords.state == .on || !item.isInfrastructure
        }
        countLabel.stringValue = "\(visibleRows.count) / \(allRows.count)"
        countLabel.toolTip = !query.isEmpty ? "Search covers all \(allRows.count) recorded events." : allRecords.state == .on ? "Showing every recorded tool event and message." : "\(allRows.count - visibleRows.count) system or developer messages hidden. Enable All records to include them."
        reloading = true; table.reloadData()
        if let selectedRecord, let index = visibleRows.firstIndex(where: { $0.id == selectedRecord }) { table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false) }
        else { table.deselectAll(nil) }
        reloading = false
        emptyLabel.isHidden = !visibleRows.isEmpty
        emptyLabel.stringValue = !query.isEmpty ? "No recorded events match this search." : allRows.isEmpty ? "No tool events or messages recorded." : "Enable All records to view system and developer messages."
    }

    func numberOfRows(in tableView: NSTableView) -> Int { visibleRows.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard visibleRows.indices.contains(row) else { return nil }
        let identifier = tableColumn?.identifier ?? .init("action")
        let cell = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView ?? NSTableCellView()
        if cell.textField == nil {
            let label = headerLabel("", size: 12)
            label.lineBreakMode = .byTruncatingTail
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            label.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(label); cell.textField = label
            NSLayoutConstraint.activate([label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6), label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6), label.centerYAnchor.constraint(equalTo: cell.centerYAnchor)])
        }
        cell.identifier = identifier
        let item = visibleRows[row]
        cell.textField?.stringValue = identifier.rawValue == "time" ? item.time.map { AgentWorkPresentation.date($0, timeOnly: true) } ?? "Not recorded" : identifier.rawValue == "action" ? item.title : item.summary.replacingOccurrences(of: "\n", with: " ")
        cell.textField?.textColor = item.isError ? Theme.danger : identifier.rawValue == "action" ? Theme.textBase : Theme.textMuted
        cell.toolTip = "\(item.title)\n\(item.summary)"
        return cell
    }
    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !reloading, visibleRows.indices.contains(table.selectedRow) else { return }
        inspectRow(table.selectedRow)
    }
    @objc private func rowClicked() {
        guard visibleRows.indices.contains(table.clickedRow) else { return }
        inspectRow(table.clickedRow)
    }
    private func inspectRow(_ index: Int) {
        let item = visibleRows[index]
        selectedRecord = item.id; selectedDetailKey = "trajectory"
        present(item.detail())
    }
    private func selectedDetail() -> AgentWorkDetail? {
        guard let snapshot else { return nil }
        if selectedDetailKey == "trajectory" { return allRows.first { $0.id == selectedRecord }?.detail() }
        if selectedDetailKey == "context" { return AgentWorkPresentation.context(snapshot) }
        if selectedDetailKey == "parent" { return AgentWorkPresentation.parent(snapshot) }
        if let key = selectedDetailKey, key.hasPrefix("child:"), let child = snapshot.subagents.first(where: { $0.id == String(key.dropFirst(6)) }) { return AgentWorkPresentation.child(child, snapshot: snapshot) }
        return AgentWorkPresentation.session(snapshot)
    }
    private func selectNode(_ key: String) {
        selectedDetailKey = key
        guard let detail = selectedDetail() else { return }
        present(detail)
        if key != "current", let linkedID = detail.linkedSessionID { onOpenSession?(linkedID) }
    }
    private func present(_ detail: AgentWorkDetail) {
        let inspector = AgentWorkDetailController(detail)
        inspector.onOpenSession = { [weak self] id in self?.onOpenSession?(id) }
        activeInspector = inspector
        onInspectDetails?(inspector)
    }
    @objc private func openContext() { selectedDetailKey = "context"; if let detail = selectedDetail() { present(detail) } }
    @objc private func openSessionDetails() { selectedDetailKey = "current"; graph.selectedKey = "current"; if let detail = selectedDetail() { present(detail) } }
}
