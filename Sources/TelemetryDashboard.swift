import AppKit

private struct TelemetrySummaryRow {
    var id: String
    var title: String
    var value: String
    var detail: () -> String
}

private struct TelemetrySummarySection {
    var id: String
    var title: String
    var rows: [TelemetrySummaryRow]
}

private func telemetryJSON<T: Encodable>(_ value: T) -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    encoder.dateEncodingStrategy = .iso8601
    encoder.nonConformingFloatEncodingStrategy = .convertToString(positiveInfinity: "Infinity", negativeInfinity: "-Infinity", nan: "NaN")
    do { return String(decoding: try encoder.encode(value), as: UTF8.self) }
    catch { return "Could not format recorded data: \(error)" }
}

private func telemetryPretty(_ raw: String) -> String {
    guard let data = raw.data(using: .utf8) else { return raw }
    do {
        let object = try JSONSerialization.jsonObject(with: data, options: .fragmentsAllowed)
        let pretty = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed, .withoutEscapingSlashes])
        return String(decoding: pretty, as: UTF8.self)
    } catch { return raw }
}

private func telemetryDate(_ date: Date?, timeOnly: Bool = false) -> String {
    guard let date else { return "Not recorded" }
    let formatter = DateFormatter()
    formatter.dateStyle = timeOnly ? .none : .medium
    formatter.timeStyle = .medium
    return formatter.string(from: date)
}

