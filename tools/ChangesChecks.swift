// Executed by verify_changes.py against the actual application types.
func check(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else { fatalError("FAIL: " + message) }
}

func pump(until predicate: () -> Bool) {
    let deadline = Date().addingTimeInterval(5)
    while !predicate() && Date() < deadline {
        RunLoop.main.run(until: Date().addingTimeInterval(0.005))
    }
    check(predicate(), "asynchronous check timed out")
}

final class CollectionProbe {
    private let lock = NSLock()
    private var items: [String] = []
    private var active = 0
    private var peak = 0
    func begin(_ input: String) {
        lock.lock(); defer { lock.unlock() }
        items.append(input)
        active += 1
        peak = max(peak, active)
    }
    func end() { lock.lock(); active -= 1; lock.unlock() }
    var state: ([String], Int) {
        lock.lock(); defer { lock.unlock() }
        return (items, peak)
    }
}

let probe = CollectionProbe()
let blockedRead = DispatchSemaphore(value: 0)
let loader = LatestRequestLoader<String, String>(label: "ChangesChecks.slow-read") { input in
    check(!Thread.isMainThread, "blocking collection ran on main")
    probe.begin(input)
    if input == "old-project" { blockedRead.wait() }
    probe.end()
    return input
}
var applied: [String] = []
loader.apply = { _, value in
    check(Thread.isMainThread, "UI publication ran off main")
    applied.append(value)
}
let began = Date()
loader.request("old-project")
let requestTime = Date().timeIntervalSince(began)
pump { probe.state.0 == ["old-project"] }
var heartbeat = 0
let heartbeatTimer = Timer.scheduledTimer(withTimeInterval: 0.01, repeats: true) { _ in heartbeat += 1 }
pump { heartbeat >= 5 }
for _ in 0..<100 { loader.request("old-project") }
loader.request("superseded-project")
loader.request("latest-project")
blockedRead.signal()
pump { applied == ["latest-project"] }
heartbeatTimer.invalidate()
check(probe.state.0 == ["old-project", "latest-project"], "queued obsolete or duplicate work")
check(probe.state.1 == 1, "collection overlapped")
check(requestTime < 0.1, "request blocked the caller")
print("PASS: single-flight refresh, latest result wins, main heartbeat continues (request \(String(format: "%.3f", requestTime * 1000)) ms)")

let reentrant = LatestRequestLoader<Int, Int>(label: "ChangesChecks.reentrant") { $0 }
var reentrantValues: [Int] = []
reentrant.apply = { _, value in
    reentrantValues.append(value)
    if value == 1 { reentrant.request(2) }
}
reentrant.request(1)
pump { reentrantValues == [1, 2] }
print("PASS: a refresh requested during publication completes")

let repo = CommandLine.arguments[1]
let linked = CommandLine.arguments[3]
for project in [repo, linked] {
    for arguments in [["status", "--porcelain=v1", "-z", "--no-renames"], ["diff", "--no-ext-diff", "--numstat", "-z", "HEAD"], ["log", "-1", "--format=%H"], ["branch", "--show-current"], ["diff", "HEAD", "--", "tracked.txt"]] {
        let original = run("/usr/bin/git", ["--no-optional-locks", "-C", project] + arguments)
        let routed = repositoryCommand(project, arguments)
        check(routed.code == original.code && routed.output == original.output, "repository routing changed \(arguments) for \(project)")
    }
}
check(repositoryCommand(linked, ["branch", "--show-current"]).output.contains("fixture-linked"), "linked worktree used main HEAD/index")
print("PASS: explicit Git routing matches discovery, including relative linked-worktree pointers")
let snapshot = ChangesPane.collectSnapshot(project: repo)
check(snapshot.error == nil, "fixture snapshot failed")
check(!snapshot.branch.contains("fatal:"), "missing upstream displayed as a branch")
func file(_ path: String) -> ChangesPane.ChangedFile {
    guard let file = snapshot.files.first(where: { $0.path == path }) else { fatalError("Missing fixture file: " + path) }
    return file
}
check(file("staged.txt").adds == "1", "staged count missing")
check(file("tracked.txt").adds == "1", "unstaged count missing")
check(file("space → \"quote\".txt").adds == "1", "quoted Unicode path corrupted")
check(file("untracked\nline.txt").status == "??", "newline path corrupted")
check(file("rename-old.txt").status == "D", "rename source missing")
check(file("rename-new.txt").status == "A", "rename destination missing")
for (path, line) in [("staged.txt", "+staged-line"), ("tracked.txt", "+unstaged-line"), ("untracked\nline.txt", "+new-file-line")] {
    let diff = ChangesPane.collectDiff(.init(project: repo, file: file(path)))
    check(!diff.error && diff.text.contains(line), "wrong diff for " + path)
}
let linkDiff = ChangesPane.collectDiff(.init(project: repo, file: file("broken-link")))
check(!linkDiff.error && linkDiff.text.contains("+missing-target"), "broken symlink diff missing")
let unborn = CommandLine.arguments[2]
let first = ChangesPane.collectSnapshot(project: unborn)
check(first.error == nil && first.files.first?.adds == "2", "unborn repository omitted edits made after staging")
let firstDiff = ChangesPane.collectDiff(.init(project: unborn, file: first.files[0]))
check(!firstDiff.error && firstDiff.text.contains("+first-line") && firstDiff.text.contains("+after-staging"), "unborn diff omitted edits made after staging")
check(ChangesPane.collectSnapshot(project: repo + "/missing").error != nil, "failed refresh masquerades as clean")
print("PASS: staged/unstaged/untracked diffs, Unicode/newline paths, renames, unborn repository and read failure")

