import AppKit

// Repository and process snapshots contain values only. All AppKit work stays
// on the main queue; collectors never read changing preferences in flight.
final class IncrementsPane: Pane {
    struct CommitRow: Equatable {
        let hash: String
        let shortHash: String
        let subject: String
        let when: String
    }
    struct HistoryRequest: Equatable { let project: String; var selectedHash: String? = nil }
    struct DetailRequest: Equatable { let project: String; let hash: String }
    struct HistoryResult { let rows: [CommitRow]; let error: String?; var selectionError: String? = nil }
    struct DetailResult { let text: String; let error: String? }

    let table = NSTableView()
    let detail = NSTextView()
    let listScroll = FittingTableScrollView()
    let detailScroll = WrappingTextScrollView()
    let body = NSStackView()
    let listGroup = NSStackView()
    let detailPanel = NSView()
    let statusLabel = headerLabel("Loading increments…", size: 12, color: Theme.textMuted)
    let hashLabel = headerLabel("", size: 12, color: Theme.textMuted)
    let detailTitle = NSTextField(wrappingLabelWithString: "Select an increment")
    let detailState = NSView()
    let detailSpinner = NSProgressIndicator()
    let detailStateTitle = headerLabel("Loading increments", size: 13, weight: .semibold)
    let detailStateMessage = NSTextField(wrappingLabelWithString: "Reading committed work from this project…")
    let copyButton = NSButton()
    let previousButton = NSButton()
    let nextButton = NSButton()
    var rows: [CommitRow] = []
    var currentCommit = ""
    private var pendingCommitHash: String?
    private var currentProject = ""
    private var loadedDetail: DetailRequest?
    private var requestedDetail: DetailRequest?
    private var applyingRows = false
    private var listHeight: NSLayoutConstraint!
    private var detailMinimumHeight: NSLayoutConstraint!
    private var listWidth: NSLayoutConstraint!
    private var detailBodyHeight: NSLayoutConstraint!
    private var listBodyHeight: NSLayoutConstraint!
    private var pageMinimumHeight: NSLayoutConstraint!
    private var stackedWidths: [NSLayoutConstraint] = []
    private lazy var historyLoader: LatestRequestLoader<HistoryRequest, HistoryResult> = {
        let loader = LatestRequestLoader<HistoryRequest, HistoryResult>(label: "agentwatcher.increments", collect: Self.collectHistory)
        loader.apply = { [weak self] request, result in self?.applyHistory(request, result) }
        return loader
    }()
    private lazy var detailLoader: LatestRequestLoader<DetailRequest, DetailResult> = {
        let loader = LatestRequestLoader<DetailRequest, DetailResult>(label: "agentwatcher.commit-detail", collect: Self.collectDetail)
        loader.apply = { [weak self] request, result in self?.applyDetail(request, result) }
        return loader
    }()

    static func parseHistory(_ output: String) -> [CommitRow] {
        output.split(separator: "\n").compactMap { line in
            let fields = line.split(separator: "\u{1f}", maxSplits: 3, omittingEmptySubsequences: false).map(String.init)
            guard fields.count == 4, fields[0].count >= 40,
                  fields[0].allSatisfy({ $0.isHexDigit }), !fields[1].isEmpty else { return nil }
            return CommitRow(hash: fields[0], shortHash: fields[1], subject: fields[2], when: fields[3])
        }
    }

    static func collectHistory(_ request: HistoryRequest) -> HistoryResult {
        let result = repositoryCommand(request.project, ["log", "-40", "--pretty=format:%H%x1f%h%x1f%s%x1f%cr"])
        guard result.code == 0 else {
            if result.output.contains("does not have any commits yet") {
                return HistoryResult(rows: [], error: nil)
            }
            return HistoryResult(rows: [], error: repositoryReadError(result.output, fallback: "Git history could not be read."))
        }
        var rows = parseHistory(result.output)
        if !result.output.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && rows.isEmpty {
            return HistoryResult(rows: [], error: "Git returned an unreadable commit list.")
        }
        if let hash = request.selectedHash, !rows.contains(where: { $0.hash == hash || $0.shortHash == hash }) {
            guard (4...64).contains(hash.count), hash.allSatisfy({ $0.isHexDigit }) else {
                return HistoryResult(rows: rows, error: nil, selectionError: "The selected commit identifier is invalid.")
            }
            let selected = repositoryCommand(request.project, ["log", "-1", "--pretty=format:%H%x1f%h%x1f%s%x1f%cr", hash, "--"])
            guard selected.code == 0, let row = parseHistory(selected.output).first else {
                return HistoryResult(rows: rows, error: nil, selectionError: "The selected commit is not available in this project.")
            }
            rows.append(row)
        }
        return HistoryResult(rows: rows, error: nil)
    }

