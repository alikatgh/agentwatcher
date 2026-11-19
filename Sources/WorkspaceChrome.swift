import AppKit

struct WorkspaceCommand {
    let id: String
    let title: String
    let subtitle: String
    let shortcut: String
    let perform: () -> Void

    init(id: String, title: String, subtitle: String = "", shortcut: String = "",
         perform: @escaping () -> Void) {
        self.id = id
        self.title = title
        self.subtitle = subtitle
        self.shortcut = shortcut
        self.perform = perform
    }
}

/// A restrained selection treatment for both navigation and command lists.
final class WorkspaceNavigationRowView: NSTableRowView {
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
    override func drawSelection(in dirtyRect: NSRect) {
        guard selectionHighlightStyle != .none else { return }
        Theme.layer02.setFill()
        bounds.fill()
        Theme.accent.setFill()
        NSRect(x: bounds.minX, y: bounds.minY, width: 2, height: bounds.height).fill()
    }
}

/// The host owns its width constraints; each drag reports a window-space delta.
final class WorkspaceDivider: NSView {
    var onDrag: ((CGFloat) -> Void)?
    var onReset: (() -> Void)?
    private var lastMouseX: CGFloat?
    private var hovering = false
    private var tracking: NSTrackingArea?

    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityElement(true)
        setAccessibilityRole(.splitter)
        setAccessibilityLabel("Resize workspace panes")
        setAccessibilityHelp("Drag or use the left and right arrow keys to resize. Double-click or press Return to reset.")
    }

    convenience init() { self.init(frame: .zero) }
    required init?(coder: NSCoder) { fatalError() }
    override var acceptsFirstResponder: Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero,
                                  options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
    }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .resizeLeftRight) }
    override func mouseEntered(with event: NSEvent) { hovering = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovering = false; needsDisplay = true }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 {
            lastMouseX = nil
            onReset?()
        } else {
            lastMouseX = event.locationInWindow.x
        }
        needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        guard let previous = lastMouseX else { return }
        let current = event.locationInWindow.x
        lastMouseX = current
        let delta = current - previous
        if delta != 0 { onDrag?(delta) }
    }

    override func mouseUp(with event: NSEvent) { lastMouseX = nil; needsDisplay = true }

    override func keyDown(with event: NSEvent) {
        guard event.modifierFlags.intersection([.command, .control, .option]).isEmpty else {
            super.keyDown(with: event)
            return
        }
        let step: CGFloat = event.modifierFlags.contains(.shift) ? 50 : 10
        switch event.keyCode {
        case 123: onDrag?(-step)
        case 124: onDrag?(step)
        case 36, 76: onReset?()
        default: super.keyDown(with: event)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        (hovering || lastMouseX != nil ? Theme.borderStrong : Theme.border).setFill()
        NSRect(x: floor(bounds.midX), y: bounds.minY, width: 1, height: bounds.height).fill()
    }
}

private final class WorkspaceCommandPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

private final class WorkspaceCommandRowView: NSTableRowView {
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
    override func drawSelection(in dirtyRect: NSRect) {
        guard selectionHighlightStyle != .none else { return }
        Theme.layer03.setFill()
        bounds.fill()
    }
}