// Load the actual AppKit hierarchy: the compiler cannot catch constraints
// activated before their views have a common ancestor.
let cocoa = NSApplication.shared
UserDefaults.standard.setVolatileDomain(["project": repo], forName: UserDefaults.argumentDomain)
let changesPane = ChangesPane()
_ = changesPane.view
changesPane.refresh()
pump { !changesPane.allFiles.isEmpty }
changesPane.filterField.stringValue = "no-such-fixture-file"
changesPane.filterChanged()
check(changesPane.diffHeader.stringValue == "No matching files", "filter empty state claims a clean repository")
check(!changesPane.prevButton.isEnabled && !changesPane.nextButton.isEnabled, "empty navigation remains enabled")
changesPane.filterField.stringValue = "staged.txt"
changesPane.filterChanged()
pump { changesPane.diffView.string.contains("+staged-line") }
check(changesPane.files.count == 1, "filter did not narrow loaded files")

UserDefaults.standard.setVolatileDomain(["project": (repo as NSString).deletingLastPathComponent], forName: UserDefaults.argumentDomain)
let errorPane = ChangesPane()
_ = errorPane.view
errorPane.refresh()
pump { errorPane.statsLabel.toolTip != nil }
errorPane.filterField.stringValue = "anything"
errorPane.filterChanged()
check(errorPane.branchLabel.stringValue == "Changes unavailable", "failed refresh remains labeled loading")
check(errorPane.diffView.string.contains("Could not read changes"), "filter erased the read error")
print("PASS: AppKit controller loads, filtering/empty buttons and persistent refresh errors")