    static func collectDetail(_ request: DetailRequest) -> DetailResult {
        let result = repositoryCommand(request.project, ["show", "--no-ext-diff", "--no-textconv", "--stat", "--patch", "--format=commit %h%nAuthor: %an <%ae>%nDate:   %ad%n%n%s%n%n%b", request.hash, "--"])
        guard result.code == 0 else {
            return DetailResult(text: "", error: repositoryReadError(result.output, fallback: "This commit could not be read."))
        }
        let text = result.output.count > 160_000
            ? String(result.output.prefix(160_000)) + "\n… output truncated …\n"
            : result.output
        return DetailResult(text: text, error: nil)
    }

    override func loadView() {
        let root = NSView()
        let page = makePage(in: root, scrolling: true)
        let pageHeader = makeHeader("Increments", "Committed work, newest first. Select an increment to inspect its changes.")
        page.addArrangedSubview(pageHeader)
        pageHeader.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("Commit"))
        column.title = "Increment"
        column.width = 360
        column.minWidth = 160
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = false
        table.style = .plain
        table.target = self
        table.doubleAction = #selector(openCommit)
        table.toolTip = "Select to inspect · double-click to copy the full hash"
        listScroll.hasVerticalScroller = true
        listScroll.hasHorizontalScroller = false
        listScroll.documentView = table
        // FittingTableScrollView installs its compact default when assigned.
        table.rowHeight = 44
        table.intercellSpacing = .zero
        table.backgroundColor = Theme.layer01
        listScroll.drawsBackground = false
        listScroll.translatesAutoresizingMaskIntoConstraints = false
        listScroll.borderType = .noBorder
        listHeight = listScroll.heightAnchor.constraint(equalToConstant: 110)
        listHeight.priority = .defaultHigh
        listHeight.isActive = true

        listGroup.orientation = .vertical
        listGroup.alignment = .leading
        listGroup.distribution = .fill
        listGroup.spacing = 0
        listGroup.wantsLayer = true
        listGroup.layer?.backgroundColor = Theme.layer01.themeCGColor
        let listHeader = NSView()
        listHeader.translatesAutoresizingMaskIntoConstraints = false
        statusLabel.translatesAutoresizingMaskIntoConstraints = false
        listHeader.addSubview(statusLabel)
        listGroup.addArrangedSubview(listHeader)
        listGroup.addArrangedSubview(listScroll)
        NSLayoutConstraint.activate([
            listHeader.widthAnchor.constraint(equalTo: listGroup.widthAnchor),
            statusLabel.leadingAnchor.constraint(equalTo: listHeader.leadingAnchor, constant: 10),
            statusLabel.trailingAnchor.constraint(equalTo: listHeader.trailingAnchor, constant: -10),
            statusLabel.topAnchor.constraint(equalTo: listHeader.topAnchor, constant: 10),
            statusLabel.bottomAnchor.constraint(equalTo: listHeader.bottomAnchor, constant: -10),
            listScroll.widthAnchor.constraint(equalTo: listGroup.widthAnchor),
        ])
        listGroup.setContentHuggingPriority(.defaultLow, for: .horizontal)
        listWidth = listGroup.widthAnchor.constraint(equalTo: body.widthAnchor, multiplier: 0.34)