private enum TelemetryPresentation {
    static func sections(_ snapshot: TelemetrySnapshot?) -> [TelemetrySummarySection] {
        let unknown = snapshot == nil ? "Loading…" : "Not recorded"
        let missing = "This value was not included in the recorded session data."
        func info(_ title: String, _ value: String?, _ detail: @autoclosure @escaping () -> String? = nil) -> TelemetrySummaryRow {
            TelemetrySummaryRow(id: title, title: title, value: value?.isEmpty == false ? value! : unknown,
                                detail: { detail() ?? value ?? missing })
        }
        func setting(_ title: String) -> TelemetrySummaryRow {
            let records = (snapshot?.runtime ?? []) + (snapshot?.context ?? [])
            let stems = ["Permissions": "permission", "Plugins": "plugin", "Sandbox": "sandbox", "Configuration": "request header"]
            let record = records.last { $0.title.caseInsensitiveCompare(title) == .orderedSame }
                ?? stems[title].flatMap { stem in records.last { $0.title.lowercased().contains(stem) } }
            return info(title, record?.value, record.map { "\($0.title)\n\n\($0.value)\n\nRecorded data\n\(telemetryPretty($0.detailJSON))" })
        }
        func count(_ number: Int) -> String { number == 0 ? "None recorded" : String(number) }
        func tokens(_ number: Int64?) -> String? {
            number.map { NumberFormatter.localizedString(from: NSNumber(value: $0), number: .decimal) }
        }
        let tools = snapshot?.toolEvents ?? []
        let active = tools.filter { ["running", "pending", "in_progress"].contains($0.status.lowercased()) }
        let failures = tools.filter { $0.error != nil || ["failed", "error"].contains($0.status.lowercased()) }
        let timestamps = (snapshot?.messages.compactMap(\.timestamp) ?? []) + tools.flatMap { [$0.startedAt, $0.completedAt].compactMap { $0 } }
        let first = timestamps.min()
        let last = timestamps.max()
        let span: String?
        if let first, let last {
            let seconds = max(0, Int(last.timeIntervalSince(first)))
            span = seconds >= 3_600 ? "\(seconds / 3_600)h \((seconds % 3_600) / 60)m" : seconds >= 60 ? "\(seconds / 60)m \(seconds % 60)s" : "\(seconds)s"
        } else { span = nil }
        let usageDetail = snapshot?.usage.map { telemetryJSON($0) } ?? missing
        let cost: String?
        if let amount = snapshot?.recordedCost {
            cost = "\(String(format: "%.6g", amount)) \(snapshot?.costCurrency ?? "(currency not recorded)")"
        } else { cost = nil }
        let latestTool = tools.last
        let warningDetail = snapshot.map { $0.warnings.isEmpty ? "No collection warnings were reported." : $0.warnings.joined(separator: "\n\n") }
        return [
            .init(id: "task", title: "Task & session", rows: [
                info("Task", snapshot?.taskPrompt?.split(separator: "\n", omittingEmptySubsequences: true).first.map(String.init), snapshot?.taskPrompt),
                info("Session", snapshot?.title, snapshot?.rawInfoJSON.mapTelemetryPretty),
                info("Session ID", snapshot?.sessionID, snapshot?.rawInfoJSON.mapTelemetryPretty),
                info("Parent session", snapshot?.parentSessionID, snapshot?.rawInfoJSON.mapTelemetryPretty),
                info("Agent", snapshot?.agent, snapshot?.rawInfoJSON.mapTelemetryPretty)
            ]),
            .init(id: "model", title: "Model & harness", rows: [
                info("Model", snapshot?.model, snapshot?.rawInfoJSON.mapTelemetryPretty),
                info("Provider", snapshot?.provider, snapshot?.rawInfoJSON.mapTelemetryPretty),
                info("Harness", snapshot.map { $0.source.contains("DeepSeek") ? "DeepSeek Harness" : "OpenCode" }), setting("Configuration")
            ]),
            .init(id: "workspace", title: "Workspace & context", rows: [
                info("Workspace", snapshot?.project),
                info("Context records", snapshot.map { count($0.context.count) }, snapshot.map { telemetryJSON($0.context) }),
                info("Recorded span", span, "First recorded activity: \(telemetryDate(first))\nLast recorded activity: \(telemetryDate(last))\n\nThis span covers the activity present in this snapshot."),
                info("Last activity", snapshot == nil ? nil : telemetryDate(last)),
                info("Source", snapshot?.source, snapshot.map { "\($0.source)\nCollected: \(telemetryDate($0.collectedAt))\n\n\(warningDetail ?? "")" })
            ]),
            .init(id: "tools", title: "Tools & agents", rows: [
                info("Recorded tools", snapshot.map { count($0.toolEvents.count) }, snapshot.map { telemetryJSON($0.toolEvents) }),
                info("Active tools", snapshot.map { _ in count(active.count) }, snapshot.map { _ in telemetryJSON(active) }),
                info("Recorded failures", snapshot.map { _ in count(failures.count) }, snapshot.map { _ in telemetryJSON(failures) }),
                info("Subagents", snapshot.map { count($0.subagents.count) }, snapshot.map { telemetryJSON($0.subagents) }),
                info("Latest tool", latestTool.map { "\($0.name) · \($0.status)" }, latestTool?.rawJSON.mapTelemetryPretty)
            ]),
            .init(id: "usage", title: "Tokens & cost", rows: [
                info("Input tokens", tokens(snapshot?.usage?.input), usageDetail),
                info("Output tokens", tokens(snapshot?.usage?.output), usageDetail),
                info("Reasoning tokens", tokens(snapshot?.usage?.reasoning), usageDetail),
                info("Cache read", tokens(snapshot?.usage?.cacheRead), usageDetail),
                info("Cache write", tokens(snapshot?.usage?.cacheWrite), usageDetail),
                info("Total tokens", tokens(snapshot?.usage?.total), usageDetail),
                info("Recorded cost", cost, snapshot.map { "Recorded cost: \(cost ?? "Not recorded")\n\nSession info\n\(telemetryPretty($0.rawInfoJSON))" })
            ]),
            .init(id: "health", title: "Health & access", rows: [
                info("Collection", snapshot.map { $0.error ?? "Collected" }, snapshot.map { $0.error ?? "The recorded session data was collected at \(telemetryDate($0.collectedAt))." }),
                info("Warnings", snapshot.map { $0.warnings.isEmpty ? "None reported" : "\($0.warnings.count) reported" }, warningDetail),
                setting("Permissions"), setting("Plugins"), setting("Sandbox")
            ])
        ]
    }
}

private extension String {
    var mapTelemetryPretty: String { telemetryPretty(self) }
}

// NSButton keeps normal keyboard and accessibility behavior while the two
// labels provide a scan-friendly name/value row. All clicks target the button.
private final class TelemetryValueButton: NSButton {
    private let name = NSTextField(labelWithString: "")
    private let value = NSTextField(labelWithString: "")
    private let arrow = NSTextField(labelWithString: "›")
    var onOpen: (() -> Void)?