let fixtureLog = """
[2026-10-13 12:00:00] loop start
[2026-10-13 12:00:01] starting run
captured output
[2026-10-13 12:01:00] run rc=0
[2026-10-13 12:01:01] session: ses_fixture
[2026-10-13 12:01:02] loop end
"""
let activityCards = ActivityPane.parseCards(text: fixtureLog)
check(activityCards.first(where: { $0.title == "Agent run" })?.sessionID == "ses_fixture", "finished run lost its separately timestamped session ID")
check(SessionsPaneData.currentRun(in: nil).sessionID.isEmpty, "missing log fabricated a session")
check(SessionsPaneData.currentRun(in: fixtureLog).sessionID == "ses_fixture", "latest logged session missing")
let workerPattern = try! NSRegularExpression(pattern: AgentStatus.workerPattern("/tmp/a worker/loop.sh"))
func matchesWorker(_ command: String) -> Bool { workerPattern.firstMatch(in: command, range: NSRange(command.startIndex..<command.endIndex, in: command)) != nil }
check(matchesWorker("/bin/bash /tmp/a worker/loop.sh"), "actual worker command did not match")
check(!matchesWorker("python3 -c \"/tmp/a worker/loop.sh\""), "quoted configuration text looked like a running worker")
check(!matchesWorker("/usr/bin/vim /tmp/a worker/loop.sh"), "editor was mistaken for a worker")
check(!matchesWorker("/bin/cat /tmp/a worker/loop.sh"), "file reader was mistaken for a worker")
check(!matchesWorker("/bin/bash /other/loop.sh"), "another project's worker matched")
let largeLogURL = URL(fileURLWithPath: repo).deletingLastPathComponent().appendingPathComponent("large-worker-log.txt")
let largeLog = fixtureLog + "\n" + String(repeating: "unstructured output\n", count: 12000)
try! largeLog.write(to: largeLogURL, atomically: true, encoding: .utf8)
check(try! SessionsPaneData.readCurrentRun(from: largeLogURL).sessionID == "ses_fixture", "session metadata behind >200 KB output was lost")
let unavailableLog = try! SessionsPaneData.readCurrentRun(from: largeLogURL, initialReadSize: 1024, maximumReadSize: 2048)
check(unavailableLog.sessionID.isEmpty && unavailableLog.title.contains("unavailable"), "capped log read falsely reported missing session")
let sessionRows = try! SessionsPaneData.parseSessionsJSON("[{\"id\":\"ses_fixture\",\"title\":\"Fixture session\",\"time\":{\"updated\":1791854400000}}]")
check(sessionRows.count == 1 && sessionRows[0].title == "Fixture session", "JSON session list parser failed")
let privateLink = URL(string: "http://127.0.0.1:1234/auth/connect/test-secret")!
check(SessionsPaneData.origin(for: privateLink) == "http://127.0.0.1:1234", "connection label exposes token")
check(!SessionsPaneData.safeError(privateLink.absoluteString, code: 1).contains("test-secret"), "connection error exposes token")
let hash = String(repeating: "a", count: 40)
check(IncrementsPane.parseHistory(hash + "\u{1f}aaaaaaa\u{1f}A | subject\u{1f}now").first?.subject == "A | subject", "commit title parsing failed")
check(ProcessesPane.parseProcesses("123 1.5 0.2 00:10 /bin/opencode --project a b", worker: "/tmp/worker.sh").first?.command.hasSuffix("a b") == true, "process parser lost command spacing")
print("PASS: activity session binding, session JSON/current state/redaction, commit and process parsers")

let historyRepo = CommandLine.arguments[4]
let oldestHash = repositoryCommand(historyRepo, ["rev-list", "--max-parents=0", "HEAD"]).output.trimmingCharacters(in: .whitespacesAndNewlines)
let recentHistory = IncrementsPane.collectHistory(.init(project: historyRepo))
check(recentHistory.rows.count == 40 && !recentHistory.rows.contains(where: { $0.hash == oldestHash }), "long-history fixture does not exceed recent list")
let selectedHistory = IncrementsPane.collectHistory(.init(project: historyRepo, selectedHash: oldestHash))
check(selectedHistory.rows.count == 41 && selectedHistory.rows.last?.hash == oldestHash, "older chart commit was not fetched within a bounded list")
check(IncrementsPane.collectHistory(.init(project: historyRepo, selectedHash: "--all")).selectionError != nil, "invalid selected hash was accepted")
UserDefaults.standard.setVolatileDomain(["project": historyRepo, "worker": "/usr/bin/false", "autoStart": false], forName: UserDefaults.argumentDomain)
let historyPane = IncrementsPane()
historyPane.inspectCommit(oldestHash)
pump { historyPane.currentCommit == oldestHash && historyPane.detail.string.contains("Fixture history 0") }
historyPane.refresh()
historyPane.statusLabel.stringValue = "Waiting for polling publication"
pump { historyPane.statusLabel.stringValue != "Waiting for polling publication" && historyPane.rows.count == 41 && historyPane.table.selectedRow == 40 }
check(historyPane.currentCommit == oldestHash, "polling replaced the selected older commit")
let newestHash = historyPane.rows[0].hash
historyPane.inspectCommit(String(repeating: "f", count: 40))
historyPane.inspectCommit(newestHash)
historyPane.statusLabel.stringValue = "Waiting for changed selection"
pump { historyPane.statusLabel.stringValue != "Waiting for changed selection" && historyPane.detail.string.contains("Fixture history 44") }
check(historyPane.currentCommit == newestHash && historyPane.table.selectedRow == 0, "late failed chart lookup replaced a newer explicit selection")
check(historyPane.table.rowHeight == 44, "increment list is not compact")
print("PASS: chart links beyond newest 40 commits open the exact full detail and survive polling")