        detail.isEditable = false
        detail.isRichText = true
        detail.backgroundColor = Theme.deep
        detail.textContainerInset = NSSize(width: 10, height: 10)
        detail.frame = NSRect(x: 0, y: 0, width: 500, height: 240)
        detail.isVerticallyResizable = true
        detail.isHorizontallyResizable = false
        detail.autoresizingMask = [.width]
        detail.textContainer?.widthTracksTextView = true
        detail.minSize = .zero
        detail.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        detailScroll.hasVerticalScroller = true
        detailScroll.documentView = detail
        detailScroll.drawsBackground = false
        detailScroll.translatesAutoresizingMaskIntoConstraints = false
        detailPanel.translatesAutoresizingMaskIntoConstraints = false
        detailPanel.wantsLayer = true
        detailPanel.layer?.backgroundColor = Theme.deep.themeCGColor
        let detailHeader = NSStackView()
        detailHeader.orientation = .vertical
        detailHeader.alignment = .leading
        detailHeader.spacing = 5
        detailHeader.translatesAutoresizingMaskIntoConstraints = false
        detailTitle.font = Theme.uiFont(13, weight: .semibold)
        detailTitle.textColor = Theme.textBase
        detailTitle.maximumNumberOfLines = 2
        detailTitle.lineBreakMode = .byTruncatingTail
        detailTitle.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        detailHeader.addArrangedSubview(detailTitle)
        detailTitle.widthAnchor.constraint(equalTo: detailHeader.widthAnchor).isActive = true
        let detailControls = NSStackView()
        detailControls.orientation = .horizontal
        detailControls.spacing = 6
        detailControls.addArrangedSubview(hashLabel)
        hashLabel.font = Theme.codeFont(12)
        hashLabel.lineBreakMode = .byTruncatingMiddle
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        detailControls.addArrangedSubview(spacer)
        configureRepositoryButton(previousButton, symbol: "chevron.left", label: "Newer increment", target: self, action: #selector(selectPrevious))
        configureRepositoryButton(nextButton, symbol: "chevron.right", label: "Older increment", target: self, action: #selector(selectNext))
        configureRepositoryButton(copyButton, symbol: "doc.on.doc", label: "Copy commit hash", target: self, action: #selector(copyHash))
        [previousButton, nextButton, copyButton].forEach { detailControls.addArrangedSubview($0); $0.isEnabled = false }
        detailHeader.addArrangedSubview(detailControls)
        detailControls.widthAnchor.constraint(equalTo: detailHeader.widthAnchor).isActive = true
        detailPanel.addSubview(detailHeader)
        detailPanel.addSubview(detailScroll)
        detailState.translatesAutoresizingMaskIntoConstraints = false
        detailPanel.addSubview(detailState)
        let stateStack = NSStackView()
        stateStack.orientation = .vertical
        stateStack.alignment = .centerX
        stateStack.spacing = 9
        stateStack.translatesAutoresizingMaskIntoConstraints = false
        detailSpinner.style = .spinning
        detailSpinner.controlSize = .small
        detailSpinner.isDisplayedWhenStopped = false
        stateStack.addArrangedSubview(detailSpinner)
        detailStateTitle.alignment = .center
        stateStack.addArrangedSubview(detailStateTitle)
        detailStateMessage.font = Theme.uiFont(12)
        detailStateMessage.textColor = Theme.textMuted
        detailStateMessage.alignment = .center
        detailStateMessage.maximumNumberOfLines = 5
        stateStack.addArrangedSubview(detailStateMessage)
        detailState.addSubview(stateStack)
        NSLayoutConstraint.activate([
            detailHeader.topAnchor.constraint(equalTo: detailPanel.topAnchor, constant: 10),
            detailHeader.leadingAnchor.constraint(equalTo: detailPanel.leadingAnchor, constant: 10),
            detailHeader.trailingAnchor.constraint(equalTo: detailPanel.trailingAnchor, constant: -10),
            detailScroll.topAnchor.constraint(equalTo: detailHeader.bottomAnchor, constant: 10),
            detailScroll.leadingAnchor.constraint(equalTo: detailPanel.leadingAnchor),
            detailScroll.trailingAnchor.constraint(equalTo: detailPanel.trailingAnchor),
            detailScroll.bottomAnchor.constraint(equalTo: detailPanel.bottomAnchor),
            detailState.topAnchor.constraint(equalTo: detailScroll.topAnchor),
            detailState.leadingAnchor.constraint(equalTo: detailPanel.leadingAnchor, constant: 16),
            detailState.trailingAnchor.constraint(equalTo: detailPanel.trailingAnchor, constant: -16),
            detailState.bottomAnchor.constraint(equalTo: detailPanel.bottomAnchor),
            stateStack.centerXAnchor.constraint(equalTo: detailState.centerXAnchor),
            stateStack.centerYAnchor.constraint(equalTo: detailState.centerYAnchor),
            stateStack.widthAnchor.constraint(lessThanOrEqualToConstant: 340),
            stateStack.widthAnchor.constraint(lessThanOrEqualTo: detailState.widthAnchor, constant: -16),
            detailStateMessage.widthAnchor.constraint(equalTo: stateStack.widthAnchor),
        ])
        detailMinimumHeight = detailPanel.heightAnchor.constraint(greaterThanOrEqualToConstant: 230)
        detailMinimumHeight.priority = .defaultHigh
        detailMinimumHeight.isActive = true

        body.orientation = .horizontal
        body.alignment = .top
        body.distribution = .fill
        body.spacing = 1
        body.wantsLayer = true
        body.layer?.backgroundColor = Theme.borderMuted.themeCGColor
        body.addArrangedSubview(listGroup)
        body.addArrangedSubview(detailPanel)
        page.addArrangedSubview(body)
        body.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        body.setContentHuggingPriority(.defaultLow, for: .vertical)
        detailBodyHeight = detailPanel.heightAnchor.constraint(equalTo: body.heightAnchor)
        detailBodyHeight.isActive = true
        listBodyHeight = listGroup.heightAnchor.constraint(equalTo: body.heightAnchor)
        listBodyHeight.isActive = true
        listHeight.isActive = false
        pageMinimumHeight = page.heightAnchor.constraint(greaterThanOrEqualTo: root.heightAnchor, constant: -40)
        pageMinimumHeight.priority = .defaultHigh
        pageMinimumHeight.isActive = true
        listWidth.isActive = true
        stackedWidths = [listGroup.widthAnchor.constraint(equalTo: body.widthAnchor), detailPanel.widthAnchor.constraint(equalTo: body.widthAnchor)]
        addBanner(page, key: "increments", text: "Each increment includes its commit message, changed files and diff. Double-click a row to copy its full hash.")
        view = root
        showDetailMessage("Reading committed work from this project…", title: "Loading increments", loading: true)
        refresh()
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        let horizontal = view.bounds.width >= 720
        if (body.orientation == .horizontal) != horizontal {
            listWidth.isActive = false
            detailBodyHeight.isActive = false
            listBodyHeight.isActive = false
            pageMinimumHeight.isActive = false
            NSLayoutConstraint.deactivate(stackedWidths)
            body.orientation = horizontal ? .horizontal : .vertical
            body.alignment = horizontal ? .top : .leading
            listWidth.isActive = horizontal
            detailBodyHeight.isActive = horizontal
            listBodyHeight.isActive = horizontal
            listHeight.isActive = !horizontal
            pageMinimumHeight.isActive = horizontal
            if !horizontal { NSLayoutConstraint.activate(stackedWidths) }
        }
        let visibleRows = min(4, max(2, rows.count))
        listHeight.constant = CGFloat(visibleRows) * (table.rowHeight + table.intercellSpacing.height)
        let available = max(230, view.bounds.height - (horizontal ? 170 : listHeight.constant + 180))
        detailMinimumHeight.constant = rows.isEmpty ? 210 : min(500, available)
        detailTitle.preferredMaxLayoutWidth = max(0, detailPanel.bounds.width - 20)
    }

    override func refresh() {
        precondition(Thread.isMainThread)
        let request = HistoryRequest(project: Prefs.project,
                                     selectedHash: currentProject == Prefs.project ? pendingCommitHash ?? (currentCommit.isEmpty ? nil : currentCommit) : nil)
        if currentProject != request.project {
            currentProject = request.project
            pendingCommitHash = nil
            rows = []
            currentCommit = ""
            loadedDetail = nil
            requestedDetail = nil
            applyingRows = true
            table.reloadData()
            table.deselectAll(nil)
            applyingRows = false
            statusLabel.stringValue = "Loading increments…"
            hashLabel.stringValue = ""
            hashLabel.toolTip = nil
            detailTitle.stringValue = "Committed work"
            detailTitle.toolTip = nil
            showDetailMessage("Reading committed work from this project…", title: "Loading increments", loading: true)
            updateControls()
        }
        historyLoader.request(request)
    }

    private func applyHistory(_ request: HistoryRequest, _ result: HistoryResult) {
        guard request.project == currentProject, request.project == Prefs.project else { return }
        let selected = pendingCommitHash ?? (currentCommit.isEmpty ? nil : currentCommit)
        guard request.selectedHash == selected else {
            historyLoader.request(HistoryRequest(project: request.project, selectedHash: selected))
            return
        }
        if let error = result.error {
            statusLabel.stringValue = "Could not load increments · \(error)"
            statusLabel.toolTip = error
            if rows.isEmpty { showDetailMessage(error, title: "Could not load increments") }
            return
        }
        statusLabel.toolTip = nil
        statusLabel.stringValue = result.rows.isEmpty ? "No committed increments yet" : "\(min(40, result.rows.count)) recent increments" + (result.rows.count > 40 ? " · selected older increment" : "")
        if rows != result.rows {
            let origin = listScroll.contentView.bounds.origin
            let selected = currentCommit
            rows = result.rows
            applyingRows = true
            table.reloadData()
            if let index = rows.firstIndex(where: { $0.hash == selected }) {
                table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            } else if !rows.isEmpty {
                table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            } else {
                table.deselectAll(nil)
            }
            applyingRows = false
            listScroll.contentView.scroll(to: origin)
            listScroll.reflectScrolledClipView(listScroll.contentView)
        }
        listHeight.constant = CGFloat(min(4, max(2, rows.count))) * (table.rowHeight + table.intercellSpacing.height)
        if let hash = pendingCommitHash,
           let index = rows.firstIndex(where: { $0.hash == hash || $0.shortHash == hash }) {
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            table.scrollRowToVisible(index)
            pendingCommitHash = nil
        }
        if let error = result.selectionError {
            pendingCommitHash = nil
            applyingRows = true
            table.deselectAll(nil)
            applyingRows = false
            currentCommit = ""
            loadedDetail = nil
            requestedDetail = nil
            hashLabel.stringValue = ""
            detailTitle.stringValue = "Selected increment unavailable"
            showDetailMessage(error, title: "Could not load this commit")
        } else if rows.isEmpty {
            currentCommit = ""
            loadedDetail = nil
            requestedDetail = nil
            hashLabel.stringValue = ""
            hashLabel.toolTip = nil
            detailTitle.stringValue = "Committed work"
            detailTitle.toolTip = nil
            showDetailMessage("Committed work will appear here once this project has its first increment.", title: "No increments yet")
        } else {
            loadDetail()
        }
        updateControls()
    }

    func loadDetail() {
        let row = table.selectedRow
        guard row >= 0, row < rows.count else {
            currentCommit = ""
            loadedDetail = nil
            requestedDetail = nil
            hashLabel.stringValue = ""
            hashLabel.toolTip = nil
            detailTitle.stringValue = "Select an increment"
            detailTitle.toolTip = nil
            showDetailMessage(rows.isEmpty ? "Committed work will appear here." : "Choose a commit in the list to read its message, files and diff.", title: rows.isEmpty ? "No increments yet" : "Inspect committed work")
            updateControls()
            return
        }
        let selected = rows[row]
        detailTitle.stringValue = selected.subject
        detailTitle.toolTip = selected.subject
        let request = DetailRequest(project: currentProject, hash: selected.hash)
        if currentCommit == selected.hash, loadedDetail == request || requestedDetail == request { updateControls(); return }
        let changed = currentCommit != selected.hash
        currentCommit = selected.hash
        if changed { loadedDetail = nil }
        requestedDetail = request
        hashLabel.stringValue = selected.shortHash
        hashLabel.toolTip = selected.hash
        if changed || loadedDetail == nil { showDetailMessage("Reading the message, changed files and diff…", title: "Loading commit \(selected.shortHash)", loading: true) }
        updateControls()
        detailLoader.request(request)
    }

    func inspectCommit(_ hash: String) {
        _ = view
        if currentProject != Prefs.project { refresh() }
        pendingCommitHash = hash
        if let index = rows.firstIndex(where: { $0.hash == hash || $0.shortHash == hash }) {
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            pendingCommitHash = nil
            loadDetail()
        } else {
            applyingRows = true
            table.deselectAll(nil)
            applyingRows = false
            currentCommit = ""
            showDetailMessage("Reading the selected increment…", title: "Loading commit \(hash)", loading: true)
            refresh()
        }
    }

    private func applyDetail(_ request: DetailRequest, _ result: DetailResult) {
        guard request.project == currentProject, request.project == Prefs.project,
              request.hash == currentCommit else { return }
        if let error = result.error {
            loadedDetail = nil
            requestedDetail = nil
            showDetailMessage(error, title: "Could not load this commit")
            return
        }
        // A commit is immutable; an unchanged refresh leaves its text and scroll intact.
        guard loadedDetail != request else { return }
        loadedDetail = request
        requestedDetail = nil
        detailSpinner.stopAnimation(nil)
        detailState.isHidden = true
        detailScroll.isHidden = false
        detail.textStorage?.setAttributedString(numberedText(result.text, size: 12))
        detail.scrollToBeginningOfDocument(nil)
    }

    private func showDetailMessage(_ text: String, title: String = "Inspect committed work", loading: Bool = false) {
        detailStateTitle.stringValue = title
        detailStateMessage.stringValue = text
        detailStateMessage.toolTip = text
        detailState.isHidden = false
        detailScroll.isHidden = true
        detailSpinner.isHidden = !loading
        if loading { detailSpinner.startAnimation(nil) } else { detailSpinner.stopAnimation(nil) }
    }

    private func updateControls() {
        let selected = table.selectedRow
        let valid = selected >= 0 && selected < rows.count && currentProject == Prefs.project
        copyButton.isEnabled = valid && !currentCommit.isEmpty
        previousButton.isEnabled = valid && selected > 0
        nextButton.isEnabled = valid && selected + 1 < rows.count
    }

    @objc func selectPrevious() { selectRelative(-1) }
    @objc func selectNext() { selectRelative(1) }
    private func selectRelative(_ delta: Int) {
        let row = table.selectedRow + delta
        guard currentProject == Prefs.project, row >= 0, row < rows.count else { return }
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        table.scrollRowToVisible(row)
        loadDetail()
    }
    @objc func copyHash() {
        guard currentProject == Prefs.project, !currentCommit.isEmpty,
              table.selectedRow >= 0, table.selectedRow < rows.count else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(currentCommit, forType: .string)
    }
    @objc func openCommit() { copyHash() }

    final class CommitCell: NSTableCellView {
        let subject = NSTextField(wrappingLabelWithString: "")
        let metadata = headerLabel("", size: 11, color: Theme.textMuted)
        override init(frame frameRect: NSRect) {
            super.init(frame: frameRect)
            subject.font = Theme.uiFont(12, weight: .medium)
            subject.textColor = Theme.textBase
            subject.maximumNumberOfLines = 1
            subject.lineBreakMode = .byTruncatingTail
            subject.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            subject.translatesAutoresizingMaskIntoConstraints = false
            metadata.lineBreakMode = .byTruncatingTail
            metadata.translatesAutoresizingMaskIntoConstraints = false
            addSubview(subject)
            addSubview(metadata)
            textField = subject
            NSLayoutConstraint.activate([
                subject.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
                subject.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
                subject.topAnchor.constraint(equalTo: topAnchor, constant: 5),
                subject.bottomAnchor.constraint(lessThanOrEqualTo: metadata.topAnchor, constant: -3),
                metadata.leadingAnchor.constraint(equalTo: subject.leadingAnchor),
                metadata.trailingAnchor.constraint(equalTo: subject.trailingAnchor),
                metadata.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -5),
            ])
        }
        required init?(coder: NSCoder) { fatalError() }
        func set(_ row: CommitRow) {
            subject.stringValue = row.subject
            metadata.stringValue = "\(row.shortHash)  ·  \(row.when)"
            toolTip = row.subject
            setAccessibilityLabel("\(row.subject), \(row.shortHash), \(row.when)")
        }
        override func layout() {
            subject.preferredMaxLayoutWidth = max(0, bounds.width - 20)
            super.layout()
        }
    }
}

extension IncrementsPane: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { RepositoryRowView() }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard row >= 0, row < rows.count else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("IncrementCommitCell")
        let cell = tableView.makeView(withIdentifier: identifier, owner: nil) as? CommitCell ?? CommitCell(frame: .zero)
        cell.identifier = identifier
        cell.set(rows[row])
        return cell
    }
    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !applyingRows else { return }
        pendingCommitHash = nil
        loadDetail()
    }
}