private final class WorkspaceCommandCell: NSTableCellView {
    private let titleLabel = NSTextField(labelWithString: "")
    private let subtitleLabel = NSTextField(labelWithString: "")
    private let shortcutLabel = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        titleLabel.font = Theme.uiFont(12, weight: .medium)
        titleLabel.textColor = Theme.textBase
        subtitleLabel.font = Theme.uiFont(10)
        subtitleLabel.textColor = Theme.textMuted
        shortcutLabel.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        shortcutLabel.textColor = Theme.textMuted
        shortcutLabel.alignment = .right
        shortcutLabel.setContentHuggingPriority(.required, for: .horizontal)
        shortcutLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        for label in [titleLabel, subtitleLabel, shortcutLabel] {
            label.translatesAutoresizingMaskIntoConstraints = false
            label.lineBreakMode = .byTruncatingTail
            addSubview(label)
        }
        titleLabel.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
        titleLabel.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        subtitleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        NSLayoutConstraint.activate([
            titleLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            titleLabel.trailingAnchor.constraint(lessThanOrEqualTo: shortcutLabel.leadingAnchor, constant: -12),
            titleLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            subtitleLabel.leadingAnchor.constraint(equalTo: titleLabel.trailingAnchor, constant: 8),
            subtitleLabel.trailingAnchor.constraint(lessThanOrEqualTo: shortcutLabel.leadingAnchor, constant: -12),
            subtitleLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            shortcutLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            shortcutLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ command: WorkspaceCommand, selected: Bool) {
        titleLabel.stringValue = command.title
        subtitleLabel.stringValue = command.subtitle
        shortcutLabel.stringValue = command.shortcut
        subtitleLabel.isHidden = command.subtitle.isEmpty
        titleLabel.textColor = selected ? .white : Theme.textBase
        subtitleLabel.textColor = selected ? NSColor.white.withAlphaComponent(0.82) : Theme.textMuted
        shortcutLabel.textColor = selected ? NSColor.white.withAlphaComponent(0.9) : Theme.textMuted
        toolTip = command.subtitle.isEmpty ? command.title : "\(command.title) — \(command.subtitle)"
        setAccessibilityElement(true)
        setAccessibilityLabel([command.title, command.subtitle, command.shortcut].filter { !$0.isEmpty }.joined(separator: ", "))
    }
}

/// Window-scoped commands. Showing the palette does not load data or run work.
final class WorkspaceCommandPaletteController: NSWindowController,
                                                NSWindowDelegate,
                                                NSTableViewDataSource,
                                                NSTableViewDelegate,
                                                NSSearchFieldDelegate {
    private let commands: [WorkspaceCommand]
    private(set) var filteredCommands: [WorkspaceCommand]
    private(set) var isPresented = false
    let searchField = NSSearchField()
    private let table = NSTableView()
    private let noMatches = NSTextField(labelWithString: "No matching commands")
    private let footer = NSTextField(labelWithString: "↑ ↓  Navigate     ↵  Run command     esc  Close")
    private weak var ownerWindow: NSWindow?
    private weak var previousFirstResponder: NSResponder?
    private var keyMonitor: Any?
    private var ownerCloseObservation: NSObjectProtocol?

    var selectedCommandID: String? {
        guard filteredCommands.indices.contains(table.selectedRow) else { return nil }
        return filteredCommands[table.selectedRow].id
    }

    init(commands: [WorkspaceCommand]) {
        self.commands = commands
        filteredCommands = commands
        let panel = WorkspaceCommandPanel(contentRect: NSRect(x: 0, y: 0, width: 560, height: 420),
                                          styleMask: [.titled, .fullSizeContentView],
                                          backing: .buffered, defer: false)
        panel.title = "Commands"
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isReleasedWhenClosed = false
        panel.isMovableByWindowBackground = false
        panel.hidesOnDeactivate = false
        panel.level = .floating
        panel.animationBehavior = .none
        for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            panel.standardWindowButton(button)?.isHidden = true
        }
        super.init(window: panel)
        panel.delegate = self
        buildContent()
        AppearanceController.apply(to: panel)
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit {
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        if let ownerCloseObservation { NotificationCenter.default.removeObserver(ownerCloseObservation) }
    }

    private func buildContent() {
        guard let window else { return }
        let root = NSView()
        root.wantsLayer = true
        root.layer?.backgroundColor = Theme.layer01.themeCGColor
        root.layer?.borderColor = Theme.border.themeCGColor
        root.layer?.borderWidth = 1
        window.contentView = root

        searchField.placeholderString = "Search commands…"
        searchField.font = Theme.uiFont(13)
        searchField.controlSize = .regular
        searchField.focusRingType = .none
        searchField.sendsSearchStringImmediately = true
        searchField.sendsWholeSearchString = false
        searchField.delegate = self
        searchField.setAccessibilityLabel("Search workspace commands")

        table.headerView = nil
        table.rowHeight = 30
        table.intercellSpacing = .zero
        table.backgroundColor = .clear
        table.style = .plain
        table.selectionHighlightStyle = .regular
        table.allowsEmptySelection = false
        table.allowsMultipleSelection = false
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.autoresizingMask = [.width]
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(commandClicked)
        table.setAccessibilityLabel("Workspace commands")
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("command"))
        column.width = 540
        column.minWidth = 0
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)