func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
UserDefaults.standard.setVolatileDomain(["project": repo, "worker": "/usr/bin/false", "state": (repo as NSString).deletingLastPathComponent, "autoStart": false], forName: UserDefaults.argumentDomain)
for (width, height) in [(480.0, 548.0), (628.0, 548.0), (812.0, 748.0), (848.0, 548.0), (1028.0, 748.0), (1450.0, 900.0)] {
    let panes: [Pane] = [OverviewPane(), ActivityPane(), ProcessesPane(), IncrementsPane(), ChangesPane(), SessionsPane(), SettingsPane()]
    for pane in panes {
        let hostWindow = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: height), styleMask: [.titled], backing: .buffered, defer: false)
        hostWindow.isReleasedWhenClosed = false
        hostWindow.contentMinSize = NSSize(width: width, height: height)
        hostWindow.contentMaxSize = NSSize(width: width, height: height)
        let host = NSView(frame: NSRect(x: 0, y: 0, width: width, height: height))
        hostWindow.contentView = host
        NSLayoutConstraint.activate([
            host.widthAnchor.constraint(equalToConstant: width),
            host.heightAnchor.constraint(equalToConstant: height),
        ])
        pane.view.translatesAutoresizingMaskIntoConstraints = false
        pane.view.frame = host.bounds
        host.addSubview(pane.view)
        NSLayoutConstraint.activate([
            pane.view.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            pane.view.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            pane.view.topAnchor.constraint(equalTo: host.topAnchor),
            pane.view.bottomAnchor.constraint(equalTo: host.bottomAnchor),
        ])
        host.updateConstraintsForSubtreeIfNeeded()
        host.needsLayout = true
        host.layoutSubtreeIfNeeded()
        pane.view.layoutSubtreeIfNeeded()
        check(abs(pane.view.frame.width - width) < 1, "pane does not fill host window in \(type(of: pane)): \(pane.view.frame), host \(host.frame)")
        for scroll in descendants(pane.view).compactMap({ $0 as? FittingTableScrollView }) {
            scroll.needsLayout = true
            scroll.layoutSubtreeIfNeeded()
            guard let table = scroll.documentView as? NSTableView, !table.tableColumns.isEmpty else { continue }
            check(abs(table.frame.width - scroll.contentSize.width) < 1, "table document overflow in \(type(of: pane)) at \(width): \(table.frame.width) vs \(scroll.contentSize.width)")
            check(table.rect(ofColumn: table.tableColumns.count - 1).maxX <= scroll.contentSize.width + 1, "last column clipped in \(type(of: pane)) at \(width)")
        }
        for scroll in descendants(pane.view).compactMap({ $0 as? WrappingTextScrollView }) {
            scroll.needsLayout = true
            scroll.layoutSubtreeIfNeeded()
            if let text = scroll.documentView as? NSTextView {
                check(abs(text.frame.width - scroll.contentSize.width) < 1, "text document wider than visible pane in \(type(of: pane))")
            }
        }
        if let activity = pane as? ActivityPane {
            check(abs(activity.listScroll.documentView!.frame.width - activity.listScroll.contentSize.width) < 1, "activity feed width does not track clip view")
            check(abs(activity.contextScroll.documentView!.frame.width - activity.contextScroll.contentSize.width) < 1, "context width does not track clip view")
            let columns = activity.listScroll.superview!
            check(abs(columns.frame.width - (width - 32)) < 1, "activity columns do not fill page")
            let detail = columns.subviews.first { $0 !== activity.listScroll && !($0 is WorkspaceDivider) }!
            check(abs(detail.frame.maxX - columns.bounds.maxX) < 1, "activity detail leaves unused horizontal space")
            check(detail.frame.width > min(180, width * 0.5), "activity detail remains a narrow strip")
            activity.tabPicker.selectedSegment = 1
            activity.contextStack.addArrangedSubview(headerLabel("stale context", size: 12))
            activity.renderRight()
            check(!activity.contextStack.arrangedSubviews.compactMap { ($0 as? NSTextField)?.stringValue }.contains("stale context"), "empty Context retained stale messages")
            check(!activity.contextStack.arrangedSubviews.isEmpty, "empty Context is blank")
        }
        if let settings = pane as? SettingsPane {
            check(settings.projectField.frame.width > min(500, width - 130), "settings path field collapsed at \(width)")
        }
        if let overview = pane as? OverviewPane {
            check(overview.chart.frame.height == 190, "overview chart stretched")
            check(!overview.startButton.isEnabled && !overview.pauseButton.isEnabled && !overview.stopButton.isEnabled, "worker actions enabled before status is known")
        }
        hostWindow.close()
    }
}
print("PASS: all seven AppKit panes load at compact/wide sizes, table columns fit and activity documents track their clip views")

