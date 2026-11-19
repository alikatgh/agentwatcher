import AppKit

// Called by the integrated verification harness against application types.
func runAgentWorkChecks() {
    func require(_ condition: @autoclosure () -> Bool, _ message: String) { precondition(condition(), "Agent work: " + message) }
    let base = Date(timeIntervalSince1970: 1_800_000_000)
    var snapshot = TelemetrySnapshot(sessionID: "ses_work", title: "Read recorded work", project: "/fixture", collectedAt: base)
    snapshot.taskPrompt = String(repeating: "A long task that must remain within its viewport. ", count: 32)
    snapshot.parentSessionID = "ses_parent"
    snapshot.context = [.init(id: "context", title: "Recorded user context", value: "Exact retained prompt", detailJSON: "{\"retained\":true}")]
    snapshot.messages = (0..<80).map { index in
        .init(id: "message-\(index)", type: index.isMultiple(of: 2) ? "user" : "assistant", text: "Message \(index)", timestamp: base.addingTimeInterval(Double(index * 2)), rawJSON: "{\"index\":\(index)}")
    }
    snapshot.messages.append(.init(id: "untimed", type: "system", text: "Timestamp absent", timestamp: nil, rawJSON: "{}"))
    snapshot.toolEvents = (0..<75).map { index in
        .init(id: "tool-\(index)", name: "exec_command", status: "Completed", inputJSON: "{\"command\":\"command-\(index)\"}", output: "Full result \(index)\nTAIL-\(index)", error: nil,
              startedAt: base.addingTimeInterval(Double(index * 2 + 1)), completedAt: base.addingTimeInterval(Double(index * 2 + 2)), duration: 1,
              rawJSON: "{\"turn\":2,\"step\":\(index),\"agent\":\"worker\",\"toolSchema\":{\"name\":\"exec_command\",\"properties\":{\"command\":{\"type\":\"string\"}}}}")
    }
    snapshot.subagents = (0..<160).map { index in
        .init(id: "launch-\(index)", title: "Inspect module \(index)", sessionID: "ses_child_\(index)", agent: "explorer", status: "Completed", prompt: "Exact task \(index)\nTASK-TAIL-\(index)", rawJSON: "{\"launch\":\(index)}")
    }
    let rows = AgentWorkPresentation.trajectory(snapshot)
    require(rows.count == 156, "all tool events and messages must survive; no first-40 cap")
    require(rows.last?.time == nil, "untimed records must remain visible after timed records")
    require(zip(rows.compactMap(\.time), rows.compactMap(\.time).dropFirst()).allSatisfy { $0 <= $1 }, "trajectory chronology")
    require(rows.contains { $0.searchableText.contains("TAIL-74") }, "full tool output must be searchable")
    let tool = AgentWorkPresentation.tool(snapshot.toolEvents[74], snapshot: snapshot)
    require(tool.sections.map(\.title) == ["Summary", "Hierarchy", "Payload", "Result", "Schema", "Timing", "Recorded data"], "tool detail sections")
    require(tool.fullText.contains("TAIL-74") && tool.fullText.contains("command-74") && tool.fullText.contains("properties"), "complete result, payload and schema")
    require(AgentWorkPresentation.child(snapshot.subagents[159], snapshot: snapshot).fullText.contains("TASK-TAIL-159"), "last child's entire recorded task")
    require(AgentWorkPresentation.parent(snapshot).linkedSessionID == "ses_parent", "exact parent navigation")
    require(AgentWorkPresentation.validSessionID("ses_child_159") != nil && AgentWorkPresentation.validSessionID("session-recorded") != nil, "known source session ID formats")
    require(AgentWorkPresentation.validSessionID("019c6e27-e55b-73d1-87d8-4e01f1f75043") != nil, "Codex UUID links")
    require(AgentWorkPresentation.validSessionID("../../private") == nil, "invalid linked paths cannot navigate")
    require(AgentWorkPresentation.context(snapshot).fullText.contains("Exact retained prompt"), "context retained")

    _ = NSApplication.shared
    let controller = AgentWorkController()
    controller.update(snapshot)
    controller.view.frame = NSRect(x: 0, y: 0, width: 900, height: 700)
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 700), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
    window.contentViewController = controller
    controller.view.layoutSubtreeIfNeeded()
    func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
    let views = descendants(controller.view)
    guard let table = views.compactMap({ $0 as? NSTableView }).first else { preconditionFailure("Agent work table missing") }
    require(table.numberOfRows == 155, "default trajectory focuses on user, assistant and tool work")
    guard let recordScope = views.compactMap({ $0 as? NSButton }).first(where: { $0.title == "All records" }) else {
        preconditionFailure("All records control missing")
    }
    recordScope.performClick(nil)
    require(table.numberOfRows == 156, "All records restores the infrastructure message")
    recordScope.performClick(nil)
    guard let search = views.compactMap({ $0 as? NSSearchField }).first else { preconditionFailure("Trajectory search missing") }
    search.stringValue = "Timestamp absent"
    controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: search))
    require(table.numberOfRows == 1, "search still finds infrastructure records with All records disabled")
    search.stringValue = ""
    controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: search))
    require(table.numberOfRows == 155, "clearing search restores focused work records")
    let nodeButtons = views.compactMap { $0 as? NSButton }.filter { $0.accessibilityLabel()?.hasPrefix("SUBAGENT") == true }
    require(!nodeButtons.isEmpty && nodeButtons.count < 12, "large hierarchy uses bounded native node views")
    guard let graphScroll = views.compactMap({ $0 as? NSScrollView }).first(where: { $0.hasHorizontalScroller && !($0.documentView is NSTableView) }), let graph = graphScroll.documentView else { preconditionFailure("Agent graph missing") }
    require(graph.frame.width > 30_000 && graphScroll.frame.height <= 260, "full large hierarchy fits a bounded horizontal viewport")
    graphScroll.contentView.scroll(to: NSPoint(x: max(0, graph.frame.width - graphScroll.contentSize.width), y: 0))
    graphScroll.reflectScrolledClipView(graphScroll.contentView)
    graph.layoutSubtreeIfNeeded()
    require(descendants(graph).compactMap { $0 as? NSButton }.contains { $0.accessibilityLabel()?.contains("Inspect module 159") == true }, "last subagent is reachable")

    var openedSession: String?
    var inspected: AgentWorkDetailController?
    controller.onOpenSession = { openedSession = $0 }
    controller.onInspectDetails = { inspected = $0 as? AgentWorkDetailController }
    descendants(graph).compactMap { $0 as? NSButton }.first { $0.accessibilityLabel()?.contains("Inspect module 159") == true }?.performClick(nil)
    require(openedSession == "ses_child_159" && inspected != nil, "child selection inspects launch and follows valid recorded link")
    let inspector = inspected ?? AgentWorkDetailController(tool)
    inspector.update(tool)
    inspector.view.frame = NSRect(x: 0, y: 0, width: 340, height: 650)
    let inspectorWindow = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 340, height: 650), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
    inspectorWindow.contentViewController = inspector
    inspector.view.layoutSubtreeIfNeeded()
    require(inspector.preferredContentSize == .zero, "inspector must not impose a main-window size")
    require(inspector.view.frame.width <= 341, "recorded detail fits a 340pt inspector")
    require(descendants(inspector.view).compactMap { $0 as? NSTextView }.first?.string.contains("TAIL-74") == true, "complete selectable result in native detail")
    inspectorWindow.contentViewController = nil
    window.contentViewController = nil
    print("PASS: agent-work full chronology/search/details, parent-child links, 160-child bounded canvas and 340pt native inspector")
}