        let scroll = FittingTableScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.documentView = table
        let searchSeparator = NSBox()
        searchSeparator.boxType = .separator
        let footerSeparator = NSBox()
        footerSeparator.boxType = .separator
        noMatches.font = Theme.uiFont(12)
        noMatches.textColor = Theme.textMuted
        noMatches.alignment = .center
        noMatches.lineBreakMode = .byTruncatingTail
        footer.font = Theme.uiFont(10)
        footer.textColor = Theme.textMuted
        footer.lineBreakMode = .byTruncatingTail

        let contentViews: [NSView] = [searchField, searchSeparator, scroll, noMatches, footerSeparator, footer]
        for view in contentViews {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            searchField.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            searchField.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            searchField.topAnchor.constraint(equalTo: root.topAnchor, constant: 6),
            searchField.heightAnchor.constraint(equalToConstant: 24),
            searchSeparator.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            searchSeparator.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            searchSeparator.topAnchor.constraint(equalTo: searchField.bottomAnchor, constant: 6),
            scroll.topAnchor.constraint(equalTo: searchSeparator.bottomAnchor, constant: 4),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 4),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -4),
            scroll.bottomAnchor.constraint(equalTo: footerSeparator.topAnchor, constant: -4),
            noMatches.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            noMatches.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            noMatches.centerYAnchor.constraint(equalTo: scroll.centerYAnchor),
            footerSeparator.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            footerSeparator.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            footerSeparator.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -9),
            footer.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 14),
            footer.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -14),
            footer.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -10),
        ])
        updateQuery("")
    }

    func show(relativeTo owner: NSWindow) {
        guard let panel = window, owner !== panel else { return }
        if isPresented { dismiss(restoreFocus: false) }
        ownerWindow = owner
        previousFirstResponder = owner.firstResponder
        updateQuery("")
        let width = min(560, max(300, owner.frame.width - 48))
        let height = min(420, max(240, owner.frame.height - 100))
        var frame = NSRect(x: owner.frame.midX - width / 2,
                           y: owner.frame.maxY - height - 72,
                           width: width, height: height)
        if let visible = owner.screen?.visibleFrame {
            frame.origin.x = max(visible.minX, min(frame.minX, visible.maxX - width))
            frame.origin.y = max(visible.minY, min(frame.minY, visible.maxY - height))
        }
        panel.setFrame(frame, display: false)
        panel.appearance = owner.appearance
        isPresented = true
        owner.addChildWindow(panel, ordered: .above)
        ownerCloseObservation = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification,
                                                                      object: owner, queue: .main) { [weak self] _ in
            self?.dismiss(restoreFocus: false)
        }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.isPresented, event.window === self.window else { return event }
            return self.handleKey(event) ? nil : event
        }
        panel.makeKeyAndOrderFront(nil)
        panel.makeFirstResponder(searchField)
        searchField.selectText(nil)
    }

    override func close() { dismiss(restoreFocus: true) }

    private func dismiss(restoreFocus: Bool) {
        guard isPresented else { return }
        isPresented = false
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor); self.keyMonitor = nil }
        if let ownerCloseObservation {
            NotificationCenter.default.removeObserver(ownerCloseObservation)
            self.ownerCloseObservation = nil
        }
        let owner = ownerWindow
        let responder = previousFirstResponder
        ownerWindow = nil
        previousFirstResponder = nil
        if let window {
            owner?.removeChildWindow(window)
            window.orderOut(nil)
        }
        if restoreFocus, let owner, owner.isVisible, NSApp.isActive {
            owner.makeKeyAndOrderFront(nil)
            if let view = responder as? NSView, view.window !== owner {
                owner.makeFirstResponder(nil)
            } else {
                owner.makeFirstResponder(responder)
            }
        }
    }

    func windowDidResignKey(_ notification: Notification) { dismiss(restoreFocus: false) }
    func windowWillClose(_ notification: Notification) { dismiss(restoreFocus: true) }

    func controlTextDidChange(_ notification: Notification) { updateQuery(searchField.stringValue) }

    func control(_ control: NSControl, textView: NSTextView,
                 doCommandBy commandSelector: Selector) -> Bool {
        // The input method owns candidate navigation, commit, and cancellation
        // while its field editor contains marked text.
        guard isPresented, control === searchField, !textView.hasMarkedText() else { return false }
        switch commandSelector {
        case #selector(NSResponder.moveDown(_:)): moveSelection(by: 1)
        case #selector(NSResponder.moveUp(_:)): moveSelection(by: -1)
        case #selector(NSResponder.insertNewline(_:)): performSelectedCommand()
        case #selector(NSResponder.cancelOperation(_:)): close()
        default: return false
        }
        return true
    }

    func updateQuery(_ query: String) {
        let selectedID = selectedCommandID
        // Do not replace an active field editor's text for its own change event.
        if searchField.stringValue != query { searchField.stringValue = query }
        let terms = query.split(whereSeparator: { $0.isWhitespace }).map(String.init)
        filteredCommands = commands.filter { command in
            let text = "\(command.title) \(command.subtitle) \(command.shortcut)"
            return terms.allSatisfy { text.localizedCaseInsensitiveContains($0) }
        }
        table.reloadData()
        noMatches.isHidden = !filteredCommands.isEmpty
        if !filteredCommands.isEmpty {
            let row = filteredCommands.firstIndex(where: { $0.id == selectedID }) ?? 0
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            table.scrollRowToVisible(row)
        } else {
            table.deselectAll(nil)
        }
    }

    func moveSelection(by delta: Int) {
        guard !filteredCommands.isEmpty else { return }
        let current = max(0, table.selectedRow)
        let next = min(filteredCommands.count - 1, max(0, current + delta))
        table.selectRowIndexes(IndexSet(integer: next), byExtendingSelection: false)
        table.scrollRowToVisible(next)
    }

    /// Every input path uses this guard, so dismissal prevents repeat execution.
    func performSelectedCommand() {
        guard isPresented, filteredCommands.indices.contains(table.selectedRow) else { return }
        let action = filteredCommands[table.selectedRow].perform
        close()
        action()
    }

    @objc private func commandClicked() {
        guard filteredCommands.indices.contains(table.clickedRow) else { return }
        table.selectRowIndexes(IndexSet(integer: table.clickedRow), byExtendingSelection: false)
        performSelectedCommand()
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        guard (window?.firstResponder as? NSTextView)?.hasMarkedText() != true else { return false }
        guard event.modifierFlags.intersection([.command, .control, .option]).isEmpty else { return false }
        switch event.keyCode {
        case 125: moveSelection(by: 1)
        case 126: moveSelection(by: -1)
        case 36, 76: performSelectedCommand()
        case 53: close()
        default: return false
        }
        return true
    }

    func numberOfRows(in tableView: NSTableView) -> Int { filteredCommands.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard filteredCommands.indices.contains(row) else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("WorkspaceCommandCell")
        let cell = (tableView.makeView(withIdentifier: identifier, owner: nil) as? WorkspaceCommandCell)
            ?? WorkspaceCommandCell()
        cell.identifier = identifier
        cell.configure(filteredCommands[row], selected: tableView.selectedRow == row)
        return cell
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        WorkspaceCommandRowView()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        for row in filteredCommands.indices {
            guard let cell = table.view(atColumn: 0, row: row, makeIfNecessary: false) as? WorkspaceCommandCell else { continue }
            cell.configure(filteredCommands[row], selected: table.selectedRow == row)
        }
    }
}