    init(row: TelemetrySummaryRow) {
        super.init(frame: .zero)
        title = ""
        setButtonType(.momentaryPushIn)
        bezelStyle = .inline
        isBordered = false
        target = self
        action = #selector(open)
        name.font = Theme.uiFont(11)
        name.textColor = Theme.textMuted
        value.font = Theme.uiFont(12, weight: .medium)
        value.textColor = Theme.textBase
        arrow.font = Theme.uiFont(15)
        arrow.textColor = Theme.textFaint
        for label in [name, value, arrow] {
            label.translatesAutoresizingMaskIntoConstraints = false
            label.lineBreakMode = .byTruncatingTail
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            label.setAccessibilityElement(false)
            addSubview(label)
            label.centerYAnchor.constraint(equalTo: centerYAnchor).isActive = true
        }
        value.lineBreakMode = .byTruncatingMiddle
        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: 26),
            name.leadingAnchor.constraint(equalTo: leadingAnchor),
            name.widthAnchor.constraint(equalTo: widthAnchor, multiplier: 0.41),
            value.leadingAnchor.constraint(equalTo: name.trailingAnchor, constant: 8),
            value.trailingAnchor.constraint(equalTo: arrow.leadingAnchor, constant: -6),
            arrow.trailingAnchor.constraint(equalTo: trailingAnchor),
            arrow.widthAnchor.constraint(equalToConstant: 8)
        ])
        update(row)
    }

    func update(_ row: TelemetrySummaryRow) {
        name.stringValue = row.title
        value.stringValue = row.value
        identifier = NSUserInterfaceItemIdentifier(row.id)
        toolTip = "\(row.title): \(row.value)\nShow recorded details"
        setAccessibilityLabel("\(row.title): \(row.value)")
        setAccessibilityHelp("Open recorded details")
    }

    override func hitTest(_ point: NSPoint) -> NSView? { super.hitTest(point) == nil ? nil : self }
    @objc private func open() { onOpen?() }
    required init?(coder: NSCoder) { fatalError() }
}

private final class TelemetrySectionView: NSView {
    let sectionID: String
    let heading = NSButton()
    let rows: [TelemetryValueButton]
    let naturalHeight: CGFloat

    init(section: TelemetrySummarySection, onOpen: @escaping (String) -> Void) {
        sectionID = section.id
        rows = section.rows.map(TelemetryValueButton.init)
        naturalHeight = 33 + CGFloat(rows.count) * 26
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        heading.title = section.title + "  ›"
        heading.font = Theme.uiFont(13, weight: .semibold)
        heading.contentTintColor = Theme.textBase
        heading.alignment = .left
        heading.isBordered = false
        heading.bezelStyle = .inline
        heading.lineBreakMode = .byTruncatingTail
        heading.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        heading.identifier = NSUserInterfaceItemIdentifier(section.id)
        heading.target = self
        heading.action = #selector(openSection)
        heading.setAccessibilityLabel(section.title)
        heading.setAccessibilityHelp("Open recorded details for this section")
        self.onOpen = onOpen
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.distribution = .fill
        stack.spacing = 0
        stack.setHuggingPriority(.defaultLow, for: .horizontal)
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(heading)
        heading.heightAnchor.constraint(equalToConstant: 24).isActive = true
        for height: CGFloat in [4, 1, 4] {
            let separator = NSView()
            separator.wantsLayer = true
            separator.layer?.backgroundColor = height == 1 ? Theme.borderMuted.themeCGColor : NSColor.clear.cgColor
            stack.addArrangedSubview(separator)
            separator.heightAnchor.constraint(equalToConstant: height).isActive = true
            separator.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        for (index, button) in rows.enumerated() {
            button.onOpen = { onOpen(section.rows[index].id) }
            stack.addArrangedSubview(button)
            button.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor), stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor), stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            heading.widthAnchor.constraint(equalTo: stack.widthAnchor), heightAnchor.constraint(equalToConstant: naturalHeight)
        ])
    }

    private var onOpen: ((String) -> Void)?
    @objc private func openSection() { onOpen?(sectionID) }
    func update(_ section: TelemetrySummarySection, enabled: Bool) {
        heading.isEnabled = enabled
        for (button, row) in zip(rows, section.rows) { button.update(row); button.isEnabled = enabled }
    }
    required init?(coder: NSCoder) { fatalError() }
}