final class ProcessesPane: Pane {
    struct ProcessRow: Equatable {
        let pid: String
        let cpu: String
        let mem: String
        let elapsed: String
        let command: String
    }
    struct Request: Equatable { let project: String; let worker: String; var revision: Int = 0 }
    struct Snapshot { let rows: [ProcessRow]; let error: String? }
    let table = NSTableView()
    let scroll = FittingTableScrollView()
    let infoLabel = headerLabel("Loading processes…", size: 12, color: Theme.textMuted)
    let stopButton = NSButton(title: "Stop Selected Process", target: nil, action: nil)
    var rows: [ProcessRow] = []
    private var currentRequest: Request?
    private var applyingRows = false
    private var stopping = false
    private var snapshotValid = false
    private var actionMessage: String?
    private var snapshotRevision = 0
    private var tableHeight: NSLayoutConstraint!
    private lazy var loader: LatestRequestLoader<Request, Snapshot> = {
        let loader = LatestRequestLoader<Request, Snapshot>(label: "agentwatcher.processes", collect: Self.collect)
        loader.apply = { [weak self] request, result in self?.apply(request, result) }
        return loader
    }()

    static func parseProcesses(_ output: String, worker: String) -> [ProcessRow] {
        var patterns = ["caffeinate", "opencode"]
        let workerName = (worker as NSString).lastPathComponent
        if !workerName.isEmpty { patterns.append(workerName) }
        return output.split(separator: "\n").compactMap { line in
            let parts = line.split(maxSplits: 4, omittingEmptySubsequences: true, whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
            guard parts.count == 5, let pid = Int32(parts[0]), pid > 1,
                  Double(parts[1]) != nil, Double(parts[2]) != nil else { return nil }
            let command = parts[4]
            guard patterns.contains(where: { command.contains($0) }),
                  !command.contains("AgentWatcher"), !command.contains("ps -axo"), !command.contains("grep") else { return nil }
            return ProcessRow(pid: parts[0], cpu: parts[1], mem: parts[2], elapsed: parts[3], command: command)
        }
    }

    static func collect(_ request: Request) -> Snapshot {
        let result = run("/bin/ps", ["-axo", "pid=,pcpu=,pmem=,etime=,command="])
        guard result.code == 0 else {
            return Snapshot(rows: [], error: repositoryReadError(result.output, fallback: "The process list could not be read."))
        }
        return Snapshot(rows: parseProcesses(result.output, worker: request.worker), error: nil)
    }

    override func loadView() {
        let root = NSView()
        let page = makePage(in: root, scrolling: true)
        let pageHeader = makeHeader("Processes", "Live worker, OpenCode and keep-awake processes")
        page.addArrangedSubview(pageHeader)
        pageHeader.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        infoLabel.lineBreakMode = .byTruncatingTail
        infoLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        for (title, width) in [("PID", CGFloat(64)), ("CPU %", CGFloat(58)), ("MEM %", CGFloat(58)), ("Elapsed", CGFloat(84)), ("Command", CGFloat(360))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(title))
            column.title = title
            column.width = width
            column.minWidth = title == "Command" ? 100 : 44
            column.resizingMask = title == "Command" ? .autoresizingMask : []
            table.addTableColumn(column)
        }
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.dataSource = self
        table.delegate = self
        table.rowHeight = 28
        table.usesAlternatingRowBackgroundColors = false
        table.style = .plain
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.documentView = table
        table.intercellSpacing = .zero
        table.backgroundColor = Theme.layer01
        scroll.drawsBackground = false
        stopButton.target = self
        stopButton.action = #selector(killSelected)
        stopButton.bezelStyle = .rounded
        stopButton.controlSize = .small
        stopButton.font = Theme.uiFont(12)
        stopButton.toolTip = "Send SIGTERM to the selected process"
        stopButton.isEnabled = false
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let controls = NSStackView(views: [infoLabel, spacer, stopButton])
        controls.orientation = .horizontal
        controls.alignment = .centerY
        controls.spacing = 10
        page.addArrangedSubview(controls)
        controls.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        page.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        tableHeight = scroll.heightAnchor.constraint(equalToConstant: 86)
        tableHeight.priority = .defaultHigh
        tableHeight.isActive = true
        addBanner(page, key: "processes", text: "Select a process to stop it cleanly with SIGTERM. CPU and memory values update automatically; the selected process stays selected while it is running.")
        view = root
        refresh()
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        guard table.tableColumns.count == 5 else { return }
        let width = scroll.contentView.bounds.width
        let fixed: [CGFloat] = [64, 58, 58, 84]
        for index in 0..<4 { table.tableColumns[index].width = fixed[index] }
        table.tableColumns[4].width = max(100, width - fixed.reduce(0, +))
    }