let navigationWindow = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1450, height: 900), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
navigationWindow.isReleasedWhenClosed = false
navigationWindow.contentMinSize = NSSize(width: 848, height: 548)
let navigationHost = NSView(frame: NSRect(x: 0, y: 0, width: 1450, height: 900))
navigationWindow.contentView = navigationHost
navigationWindow.orderBack(nil)
let originalWindowSize = navigationWindow.frame.size
for pane in [OverviewPane(), ActivityPane(), ProcessesPane(), IncrementsPane(), ChangesPane(), SessionsPane(), SettingsPane()] as [Pane] {
    navigationHost.subviews.forEach { $0.removeFromSuperview() }
    pane.view.translatesAutoresizingMaskIntoConstraints = false
    navigationHost.addSubview(pane.view)
    NSLayoutConstraint.activate([
        pane.view.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor),
        pane.view.trailingAnchor.constraint(equalTo: navigationHost.trailingAnchor),
        pane.view.topAnchor.constraint(equalTo: navigationHost.topAnchor),
        pane.view.bottomAnchor.constraint(equalTo: navigationHost.bottomAnchor),
    ])
    navigationHost.layoutSubtreeIfNeeded()
    navigationWindow.layoutIfNeeded()
    navigationWindow.displayIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.02))
    check(navigationWindow.frame.size == originalWindowSize, "navigation resized window in \(type(of: pane)): \(navigationWindow.frame.size)")
}
navigationWindow.close()
print("PASS: navigation across all panes preserves the chosen window size")