final class TelemetryDashboardView: NSView {
    // Override presentation when the containing pane owns its own inspector.
    var onSelectDetail: ((String) -> Void)?
    var onOpenSession: ((String) -> Void)?
    private(set) var snapshot: TelemetrySnapshot?
    private let columns = NSStackView()
    private let provenance = NSButton()
    private let inspectAll = NSButton(title: "Inspect all", target: nil, action: nil)
    private var sectionViews: [TelemetrySectionView] = []
    private var columnCount = 0
    private var naturalHeight: NSLayoutConstraint!
    private var inspector: TelemetryDetailController?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        let heading = headerLabel("Agent telemetry", size: 14, weight: .semibold)
        let header = NSStackView(views: [heading, NSView(), inspectAll])
        header.orientation = .horizontal
        header.spacing = 10
        header.arrangedSubviews[1].setContentHuggingPriority(.defaultLow, for: .horizontal)
        inspectAll.bezelStyle = .rounded
        inspectAll.controlSize = .small
        inspectAll.target = self
        inspectAll.action = #selector(openAll)
        provenance.title = "Waiting for recorded session data…"
        provenance.font = Theme.uiFont(11)
        provenance.contentTintColor = Theme.textMuted
        provenance.isBordered = false
        provenance.alignment = .left
        provenance.lineBreakMode = .byTruncatingMiddle
        provenance.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        provenance.target = self
        provenance.action = #selector(openSource)
        columns.orientation = .horizontal
        columns.alignment = .top
        columns.distribution = .fillEqually
        columns.spacing = 24
        columns.setHuggingPriority(.defaultLow, for: .horizontal)
        for child in [header, provenance, columns] {
            child.translatesAutoresizingMaskIntoConstraints = false
            addSubview(child)
        }
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: topAnchor), header.leadingAnchor.constraint(equalTo: leadingAnchor), header.trailingAnchor.constraint(equalTo: trailingAnchor), header.heightAnchor.constraint(equalToConstant: 24),
            provenance.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 2), provenance.leadingAnchor.constraint(equalTo: leadingAnchor), provenance.trailingAnchor.constraint(equalTo: trailingAnchor), provenance.heightAnchor.constraint(equalToConstant: 18),
            columns.topAnchor.constraint(equalTo: provenance.bottomAnchor, constant: 12), columns.leadingAnchor.constraint(equalTo: leadingAnchor), columns.trailingAnchor.constraint(equalTo: trailingAnchor), columns.bottomAnchor.constraint(equalTo: bottomAnchor)
        ])
        sectionViews = TelemetryPresentation.sections(nil).map { section in
            TelemetrySectionView(section: section) { [weak self] id in self?.inspect(sectionID: id) }
        }
        naturalHeight = heightAnchor.constraint(equalToConstant: 600)
        naturalHeight.isActive = true
        arrangeColumns(2)
        setEnabled(false)
    }

    func update(_ snapshot: TelemetrySnapshot) {
        self.snapshot = snapshot
        let sections = TelemetryPresentation.sections(snapshot)
        for (view, section) in zip(sectionViews, sections) { view.update(section, enabled: true) }
        provenance.title = "\(snapshot.source) · Collected \(telemetryDate(snapshot.collectedAt))" + (snapshot.error == nil ? "" : " · Collection issue")
        provenance.toolTip = provenance.title
        provenance.contentTintColor = snapshot.error == nil ? Theme.textMuted : Theme.warning
        setEnabled(true)
    }

    private func setEnabled(_ enabled: Bool) {
        inspectAll.isEnabled = enabled
        provenance.isEnabled = enabled
        for view in sectionViews {
            view.heading.isEnabled = enabled
            view.rows.forEach { $0.isEnabled = enabled }
        }
    }

    override func layout() {
        let desired = bounds.width >= 1_050 ? 3 : 2
        if desired != columnCount { arrangeColumns(desired) }
        super.layout()
    }

    private func arrangeColumns(_ count: Int) {
        sectionViews.forEach { $0.removeFromSuperview() }
        for column in columns.arrangedSubviews { columns.removeArrangedSubview(column); column.removeFromSuperview() }
        var heights = [CGFloat](repeating: 0, count: count)
        let stacks = (0..<count).map { _ -> NSStackView in
            let stack = NSStackView()
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.distribution = .fill
            stack.spacing = 18
            stack.setHuggingPriority(.defaultLow, for: .horizontal)
            columns.addArrangedSubview(stack)
            return stack
        }
        for (index, section) in sectionViews.enumerated() {
            let column = index % count
            if heights[column] > 0 { heights[column] += 18 }
            heights[column] += section.naturalHeight
            stacks[column].addArrangedSubview(section)
            section.widthAnchor.constraint(equalTo: stacks[column].widthAnchor).isActive = true
        }
        columnCount = count
        naturalHeight.constant = 56 + (heights.max() ?? 0)
    }

    func inspect(sectionID: String = "all") {
        guard let snapshot else { return }
        if let onSelectDetail { onSelectDetail(sectionID); return }
        if let inspector { inspector.update(snapshot, selectionID: sectionID) }
        else { inspector = TelemetryDetailController(snapshot: snapshot, selectionID: sectionID) }
        inspector?.onOpenSession = onOpenSession
        inspector?.present()
    }
    @objc private func openAll() { inspect() }
    @objc private func openSource() { inspect(sectionID: "Source") }
    required init?(coder: NSCoder) { fatalError() }
}