    override func refresh() {
        precondition(Thread.isMainThread)
        let request = Request(project: Prefs.project, worker: Prefs.worker, revision: snapshotRevision)
        if currentRequest != request {
            currentRequest = request
            rows = []
            snapshotValid = false
            actionMessage = nil
            applyingRows = true
            table.reloadData()
            table.deselectAll(nil)
            applyingRows = false
            infoLabel.stringValue = "Loading processes…"
            updateStopButton()
        }
        loader.request(request)
    }

    private func apply(_ request: Request, _ snapshot: Snapshot) {
        guard currentRequest == request, request.project == Prefs.project, request.worker == Prefs.worker else { return }
        if let error = snapshot.error {
            snapshotValid = false
            infoLabel.stringValue = "Could not load processes · \(error)"
            infoLabel.toolTip = error
            // Stale process information must never leave Stop enabled.
            stopButton.isEnabled = false
            return
        }
        infoLabel.toolTip = nil
        snapshotValid = true
        if !stopping {
            infoLabel.stringValue = actionMessage ?? (snapshot.rows.isEmpty ? "No agent processes running" : "\(snapshot.rows.count) agent processes")
        }
        if rows != snapshot.rows {
            let selected = table.selectedRow >= 0 && table.selectedRow < rows.count ? rows[table.selectedRow] : nil
            let origin = scroll.contentView.bounds.origin
            rows = snapshot.rows
            applyingRows = true
            table.reloadData()
            if let selected, let index = rows.firstIndex(where: { $0.pid == selected.pid && $0.command == selected.command }) {
                table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            } else { table.deselectAll(nil) }
            applyingRows = false
            scroll.contentView.scroll(to: origin)
            scroll.reflectScrolledClipView(scroll.contentView)
        }
        tableHeight.constant = CGFloat(min(14, max(2, rows.count))) * table.rowHeight + 30
        updateStopButton()
    }