let chart = LineChartView(frame: NSRect(x: 0, y: 0, width: 200, height: 100))
chart.series = [.init(name: "Increments", color: Theme.primary, values: [0, 4, 0]), .init(name: "Runs", color: Theme.accent, values: [1, 0, 0])]
var chartSelection: (Int, String)?
chart.onSelect = { index, series, _ in chartSelection = (index, series) }
let hit = chart.selection(for: NSPoint(x: 100, y: 90))
check(hit?.bucketIndex == 1 && hit?.seriesName == "Increments", "spike hit did not select matching line/time")
chart.selectBucket(0, seriesName: "Runs")
check(chartSelection?.0 == 0 && chartSelection?.1 == "Runs", "run line did not activate inspector callback")
chart.series[0].visible = false
check(chart.selection(for: NSPoint(x: 100, y: 90))?.seriesName == "Runs", "hidden series still receives clicks")
chart.selectBucket(999)
check(chart.selectedBucketIndex == 2, "chart index did not clamp")
chart.series = []
check(chart.selection(for: .zero) == nil && chart.selectBucket(0) == nil, "empty chart still activates")
let chartNow = Date(timeIntervalSince1970: 1791854400)
for range in 0...2 {
    let timeline = ChartTimeline(updated: chartNow, range: range)
    check(timeline.bucketIndex(for: chartNow) == timeline.count - 1, "latest chart bucket incorrect")
    check(timeline.bucketIndex(for: chartNow.addingTimeInterval(-timeline.seconds)) == timeline.count - 2, "chart boundary double-counted")
    check(timeline.bucketIndex(for: chartNow.addingTimeInterval(1)) == nil, "future event plotted in history")
    check(timeline.bucketIndex(for: chartNow.addingTimeInterval(-timeline.seconds * Double(timeline.count))) == nil, "event outside visible range plotted")
}
let overviewInspection = OverviewPane()
_ = overviewInspection.view
let req = OverviewPane.Request(project: Prefs.project, worker: Prefs.worker, stateDir: Prefs.stateDir, revision: 0)
let endedRun = ActivityPane.RunCard(key: "completed", title: "Agent run", subtitle: "finished", date: chartNow.addingTimeInterval(-1800), status: true, detail: "fixture output", sessionID: "", finishedAt: chartNow.addingTimeInterval(-300))
overviewInspection.applySnapshot(.init(request: req, status: AgentStatus(), commits: [(date: chartNow.addingTimeInterval(-200), hash: "abc1234", subject: "Fixture increment")], cards: [endedRun], updated: chartNow, error: nil))
let inspected = overviewInspection.inspectionItems(bucket: 95)
check(inspected.contains { $0.identifier == "abc1234" && $0.kind == .commit }, "spike inspector omitted commit")
check(inspected.contains { $0.identifier == "completed" && $0.kind == .run }, "spike inspector omitted run active during bucket")
check(!overviewInspection.inspectionItems(bucket: 90).contains { $0.identifier == "completed" }, "run linked before it began")
let interruptedLog = "[2026-10-13 10:00:00] starting run\n[2026-10-13 10:10:00] loop end\n[2026-10-13 12:00:00] starting run"
let interruptedCards = ActivityPane.parseCards(text: interruptedLog)
check(interruptedCards.first?.finishedAt != nil && interruptedCards.first?.subtitle == "interrupted", "stopped run remained active forever")
let interruptedTimeline = ChartTimeline(updated: makeLogFormatter().date(from: "2026-10-13 12:30:00")!, range: 0)
let stopped = interruptedCards.first!
check(stopped.finishedAt! < interruptedTimeline.interval(for: 95).start, "stopped run overlaps later spike")
var inspectedID = ""
let inspector = ChartInspectorController(title: "Fixture time", items: inspected) { inspectedID = $0.identifier }
let inspectorButtons = descendants(inspector.view).compactMap { $0 as? NSButton }.filter { $0.tag >= 0 && !$0.attributedTitle.string.isEmpty }
check(inspectorButtons.count == 2, "inspector rows are not all clickable")
inspectorButtons.first?.performClick(nil)
check(!inspectedID.isEmpty, "inspector row did not activate details")
let runInspection = ActivityPane()
_ = runInspection.view
runInspection.cards = (0..<5000).map { .init(key: "run-\($0)", title: "Agent run", subtitle: "finished", date: chartNow, status: true, detail: "fixture", sessionID: "") }
runInspection.inspectRun("run-0")
check(runInspection.selectedKey == "run-0" && runInspection.listStack.arrangedSubviews.contains { $0.identifier?.rawValue == "run-0" }, "inspecting old run did not select/reveal exact row")
check(runInspection.listStack.arrangedSubviews.filter { $0.identifier != nil }.count <= 20, "old-run inspection rendered unbounded history")
print("PASS: both chart lines, time boundaries, spike commit/active-run links, every inspector row, and old-run navigation")

let transcript = SessionTranscriptController(sessionID: "ses_fixture", title: "Fixture", loadAutomatically: false)
transcript.build(messages: [
    ["type": "user", "text": "Please inspect this fixture."],
    ["type": "assistant", "content": [["type": "text", "text": "Fixture reply."],
      ["type": "tool", "name": "read", "state": ["status": "failed", "input": ["path": "/fixture"], "error": "Fixture read failed"]]]]
], error: 0)
check(transcript.parts.count == 3 && transcript.parts[2].body.contains("Fixture read failed"), "transcript omitted a step or tool error")
check(transcript.expanded.contains("u0"), "user message not initially expanded")
let hide = NSButton()
hide.identifier = NSUserInterfaceItemIdentifier("u0")
transcript.togglePart(hide)
check(!transcript.expanded.contains("u0"), "Hide cannot collapse a default-open message")
transcript.togglePart(hide)
check(transcript.expanded.contains("u0"), "Show cannot reopen a message")
transcript.window.close()
print("PASS: transcript steps/tool failures and Hide/Show interaction")

runTelemetryChecks(scratchURL: URL(fileURLWithPath: repo).deletingLastPathComponent())
runAppearanceChecks()

runDockSizingChecks()