private struct TelemetryDetailRow {
    var id: String
    var title: String
    var summary: String
    var time: Date?
    var detail: () -> String
    var linkedSessionID: String? = nil
}

final class TelemetryDetailController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    private static var openWindows: [TelemetryDetailController] = []
    private let tabs = NSSegmentedControl(labels: ["Overview", "Timeline", "Context", "Agents", "Runtime"], trackingMode: .selectOne, target: nil, action: nil)
    private let search = NSSearchField()
    private let table = NSTableView()
    private var timeColumn: NSTableColumn?
    private let text = NSTextView()
    private let titleLabel = headerLabel("", size: 16, weight: .semibold)
    private let sourceLabel = headerLabel("", size: 11, color: Theme.textMuted)
    private let countLabel = headerLabel("", size: 11, color: Theme.textMuted)
    private let detailTitle = headerLabel("Recorded details", size: 13, weight: .semibold)
    private let openSession = NSButton(title: "Open Session", target: nil, action: nil)
    var onOpenSession: ((String) -> Void)? { didSet { openSession.isEnabled = onOpenSession != nil } }
    private var snapshot: TelemetrySnapshot
    private var rowsByTab: [[TelemetryDetailRow]] = []
    private var visibleRows: [TelemetryDetailRow] = []
    private var selectedID: String?

    init(snapshot: TelemetrySnapshot, selectionID: String = "all") {
        self.snapshot = snapshot
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1_100, height: 760), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        super.init(window: window)
        window.title = "Agent telemetry"
        window.isReleasedWhenClosed = false
        AppearanceController.apply(to: window)
        window.backgroundColor = Theme.base
        window.minSize = NSSize(width: 760, height: 520)
        window.delegate = self
        window.center()
        buildUI()
        update(snapshot, selectionID: selectionID)
    }

    func present() {
        if !Self.openWindows.contains(where: { $0 === self }) { Self.openWindows.append(self) }
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) { Self.openWindows.removeAll { $0 === self } }

    private func buildUI() {
        let root = NSView()
        let header = NSStackView(views: [titleLabel, sourceLabel])
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 4
        sourceLabel.lineBreakMode = .byTruncatingMiddle
        titleLabel.lineBreakMode = .byTruncatingTail
        let controls = NSStackView()
        controls.orientation = .horizontal
        controls.spacing = 10
        tabs.target = self
        tabs.action = #selector(tabChanged)
        tabs.selectedSegment = 0
        controls.addArrangedSubview(tabs)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        controls.addArrangedSubview(spacer)
        search.placeholderString = "Filter names and summaries"
        search.delegate = self
        controls.addArrangedSubview(search)
        search.widthAnchor.constraint(equalToConstant: 220).isActive = true
        let split = NSSplitView()
        split.isVertical = true
        split.dividerStyle = .thin
        let listPane = NSView()
        let detailPane = NSView()
        split.addArrangedSubview(listPane)
        split.addArrangedSubview(detailPane)
        split.setHoldingPriority(.defaultLow, forSubviewAt: 0)
        split.setHoldingPriority(.defaultHigh, forSubviewAt: 1)
        let listScroll = FittingTableScrollView()
        listScroll.hasVerticalScroller = true
        listScroll.drawsBackground = false
        let nameColumn = NSTableColumn(identifier: .init("record"))
        nameColumn.title = "Recorded item"
        nameColumn.width = 160
        nameColumn.minWidth = 90
        nameColumn.resizingMask = .autoresizingMask
        let valueColumn = NSTableColumn(identifier: .init("summary"))
        valueColumn.title = "Summary"
        valueColumn.width = 190
        valueColumn.minWidth = 90
        valueColumn.resizingMask = .autoresizingMask
        let timeColumn = NSTableColumn(identifier: .init("time"))
        self.timeColumn = timeColumn
        timeColumn.title = "Time"
        timeColumn.width = 90
        timeColumn.minWidth = 90
        timeColumn.maxWidth = 90
        timeColumn.resizingMask = []
        table.addTableColumn(timeColumn)
        table.addTableColumn(nameColumn)
        table.addTableColumn(valueColumn)
        table.delegate = self
        table.dataSource = self
        table.allowsEmptySelection = true
        table.usesAlternatingRowBackgroundColors = false
        table.backgroundColor = Theme.base
        listScroll.documentView = table
        let detailScroll = WrappingTextScrollView()
        detailScroll.hasVerticalScroller = true
        detailScroll.drawsBackground = false
        text.isEditable = false
        text.isSelectable = true
        text.isRichText = false
        text.usesFindBar = true
        text.font = Theme.codeFont(11)
        text.textColor = Theme.textBase
        text.backgroundColor = Theme.deep
        text.textContainerInset = NSSize(width: 12, height: 12)
        text.isVerticallyResizable = true
        text.isHorizontallyResizable = false
        text.autoresizingMask = [.width]
        text.textContainer?.widthTracksTextView = true
        detailScroll.documentView = text
        let copy = NSButton(title: "Copy Detail", target: self, action: #selector(copyDetail))
        copy.bezelStyle = .rounded
        copy.controlSize = .small
        openSession.bezelStyle = .rounded
        openSession.controlSize = .small
        openSession.target = self
        openSession.action = #selector(openLinkedSession)
        openSession.isHidden = true
        detailTitle.lineBreakMode = .byTruncatingTail
        detailTitle.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let detailHeader = NSStackView(views: [detailTitle, NSView(), openSession, copy])
        detailHeader.orientation = .horizontal
        detailHeader.arrangedSubviews[1].setContentHuggingPriority(.defaultLow, for: .horizontal)
        for child in [header, controls, split] { child.translatesAutoresizingMaskIntoConstraints = false; root.addSubview(child) }
        for child in [countLabel, listScroll] { child.translatesAutoresizingMaskIntoConstraints = false; listPane.addSubview(child) }
        for child in [detailHeader, detailScroll] { child.translatesAutoresizingMaskIntoConstraints = false; detailPane.addSubview(child) }
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: root.topAnchor, constant: 16), header.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20), header.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            titleLabel.widthAnchor.constraint(equalTo: header.widthAnchor), sourceLabel.widthAnchor.constraint(equalTo: header.widthAnchor),
            controls.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 14), controls.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20), controls.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            split.topAnchor.constraint(equalTo: controls.bottomAnchor, constant: 14), split.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12), split.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12), split.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -12),
            listPane.widthAnchor.constraint(greaterThanOrEqualToConstant: 300), detailPane.widthAnchor.constraint(greaterThanOrEqualToConstant: 320),
            countLabel.topAnchor.constraint(equalTo: listPane.topAnchor, constant: 4), countLabel.leadingAnchor.constraint(equalTo: listPane.leadingAnchor, constant: 8), countLabel.trailingAnchor.constraint(equalTo: listPane.trailingAnchor, constant: -8),
            listScroll.topAnchor.constraint(equalTo: countLabel.bottomAnchor, constant: 8), listScroll.leadingAnchor.constraint(equalTo: listPane.leadingAnchor), listScroll.trailingAnchor.constraint(equalTo: listPane.trailingAnchor), listScroll.bottomAnchor.constraint(equalTo: listPane.bottomAnchor),
            detailHeader.topAnchor.constraint(equalTo: detailPane.topAnchor), detailHeader.leadingAnchor.constraint(equalTo: detailPane.leadingAnchor, constant: 12), detailHeader.trailingAnchor.constraint(equalTo: detailPane.trailingAnchor, constant: -8),
            detailScroll.topAnchor.constraint(equalTo: detailHeader.bottomAnchor, constant: 8), detailScroll.leadingAnchor.constraint(equalTo: detailPane.leadingAnchor, constant: 8), detailScroll.trailingAnchor.constraint(equalTo: detailPane.trailingAnchor), detailScroll.bottomAnchor.constraint(equalTo: detailPane.bottomAnchor)
        ])
        let listWidth = listPane.widthAnchor.constraint(equalTo: split.widthAnchor, multiplier: 0.38)
        listWidth.priority = .defaultHigh
        listWidth.isActive = true
        window?.contentView = root
    }

    func update(_ snapshot: TelemetrySnapshot, selectionID: String = "all") {
        self.snapshot = snapshot
        titleLabel.stringValue = snapshot.title.isEmpty ? "Recorded session" : snapshot.title
        sourceLabel.stringValue = "\(snapshot.sessionID) · \(snapshot.source) · Collected \(telemetryDate(snapshot.collectedAt))"
        sourceLabel.toolTip = sourceLabel.stringValue
        window?.title = "Telemetry — \(snapshot.title)"
        let sections = TelemetryPresentation.sections(snapshot)
        var overview: [TelemetryDetailRow] = [
            .init(id: "all", title: "All recorded data", summary: snapshot.sessionID, time: nil, detail: { telemetryJSON(snapshot) }),
            .init(id: "raw-info", title: "Session info", summary: "Recorded source fields", time: nil, detail: { telemetryPretty(snapshot.rawInfoJSON) })
        ]
        for section in sections {
            overview.append(.init(id: section.id, title: section.title, summary: "\(section.rows.count) fields", time: nil, detail: {
                section.rows.map { "\($0.title): \($0.value)\n\n\($0.detail())" }.joined(separator: "\n\n────────────────────\n\n")
            }))
            overview += section.rows.map { .init(id: $0.id, title: $0.title, summary: $0.value, time: nil, detail: $0.detail) }
        }
        var timeline = snapshot.messages.map { message in
            TelemetryDetailRow(id: "message:\(message.id)", title: "\(message.type.capitalized) message", summary: message.text, time: message.timestamp, detail: { "\(message.type.capitalized) · \(telemetryDate(message.timestamp))\n\n\(message.text)\n\nRecorded data\n\(telemetryPretty(message.rawJSON))" })
        }
        timeline += snapshot.toolEvents.map { event in
            let raw = (try? JSONSerialization.jsonObject(with: Data(event.rawJSON.utf8))) as? [String: Any] ?? [:]
            let hierarchy = "Turn: \(raw["turn"].map { String(describing: $0) } ?? "Not recorded")\nStep: \(raw["step"].map { String(describing: $0) } ?? "Not recorded")\nAgent: \(raw["agent"].map { String(describing: $0) } ?? "Not recorded")"
            let schema: String
            if let value = raw["toolSchema"], !(value is NSNull) {
                if let string = value as? String { schema = telemetryPretty(string) }
                else if let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed]) { schema = String(decoding: data, as: UTF8.self) }
                else { schema = "Could not format the recorded schema. See full recorded data below." }
            } else { schema = "Not recorded" }
            return .init(id: "tool:\(event.id)", title: "Tool · \(event.name)", summary: event.status + (event.error.map { " · \($0)" } ?? ""), time: event.startedAt ?? event.completedAt, detail: {
                "Summary\n\(event.name)\nStatus: \(event.status)\nCall ID: \(event.id)\nError: \(event.error ?? "None reported")\n\nHierarchy\n\(hierarchy)\n\nPayload\n\(telemetryPretty(event.inputJSON))\n\nResult\n\(event.output ?? "Not recorded")\n\nSchema\n\(schema)\n\nTiming\nStarted: \(telemetryDate(event.startedAt))\nCompleted: \(telemetryDate(event.completedAt))\nDuration: \(event.duration.map { String(format: "%.3f s", $0) } ?? "Not recorded")\n\nRecorded data\n\(telemetryPretty(event.rawJSON))"
            }, linkedSessionID: raw["subagentSessionID"] as? String)
        }
        timeline = timeline.enumerated().sorted { left, right in
            if let a = left.element.time, let b = right.element.time, a != b { return a < b }
            if left.element.time != nil && right.element.time == nil { return true }
            if left.element.time == nil && right.element.time != nil { return false }
            return left.offset < right.offset
        }.map(\.element)
        let context = snapshot.context.map { record in
            TelemetryDetailRow(id: "context:\(record.id)", title: record.title, summary: record.value, time: nil, detail: { "\(record.title)\n\n\(record.value)\n\nRecorded data\n\(telemetryPretty(record.detailJSON))" })
        }
        let agents = snapshot.subagents.map { agent in
            TelemetryDetailRow(id: "agent:\(agent.id)", title: agent.title, summary: [agent.agent, agent.status, agent.sessionID].compactMap { $0 }.joined(separator: " · "), time: nil, detail: {
                "\(agent.title)\nAgent: \(agent.agent ?? "Not recorded")\nStatus: \(agent.status ?? "Not recorded")\nSession: \(agent.sessionID ?? "Not recorded")\n\nPrompt\n\(agent.prompt ?? "Not recorded")\n\nRecorded data\n\(telemetryPretty(agent.rawJSON))"
            }, linkedSessionID: agent.sessionID)
        }
        let runtime = snapshot.runtime.map { record in
            TelemetryDetailRow(id: "runtime:\(record.id)", title: record.title, summary: record.value, time: nil, detail: { "\(record.title)\n\n\(record.value)\n\nRecorded data\n\(telemetryPretty(record.detailJSON))" })
        }
        rowsByTab = [overview, timeline, context, agents, runtime]
        search.stringValue = ""
        selectedID = selectionID
        tabs.selectedSegment = 0
        let targetTool: TelemetryToolEvent?
        switch selectionID {
        case "Recorded tools": targetTool = snapshot.toolEvents.first
        case "Active tools": targetTool = snapshot.toolEvents.first { ["running", "pending", "in_progress"].contains($0.status.lowercased()) }
        case "Recorded failures": targetTool = snapshot.toolEvents.first { $0.error != nil || ["failed", "error"].contains($0.status.lowercased()) }
        case "Latest tool": targetTool = snapshot.toolEvents.last
        default: targetTool = nil
        }
        if let targetTool {
            tabs.selectedSegment = 1; selectedID = "tool:\(targetTool.id)"
        } else if selectionID == "Context records" && !context.isEmpty {
            tabs.selectedSegment = 2; selectedID = context[0].id
        } else if selectionID == "Subagents" && !agents.isEmpty {
            tabs.selectedSegment = 3; selectedID = agents[0].id
        } else if let first = runtime.first(where: { $0.title == selectionID }) {
            tabs.selectedSegment = 4; selectedID = first.id
        }
        reloadRows()
    }

    @objc private func tabChanged() { selectedID = nil; reloadRows() }
    func controlTextDidChange(_ obj: Notification) { reloadRows() }
    private func reloadRows() {
        guard rowsByTab.indices.contains(tabs.selectedSegment) else { return }
        if let timeColumn {
            let included = table.tableColumns.contains(where: { $0 === timeColumn })
            if tabs.selectedSegment == 1 && !included {
                table.addTableColumn(timeColumn)
                table.moveColumn(table.tableColumns.count - 1, toColumn: 0)
            } else if tabs.selectedSegment != 1 && included {
                table.removeTableColumn(timeColumn)
            }
        }
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        visibleRows = rowsByTab[tabs.selectedSegment].filter { query.isEmpty || $0.title.localizedCaseInsensitiveContains(query) || $0.summary.localizedCaseInsensitiveContains(query) }
        countLabel.stringValue = "\(visibleRows.count) of \(rowsByTab[tabs.selectedSegment].count) recorded items"
        table.reloadData()
        if let index = visibleRows.firstIndex(where: { $0.id == selectedID }) ?? (visibleRows.isEmpty ? nil : 0) {
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            table.scrollRowToVisible(index)
            showDetail(index)
        } else {
            table.deselectAll(nil)
            detailTitle.stringValue = query.isEmpty ? "No recorded items" : "No matching items"
            openSession.isHidden = true
            text.string = query.isEmpty ? "The source did not include any items for this category. Missing information is not a recorded zero." : "No names or summaries match “\(query)”. Clear the filter to see all recorded items."
        }
    }

    func numberOfRows(in tableView: NSTableView) -> Int { visibleRows.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard visibleRows.indices.contains(row) else { return nil }
        let identifier = tableColumn?.identifier ?? .init("record")
        let cell = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView ?? NSTableCellView()
        if cell.textField == nil {
            let label = NSTextField(labelWithString: "")
            label.translatesAutoresizingMaskIntoConstraints = false
            label.font = Theme.uiFont(11)
            label.lineBreakMode = .byTruncatingTail
            cell.addSubview(label)
            cell.textField = label
            NSLayoutConstraint.activate([label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6), label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6), label.centerYAnchor.constraint(equalTo: cell.centerYAnchor)])
        }
        cell.identifier = identifier
        let record = visibleRows[row]
        cell.textField?.stringValue = identifier.rawValue == "time" ? (record.time.map { telemetryDate($0, timeOnly: true) } ?? "Not recorded") : identifier.rawValue == "record" ? record.title : record.summary.replacingOccurrences(of: "\n", with: " ")
        cell.textField?.textColor = identifier.rawValue == "record" ? Theme.textBase : Theme.textMuted
        cell.toolTip = "\(record.title)\n\(record.summary)\n\(record.time.map { telemetryDate($0) } ?? "")"
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) { showDetail(table.selectedRow) }
    private func showDetail(_ index: Int) {
        guard visibleRows.indices.contains(index) else { return }
        let record = visibleRows[index]
        selectedID = record.id
        detailTitle.stringValue = record.title
        detailTitle.toolTip = record.title
        openSession.isHidden = record.linkedSessionID == nil
        openSession.isEnabled = onOpenSession != nil
        openSession.toolTip = record.linkedSessionID.map { "Open recorded session \($0)" }
        text.string = record.detail()
        text.setSelectedRange(NSRange(location: 0, length: 0))
        text.scrollToBeginningOfDocument(nil)
    }
    @objc private func copyDetail() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text.string, forType: .string)
    }
    @objc private func openLinkedSession() {
        guard visibleRows.indices.contains(table.selectedRow), let sessionID = visibleRows[table.selectedRow].linkedSessionID else { return }
        onOpenSession?(sessionID)
    }
    required init?(coder: NSCoder) { fatalError() }
}