    private func updateStopButton() {
        let row = table.selectedRow
        stopButton.isEnabled = snapshotValid && !stopping && row >= 0 && row < rows.count
            && currentRequest?.project == Prefs.project && currentRequest?.worker == Prefs.worker
    }

    @objc func killSelected() {
        let row = table.selectedRow
        guard snapshotValid, !stopping, row >= 0, row < rows.count, let request = currentRequest,
              request.project == Prefs.project, request.worker == Prefs.worker else { return }
        let process = rows[row]
        stopping = true
        snapshotValid = false
        actionMessage = nil
        infoLabel.stringValue = "Stopping process \(process.pid)…"
        updateStopButton()
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            // Recheck identity so an exited process cannot target a reused PID.
            let identity = run("/bin/ps", ["-p", process.pid, "-o", "command="])
            let liveCommand = identity.output.trimmingCharacters(in: .whitespacesAndNewlines)
            let result: (code: Int32, output: String)
            if identity.code != 0 || liveCommand != process.command {
                result = (-1, "The selected process exited or changed. Refresh and select it again.")
            } else {
                result = run("/bin/kill", ["-TERM", process.pid])
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.stopping = false
                guard self.currentRequest == request,
                      request.project == Prefs.project, request.worker == Prefs.worker else { self.updateStopButton(); return }
                if result.code != 0 {
                    self.actionMessage = "Could not stop process · \(repositoryReadError(result.output, fallback: "SIGTERM failed."))"
                    self.infoLabel.stringValue = self.actionMessage ?? "Could not stop process"
                    self.infoLabel.toolTip = result.output
                } else {
                    self.actionMessage = "Sent SIGTERM to process \(process.pid)"
                    self.infoLabel.stringValue = self.actionMessage ?? "SIGTERM sent"
                }
                // Force a snapshot collected after SIGTERM, even if an earlier
                // periodic read is still active for the same project/worker.
                self.snapshotValid = false
                self.snapshotRevision += 1
                let next = Request(project: request.project, worker: request.worker, revision: self.snapshotRevision)
                self.currentRequest = next
                self.updateStopButton()
                self.loader.request(next)
            }
        }
    }
}

