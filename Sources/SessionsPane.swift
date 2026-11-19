import AppKit

// Parsing and log interpretation stay independent of AppKit so malformed CLI
// output, empty logs, and pairing links can be checked without starting a worker.
enum SessionsPaneData {
    struct SessionRow: Equatable {
        let id: String
        let title: String
        let updated: String
    }

    struct CurrentRun: Equatable {
        let sessionID: String
        let title: String
        let detail: String
    }

    enum ParseError: LocalizedError {
        case invalidJSON, unsupportedFormat
        var errorDescription: String? {
            switch self {
            case .invalidJSON: return "OpenCode returned invalid session data."
            case .unsupportedFormat: return "OpenCode returned an unsupported session format."
            }
        }
    }

    static func parseSessionsJSON(_ output: String) throws -> [SessionRow] {
        guard let data = output.data(using: .utf8),
              let value = try? JSONSerialization.jsonObject(with: data) else { throw ParseError.invalidJSON }
        let entries: [[String: Any]]
        if let array = value as? [[String: Any]] {
            entries = array
        } else if let object = value as? [String: Any],
                  let array = (object["sessions"] ?? object["data"] ?? object["items"]) as? [[String: Any]] {
            entries = array
        } else {
            throw ParseError.unsupportedFormat
        }
        var seen = Set<String>()
        let rows = entries.compactMap { entry -> SessionRow? in
            guard let id = (entry["id"] ?? entry["sessionID"]) as? String,
                  isSessionID(id), seen.insert(id).inserted else { return nil }
            let title = (entry["title"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            let time = entry["time"] as? [String: Any]
            let updated = time?["updated"] ?? entry["updatedAt"] ?? entry["updated"]
            return SessionRow(id: id, title: title?.isEmpty == false ? title! : "Untitled session",
                              updated: formattedDate(updated))
        }
        if !entries.isEmpty && rows.isEmpty { throw ParseError.unsupportedFormat }
        return rows
    }

    static func isSessionID(_ value: String) -> Bool {
        value.range(of: "^ses_[A-Za-z0-9_-]+$", options: .regularExpression) != nil
    }

    private static func formattedDate(_ value: Any?) -> String {
        var date: Date?
        if let number = value as? NSNumber {
            let timestamp = number.doubleValue
            date = Date(timeIntervalSince1970: timestamp > 100_000_000_000 ? timestamp / 1_000 : timestamp)
        } else if let text = value as? String {
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            date = formatter.date(from: text)
            if date == nil {
                formatter.formatOptions = [.withInternetDateTime]
                date = formatter.date(from: text)
            }
        }
        guard let date else { return "Unknown" }
        return DateFormatter.localizedString(from: date, dateStyle: .short, timeStyle: .short)
    }

    private struct RunMarkers {
        var start = -1
        var completion: (index: Int, exitCode: String)?
        var session: (index: Int, id: String, stamp: String)?
        var ended = -1
        var hasRunBoundary: Bool { start >= 0 || completion != nil || session != nil }
    }

    private static let runMarkerPattern = try! NSRegularExpression(
        pattern: #"^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\] (starting run|run rc=(-?\d+)|session: (ses_[A-Za-z0-9_-]+)|loop end)\s*$"#)

    private static func runMarkers(in log: String) -> RunMarkers {
        var markers = RunMarkers()
        for (index, line) in log.split(separator: "\n").enumerated() where line.first == "[" {
            let text = String(line)
            guard let match = runMarkerPattern.firstMatch(in: text, range: NSRange(text.startIndex..<text.endIndex, in: text)) else { continue }
            func group(_ number: Int) -> String? {
                guard let range = Range(match.range(at: number), in: text) else { return nil }
                return String(text[range])
            }
            if let id = group(4) {
                markers.session = (index, id, group(1) ?? "")
            } else if let exitCode = group(3) {
                markers.completion = (index, exitCode)
            } else if group(2) == "starting run" {
                markers.start = index
            } else {
                markers.ended = index
            }
        }
        return markers
    }

    private static func currentRun(markers: RunMarkers, empty: Bool) -> CurrentRun {
        if empty { return CurrentRun(sessionID: "", title: "No run logged yet", detail: "The worker log is empty.") }
        let sessionIndex = markers.session?.index ?? -1
        let completionIndex = markers.completion?.index ?? -1
        if max(markers.start, completionIndex) > sessionIndex {
            if let completion = markers.completion, completion.index > markers.start {
                return CurrentRun(sessionID: "", title: "Run finished — no session ID",
                                  detail: "The last run exited with code \(completion.exitCode). No session ID was recorded.")
            }
            if markers.ended > markers.start {
                return CurrentRun(sessionID: "", title: "Worker stopped",
                                  detail: "No session ID was recorded for the last run.")
            }
            return CurrentRun(sessionID: "", title: "Run in progress",
                              detail: "The worker has not recorded this run’s session ID yet.")
        }
        if let session = markers.session {
            return CurrentRun(sessionID: session.id, title: "Latest logged session",
                              detail: "Recorded \(session.stamp) · \(session.id)")
        }
        if markers.ended >= 0 {
            return CurrentRun(sessionID: "", title: "Worker stopped", detail: "No session ID was recorded for the last run.")
        }
        return CurrentRun(sessionID: "", title: "No session ID recorded",
                          detail: "The worker log does not contain a valid OpenCode session ID.")
    }

    static func currentRun(in log: String?) -> CurrentRun {
        guard let log else {
            return CurrentRun(sessionID: "", title: "No run logged yet",
                              detail: "The worker has not written a log yet.")
        }
        return currentRun(markers: runMarkers(in: log), empty: log.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }

    // A worker appends tool output after its timestamped session metadata. Read
    // backward until the newest run boundary is visible, rather than treating
    // the last 128 KiB of unstamped output as proof that no session exists.
    static func readCurrentRun(from url: URL, initialReadSize: Int = 131_072,
                               maximumReadSize: Int = 4 * 1_024 * 1_024) throws -> CurrentRun {
        let file = try FileHandle(forReadingFrom: url)
        defer { try? file.close() }
        let end = try file.seekToEnd()
        let limit = UInt64(min(4 * 1_024 * 1_024, max(1, maximumReadSize)))
        var size = min(end, limit, UInt64(max(1, initialReadSize)))
        var lower = end
        var data = Data()
        while true {
            let nextLower = end - size
            try file.seek(toOffset: nextLower)
            let prefix = try file.read(upToCount: Int(lower - nextLower)) ?? Data()
            data.insert(contentsOf: prefix, at: 0)
            lower = nextLower
            // Ignore a first line cut by the byte boundary. It becomes eligible
            // on the next expansion, once its complete timestamp is available.
            let complete = lower == 0 ? data[...] : data.firstIndex(of: 0x0A)
                .map { data[data.index(after: $0)...] } ?? data[data.endIndex...]
            let log = String(decoding: complete, as: UTF8.self)
            let markers = runMarkers(in: log)
            if markers.hasRunBoundary || lower == 0 {
                return currentRun(markers: markers, empty: log.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            if size >= limit {
                return CurrentRun(sessionID: "", title: "Recent run metadata unavailable",
                                  detail: "Run metadata was not found within the most recent \(limit / 1_024) KiB of the worker log.")
            }
            size = min(end, limit, size * 2)
        }
    }

    static func pairingURL(in output: String) -> URL? {
        guard let pattern = try? NSRegularExpression(pattern: #"https?://[^\s<>\"']+"#) else { return nil }
        let range = NSRange(output.startIndex..<output.endIndex, in: output)
        for match in pattern.matches(in: output, range: range) {
            guard let swiftRange = Range(match.range, in: output) else { continue }
            let text = String(output[swiftRange]).trimmingCharacters(in: CharacterSet(charactersIn: ").,;"))
            if let url = URL(string: text), let origin = origin(for: url), !origin.isEmpty { return url }
        }
        return nil
    }

    static func origin(for url: URL) -> String? {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let scheme = components.scheme, ["http", "https"].contains(scheme.lowercased()),
              let host = components.host, !host.isEmpty else { return nil }
        var origin = URLComponents()
        origin.scheme = scheme
        origin.host = host
        origin.port = components.port
        return origin.string
    }

    static func safeError(_ output: String, code: Int32) -> String {
        let withoutANSI = output.replacingOccurrences(of: #"\u001B\[[0-9;]*[A-Za-z]"#,
                                                       with: "", options: .regularExpression)
        let lines = withoutANSI.split(separator: "\n").map(String.init)
        let first = lines.first { !$0.trimmingCharacters(in: .whitespaces).isEmpty } ?? "OpenCode exited with code \(code)."
        guard let pattern = try? NSRegularExpression(pattern: #"https?://[^\s<>\"']+"#) else {
            return "OpenCode could not load sessions (exit \(code))."
        }
        let safe = NSMutableString(string: first)
        for match in pattern.matches(in: first, range: NSRange(first.startIndex..<first.endIndex, in: first)).reversed() {
            let raw = safe.substring(with: match.range)
            let safeOrigin = URL(string: raw).flatMap { origin(for: $0) } ?? "[service URL]"
            safe.replaceCharacters(in: match.range, with: safeOrigin)
        }
        return String((safe as String).prefix(240))
    }
}

// MARK: - Sessions pane

final class SessionsPane: Pane {
    private struct Context: Equatable {
        let project: String
        let logFile: String
    }
    private struct PairRequest: Equatable {
        let project: String
        let generation: Int
    }
    private enum ListResult {
        case success([SessionsPaneData.SessionRow])
        case failure(String)
    }
    private enum ListState {
        case idle, loading, loaded, failed(String)
    }

    let table = NSTableView()
    let search = NSSearchField()
    let stateLabel = headerLabel("Live service not connected", size: 12, weight: .medium, color: Theme.textMuted)
    let serverLabel = headerLabel("Connect to open the live OpenCode service.", size: 11, color: Theme.textFaint)
    let currentTitle = headerLabel("Checking current run…", size: 14, weight: .semibold)
    let currentDetail = headerLabel("", size: 11, color: Theme.textMuted)
    private let connectButton = NSButton(title: "Connect", target: nil, action: nil)
    private let openLiveButton = NSButton(title: "Open live", target: nil, action: nil)
    private let copyLiveButton = NSButton(title: "Copy link", target: nil, action: nil)
    private let currentTranscript = NSButton(title: "Open transcript", target: nil, action: nil)
    private let currentCopy = NSButton(title: "Copy session ID", target: nil, action: nil)
    private let selectedTranscript = NSButton(title: "Open transcript", target: nil, action: nil)
    private let selectedCopy = NSButton(title: "Copy ID", target: nil, action: nil)
    private let reloadButton = NSButton(title: "Refresh", target: nil, action: nil)
    private let countLabel = headerLabel("", size: 11, color: Theme.textMuted)
    private let listStatus = headerLabel("", size: 11, color: Theme.textMuted)
    private let listProgress = NSProgressIndicator()
    private let emptyTitle = headerLabel("Loading sessions…", size: 14, weight: .semibold)
    private let emptyDetail = headerLabel("Sessions from this project will appear here.", size: 12, color: Theme.textMuted)
    private let emptyState = NSStackView()
    private var allRows: [SessionsPaneData.SessionRow] = []
    private var rows: [SessionsPaneData.SessionRow] = []
    private var currentSessionID = ""
    private var context: Context?
    private var listState: ListState = .idle
    private var lastListLoad = Date.distantPast
    private var pairGeneration = 0
    private var pairedURL: URL?
    private var isPairing = false

    private lazy var sessionLoader = LatestRequestLoader<String, ListResult>(label: "AgentWatcher.sessions") { project in
        let result = run("\(home)/.opencode/bin/opencode", ["session", "list", "--format", "json", "--max-count", "100"], cwd: project)
        guard result.code == 0 else { return .failure(SessionsPaneData.safeError(result.output, code: result.code)) }
        do { return .success(try SessionsPaneData.parseSessionsJSON(result.output)) }
        catch { return .failure(error.localizedDescription) }
    }
    private lazy var currentLoader = LatestRequestLoader<Context, SessionsPaneData.CurrentRun>(label: "AgentWatcher.current-session") { context in
        guard FileManager.default.fileExists(atPath: context.logFile) else { return SessionsPaneData.currentRun(in: nil) }
        do {
            return try SessionsPaneData.readCurrentRun(from: URL(fileURLWithPath: context.logFile))
        } catch {
            return SessionsPaneData.CurrentRun(sessionID: "", title: "Worker log unavailable",
                                               detail: "The worker log could not be read. Check its location in Settings.")
        }
    }
    private lazy var pairLoader = LatestRequestLoader<PairRequest, (code: Int32, url: URL?)>(label: "AgentWatcher.pair-session") { request in
        let result = run("\(home)/.opencode/bin/opencode", ["pair"], cwd: request.project)
        return (result.code, result.code == 0 ? SessionsPaneData.pairingURL(in: result.output) : nil)
    }

    override func loadView() {
        precondition(Thread.isMainThread)
        let root = NSView()
        let stack = makePage(in: root)
        stack.spacing = 8

        let heading = makeHeader("Sessions", "OpenCode conversations for the selected project")
        addFullWidth(heading, to: stack)

        let service = NSStackView()
        service.orientation = .vertical
        service.alignment = .leading
        service.spacing = 5
        let serviceRow = row()
        stateLabel.lineBreakMode = .byTruncatingTail
        stateLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        serviceRow.addArrangedSubview(stateLabel)
        serverLabel.lineBreakMode = .byTruncatingTail
        serverLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        serviceRow.addArrangedSubview(serverLabel)
        serviceRow.addArrangedSubview(spacer())
        configure(connectButton, #selector(reconnect))
        serviceRow.addArrangedSubview(connectButton)
        configure(openLiveButton, #selector(openLive))
        configure(copyLiveButton, #selector(copyLive))
        serviceRow.addArrangedSubview(openLiveButton)
        serviceRow.addArrangedSubview(copyLiveButton)
        openLiveButton.isHidden = true
        copyLiveButton.isHidden = true
        addFullWidth(serviceRow, to: service)
        addFullWidth(service, to: stack)
        addSeparator(to: stack)

        let current = NSStackView()
        current.orientation = .vertical
        current.alignment = .leading
        current.spacing = 5
        let currentActions = row()
        currentActions.addArrangedSubview(headerLabel("Current run", size: 11, weight: .semibold, color: Theme.textMuted))
        currentTitle.lineBreakMode = .byTruncatingTail
        currentTitle.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        currentActions.addArrangedSubview(currentTitle)
        currentActions.addArrangedSubview(spacer())
        configure(currentTranscript, #selector(openCurrent))
        configure(currentCopy, #selector(copyCurrent))
        currentTranscript.isEnabled = false
        currentCopy.isEnabled = false
        currentActions.addArrangedSubview(currentTranscript)
        currentActions.addArrangedSubview(currentCopy)
        addFullWidth(currentActions, to: current)
        currentDetail.lineBreakMode = .byTruncatingMiddle
        addFullWidth(currentDetail, to: current)
        current.setContentHuggingPriority(.required, for: .vertical)
        addFullWidth(current, to: stack)
        addSeparator(to: stack)

        let listHeading = row()
        listHeading.addArrangedSubview(headerLabel("Recorded sessions", size: 12, weight: .semibold))
        listHeading.addArrangedSubview(spacer())
        listHeading.addArrangedSubview(countLabel)
        addFullWidth(listHeading, to: stack)
        let controls = row()
        search.placeholderString = "Filter by title or session ID"
        search.target = self
        search.action = #selector(filterChanged)
        search.sendsSearchStringImmediately = true
        search.setContentHuggingPriority(.defaultLow, for: .horizontal)
        search.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        controls.addArrangedSubview(search)
        listProgress.style = .spinning
        listProgress.controlSize = .small
        listProgress.isDisplayedWhenStopped = false
        controls.addArrangedSubview(listProgress)
        configure(reloadButton, #selector(refreshSessions))
        controls.addArrangedSubview(reloadButton)
        addFullWidth(controls, to: stack)

        for (title, width, minimum) in [("Title", CGFloat(340), CGFloat(140)), ("Updated", CGFloat(145), CGFloat(110)),
                                        ("Session", CGFloat(180), CGFloat(90))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(title))
            column.title = title
            column.width = width
            column.minWidth = minimum
            column.resizingMask = .autoresizingMask
            table.addTableColumn(column)
        }
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = true
        table.style = .inset
        table.rowHeight = 30
        table.target = self
        table.doubleAction = #selector(openSession)
        table.toolTip = "Select a session and open its transcript, or double-click a row."
        let scroll = FittingTableScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.borderType = .bezelBorder
        scroll.documentView = table
        let listContainer = NSView()
        listContainer.translatesAutoresizingMaskIntoConstraints = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        listContainer.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: listContainer.topAnchor),
            scroll.leadingAnchor.constraint(equalTo: listContainer.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: listContainer.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: listContainer.bottomAnchor),
        ])
        emptyState.orientation = .vertical
        emptyState.alignment = .centerX
        emptyState.spacing = 7
        emptyTitle.alignment = .center
        emptyDetail.alignment = .center
        emptyDetail.maximumNumberOfLines = 3
        emptyDetail.lineBreakMode = .byWordWrapping
        emptyState.addArrangedSubview(emptyTitle)
        emptyState.addArrangedSubview(emptyDetail)
        emptyState.translatesAutoresizingMaskIntoConstraints = false
        listContainer.addSubview(emptyState)
        NSLayoutConstraint.activate([
            emptyState.centerXAnchor.constraint(equalTo: listContainer.centerXAnchor),
            emptyState.centerYAnchor.constraint(equalTo: listContainer.centerYAnchor, constant: 8),
            emptyState.leadingAnchor.constraint(greaterThanOrEqualTo: listContainer.leadingAnchor, constant: 20),
            emptyState.trailingAnchor.constraint(lessThanOrEqualTo: listContainer.trailingAnchor, constant: -20),
            emptyDetail.widthAnchor.constraint(lessThanOrEqualTo: listContainer.widthAnchor, constant: -40),
        ])
        addFullWidth(listContainer, to: stack)
        let listMinimum = listContainer.heightAnchor.constraint(greaterThanOrEqualToConstant: 100)
        listMinimum.priority = .defaultHigh
        listMinimum.isActive = true
        listContainer.setContentHuggingPriority(.defaultLow, for: .vertical)
        listContainer.setContentCompressionResistancePriority(.defaultLow, for: .vertical)

        let actions = row()
        configure(selectedTranscript, #selector(openSession))
        configure(selectedCopy, #selector(copySelected))
        selectedTranscript.isEnabled = false
        selectedCopy.isEnabled = false
        actions.addArrangedSubview(selectedTranscript)
        actions.addArrangedSubview(selectedCopy)
        actions.addArrangedSubview(spacer())
        addFullWidth(actions, to: stack)
        listStatus.lineBreakMode = .byTruncatingTail
        addFullWidth(listStatus, to: stack)
        view = root

        sessionLoader.apply = { [weak self] project, result in
            guard let self, self.context?.project == project, Prefs.project == project else { return }
            switch result {
            case .success(let rows): self.allRows = rows; self.listState = .loaded
            case .failure(let message): self.listState = .failed(message)
            }
            self.listProgress.stopAnimation(nil)
            self.reloadButton.isEnabled = true
            self.applyFilter()
        }
        currentLoader.apply = { [weak self] request, current in
            guard let self, self.context == request, Prefs.project == request.project else { return }
            self.currentSessionID = current.sessionID
            self.currentTitle.stringValue = current.title
            self.currentDetail.stringValue = current.detail
            self.currentDetail.toolTip = current.detail
            self.currentTranscript.isEnabled = !current.sessionID.isEmpty
            self.currentCopy.isEnabled = !current.sessionID.isEmpty
        }
        pairLoader.apply = { [weak self] request, result in
            guard let self, self.context?.project == request.project, Prefs.project == request.project,
                  request.generation == self.pairGeneration else { return }
            self.isPairing = false
            self.connectButton.isEnabled = true
            if let url = result.url, let origin = SessionsPaneData.origin(for: url), result.code == 0 {
                self.pairedURL = url
                self.stateLabel.stringValue = "Live service connected"
                self.stateLabel.textColor = Theme.success
                self.serverLabel.stringValue = origin
                self.connectButton.title = "Reconnect"
                self.openLiveButton.isHidden = false
                self.copyLiveButton.isHidden = false
            } else {
                self.pairedURL = nil
                self.stateLabel.stringValue = "Could not connect to live service"
                self.stateLabel.textColor = Theme.warning
                self.serverLabel.stringValue = "Try Connect again. Your recorded sessions remain available."
                self.connectButton.title = "Connect"
                self.openLiveButton.isHidden = true
                self.copyLiveButton.isHidden = true
            }
        }
        updateListState()
    }

    private func row() -> NSStackView {
        let row = NSStackView()
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 8
        row.setContentHuggingPriority(.required, for: .vertical)
        return row
    }

    private func spacer() -> NSView {
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        return spacer
    }

    private func configure(_ button: NSButton, _ action: Selector) {
        button.bezelStyle = .rounded
        button.controlSize = .small
        button.target = self
        button.action = action
        button.setContentHuggingPriority(.required, for: .horizontal)
    }

    private func addFullWidth(_ child: NSView, to parent: NSStackView) {
        parent.addArrangedSubview(child)
        child.translatesAutoresizingMaskIntoConstraints = false
        child.widthAnchor.constraint(equalTo: parent.widthAnchor).isActive = true
    }

    private func addSeparator(to stack: NSStackView) {
        let separator = NSBox()
        separator.boxType = .separator
        addFullWidth(separator, to: stack)
    }

    override func refresh() {
        precondition(Thread.isMainThread)
        guard isViewLoaded else { return }
        let nextContext = Context(project: Prefs.project, logFile: Prefs.logFile)
        if context != nextContext {
            context = nextContext
            allRows = []
            rows = []
            currentSessionID = ""
            pairedURL = nil
            isPairing = false
            pairGeneration += 1
            stateLabel.stringValue = "Live service not connected"
            stateLabel.textColor = Theme.textMuted
            serverLabel.stringValue = "Connect to open the live OpenCode service."
            connectButton.title = "Connect"
            connectButton.isEnabled = true
            openLiveButton.isHidden = true
            copyLiveButton.isHidden = true
            currentTitle.stringValue = "Checking current run…"
            currentDetail.stringValue = ""
            currentDetail.toolTip = nil
            currentTranscript.isEnabled = false
            currentCopy.isEnabled = false
            lastListLoad = .distantPast
            listState = .idle
            table.deselectAll(nil)
            table.reloadData()
            updateSelection()
            updateListState()
        }
        currentLoader.request(nextContext)
        if Date().timeIntervalSince(lastListLoad) > 15 { loadSessions() }
    }

    private func loadSessions() {
        guard let context else { return }
        lastListLoad = Date()
        listState = .loading
        listProgress.startAnimation(nil)
        reloadButton.isEnabled = false
        updateListState()
        sessionLoader.request(context.project)
    }

    private func applyFilter() {
        let selectedID = table.selectedRow >= 0 && table.selectedRow < rows.count ? rows[table.selectedRow].id : nil
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        rows = query.isEmpty ? allRows : allRows.filter {
            $0.title.localizedCaseInsensitiveContains(query) || $0.id.localizedCaseInsensitiveContains(query)
        }
        table.reloadData()
        if let selectedID, let row = rows.firstIndex(where: { $0.id == selectedID }) {
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        } else {
            table.deselectAll(nil)
        }
        updateSelection()
        updateListState()
    }

    private func updateSelection() {
        let selected = table.selectedRow >= 0 && table.selectedRow < rows.count
        selectedTranscript.isEnabled = selected
        selectedCopy.isEnabled = selected
    }

    private func updateListState() {
        countLabel.stringValue = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? "\(allRows.count) sessions" : "\(rows.count) of \(allRows.count) sessions"
        emptyState.isHidden = !rows.isEmpty
        emptyTitle.textColor = Theme.textBase
        switch listState {
        case .idle, .loading:
            emptyTitle.stringValue = "Loading sessions…"
            emptyDetail.stringValue = "Reading recorded sessions for this project."
            listStatus.stringValue = allRows.isEmpty ? "Loading recorded sessions…" : "Refreshing recorded sessions…"
        case .failed(let message):
            emptyTitle.stringValue = "Could not load sessions"
            emptyTitle.textColor = Theme.warning
            emptyDetail.stringValue = message
            listStatus.stringValue = "Refresh failed: \(message)"
        case .loaded:
            let filtered = !search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            emptyTitle.stringValue = filtered ? "No matching sessions" : "No sessions recorded"
            emptyDetail.stringValue = filtered ? "Try another title or session ID." : "OpenCode sessions for this project will appear after a run."
            listStatus.stringValue = rows.isEmpty ? (filtered ? "Clear the filter to see all recorded sessions." : "No recorded sessions for this project.")
                : "Select a session to open its transcript. Double-click also opens it."
        }
        listStatus.textColor = {
            if case .failed = listState { return Theme.warning }
            return Theme.textMuted
        }()
    }

    @objc func refreshSessions() { refresh(); loadSessions() }

    @objc func reconnect() {
        refresh()
        guard let context, !isPairing else { return }
        isPairing = true
        pairedURL = nil
        pairGeneration += 1
        connectButton.isEnabled = false
        stateLabel.stringValue = "Connecting to live service…"
        stateLabel.textColor = Theme.textMuted
        serverLabel.stringValue = "Waiting for OpenCode. Recorded sessions remain available."
        openLiveButton.isHidden = true
        copyLiveButton.isHidden = true
        pairLoader.request(PairRequest(project: context.project, generation: pairGeneration))
    }

    @objc func openLive() {
        guard let pairedURL else { return }
        NSWorkspace.shared.open(pairedURL)
    }

    @objc func copyLive() {
        guard let pairedURL else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(pairedURL.absoluteString, forType: .string)
    }

    @objc func openCurrent() {
        guard !currentSessionID.isEmpty else { return }
        SessionTranscriptController.open(sessionID: currentSessionID, title: "Loop session \(currentSessionID.prefix(12))")
    }

    @objc func copyCurrent() {
        guard !currentSessionID.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(currentSessionID, forType: .string)
    }

    @objc func openSession() {
        let index = table.selectedRow
        guard index >= 0, index < rows.count else { return }
        SessionTranscriptController.open(sessionID: rows[index].id, title: rows[index].title)
    }

    @objc func copySelected() {
        let index = table.selectedRow
        guard index >= 0, index < rows.count else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(rows[index].id, forType: .string)
    }

    @objc func filterChanged() { applyFilter() }
}

extension SessionsPane: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableViewSelectionDidChange(_ notification: Notification) { updateSelection() }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard row >= 0, row < rows.count else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("SessionCell.\(tableColumn?.identifier.rawValue ?? "Title")")
        let cell: NSTableCellView
        if let reusable = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView {
            cell = reusable
        } else {
            cell = NSTableCellView()
            cell.identifier = identifier
            let field = NSTextField(labelWithString: "")
            field.lineBreakMode = .byTruncatingTail
            field.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(field)
            cell.textField = field
            NSLayoutConstraint.activate([
                field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
                field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
                field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        guard let field = cell.textField else { return cell }
        field.font = tableColumn?.identifier.rawValue == "Session" ? Theme.codeFont(11) : .systemFont(ofSize: 12)
        field.textColor = tableColumn?.identifier.rawValue == "Title" ? Theme.textBase : Theme.textMuted
        switch tableColumn?.identifier.rawValue {
        case "Title": field.stringValue = rows[row].title
        case "Updated": field.stringValue = rows[row].updated
        default: field.stringValue = rows[row].id
        }
        cell.toolTip = field.stringValue
        return cell
    }
}