extension ProcessesPane: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { RepositoryRowView() }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard row >= 0, row < rows.count else { return nil }
        let value: String
        switch tableColumn?.identifier.rawValue {
        case "PID": value = rows[row].pid
        case "CPU %": value = rows[row].cpu
        case "MEM %": value = rows[row].mem
        case "Elapsed": value = rows[row].elapsed
        default: value = rows[row].command
        }
        return repositoryTableCell(in: tableView, column: tableColumn, value: value, size: 12)
    }
    func tableViewSelectionDidChange(_ notification: Notification) {
        if !applyingRows {
            actionMessage = nil
            updateStopButton()
        }
    }
}

private func repositoryReadError(_ output: String, fallback: String) -> String {
    let value = output.trimmingCharacters(in: .whitespacesAndNewlines)
    return value.isEmpty ? fallback : String(value.prefix(360))
}

private func configureRepositoryButton(_ button: NSButton, symbol: String, label: String, target: AnyObject, action: Selector) {
    button.title = ""
    button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
    button.isBordered = false
    button.contentTintColor = Theme.textMuted
    button.toolTip = label
    button.setAccessibilityLabel(label)
    button.target = target
    button.action = action
    button.controlSize = .small
    button.widthAnchor.constraint(equalToConstant: 24).isActive = true
    button.heightAnchor.constraint(equalToConstant: 24).isActive = true
}

private final class RepositoryRowView: NSTableRowView {
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
    override func drawSelection(in dirtyRect: NSRect) {
        guard selectionHighlightStyle != .none else { return }
        (isEmphasized ? Theme.layer03 : Theme.layer02).setFill()
        bounds.intersection(dirtyRect).fill()
    }
}

private func repositoryTableCell(in table: NSTableView, column: NSTableColumn?, value: String, size: CGFloat) -> NSTableCellView {
    let identifier = NSUserInterfaceItemIdentifier("RepositoryCell." + (column?.identifier.rawValue ?? "value"))
    if let cell = table.makeView(withIdentifier: identifier, owner: nil) as? NSTableCellView {
        cell.textField?.stringValue = value
        cell.toolTip = value
        return cell
    }
    let cell = NSTableCellView()
    cell.identifier = identifier
    let field = NSTextField(labelWithString: value)
    field.font = Theme.codeFont(size)
    field.textColor = Theme.textBase
    field.lineBreakMode = .byTruncatingTail
    field.translatesAutoresizingMaskIntoConstraints = false
    cell.addSubview(field)
    cell.textField = field
    cell.toolTip = value
    NSLayoutConstraint.activate([
        field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
        field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
        field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
    ])
    return cell
}
