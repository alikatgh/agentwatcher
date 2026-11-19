// Executed against actual application types by the regression-check harness.
func runDockSizingChecks() {
    func check(_ value: @autoclosure () -> Bool, _ label: String) {
        if !value() { fatalError("FAIL: " + label) }
    }
    let _ = NSApplication.shared

    // Use a hidden window to exercise AppKit's controller/window size
    // negotiation without launching the application or its background work.
    for initialWidth: CGFloat in [1000, 1300, 1950] {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: initialWidth, height: 800),
                              styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 900, height: 600)
        let root = InspectorRootView()
        let content = NSView()
        let host = NSView()
        let overview = OverviewPane()
        let overviewView = overview.view
        overviewView.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(overviewView)
        NSLayoutConstraint.activate([
            overviewView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            overviewView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            overviewView.topAnchor.constraint(equalTo: content.topAnchor),
            overviewView.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
        content.translatesAutoresizingMaskIntoConstraints = false
        host.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(content)
        root.addSubview(host)
        let width = host.widthAnchor.constraint(equalToConstant: 0)
        let trailing = content.trailingAnchor.constraint(equalTo: root.trailingAnchor)
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 52),
            content.topAnchor.constraint(equalTo: root.topAnchor),
            content.bottomAnchor.constraint(equalTo: root.bottomAnchor), trailing,
            host.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            host.topAnchor.constraint(equalTo: root.topAnchor),
            host.bottomAnchor.constraint(equalTo: root.bottomAnchor), width,
        ])
        var visible = false
        func update() {
            if host.isHidden == visible { host.isHidden = !visible }
            let desired = min(440, max(340, root.bounds.width * 0.36))
            let target = visible ? desired : 0
            if abs(width.constant - target) > 0.5 { width.constant = target }
            let inset: CGFloat = visible && root.bounds.width >= 1200 ? -desired : 0
            if abs(trailing.constant - inset) > 0.5 { trailing.constant = inset }
        }
        root.onLayout = { update() }
        host.isHidden = true
        window.contentView = root
        func settle() {
            for _ in 0..<5 {
                root.layoutSubtreeIfNeeded()
                RunLoop.main.run(until: Date().addingTimeInterval(0.01))
            }
        }
        settle()
        let originalFrame = window.frame
        let longValue = String(repeating: "Recorded investigation evidence and exact next steps for the project. ", count: 12)
        let longPath = "/Users/fixture/Documents/projects/" + String(repeating: "deeply-nested-workspace/", count: 20)
        var snapshot = TelemetrySnapshot(sessionID: "session-" + longValue, title: longValue,
                                         project: longPath, collectedAt: Date())
        snapshot.taskPrompt = longValue
        snapshot.parentSessionID = "parent-" + longValue
        snapshot.agent = longValue
        snapshot.model = longValue
        snapshot.provider = longValue
        snapshot.source = "DeepSeek Harness session events · " + longValue
        overview.telemetryView.update(snapshot)
        overview.telemetryState.stringValue = snapshot.source + " · " + longPath
        overview.telemetrySource.addItem(withTitle: "DSH · " + longValue)
        overview.telemetrySource.selectItem(at: overview.telemetrySource.numberOfItems - 1)
        overview.applySnapshot(.init(request: .init(project: longPath, worker: "", stateDir: "/fixture", revision: 0),
                                     status: AgentStatus(known: false),
                                     commits: (0..<5).map { (date: Date(), hash: "fixture-\($0)", subject: longValue) },
                                     cards: [], updated: Date(), error: nil))
        settle()
        check(window.frame == originalFrame, "loaded Overview with long recorded text preserves window dimensions")
        check(overview.telemetryView.frame.width <= content.bounds.width, "long telemetry remains inside the Overview viewport")
        var controller: ChartInspectorController?
        for itemCount in [0, 1, 4, 12] {
            host.subviews.forEach { $0.removeFromSuperview() }
            let items = (0..<itemCount).map { index in
                ChartInspectionItem(date: Date(), title: "Commit \(index) · " + longValue, detail: longPath,
                                    kind: .commit, identifier: "fixture-\(index)")
            }
            controller = ChartInspectorController(title: "Selected activity bucket", items: items, onSelect: { _ in })
            check(controller!.preferredContentSize == .zero, "docked chart inspector does not request a popover-sized window")
            let detail = controller!.view
            detail.translatesAutoresizingMaskIntoConstraints = false
            host.addSubview(detail)
            NSLayoutConstraint.activate([
                detail.leadingAnchor.constraint(equalTo: host.leadingAnchor),
                detail.trailingAnchor.constraint(equalTo: host.trailingAnchor),
                detail.topAnchor.constraint(equalTo: host.topAnchor),
                detail.bottomAnchor.constraint(equalTo: host.bottomAnchor),
            ])
            visible = true
            update()
            settle()
            check(window.frame == originalFrame, "docking \(itemCount) items preserves \(initialWidth)-point window frame")
            check(detail.frame.height == root.bounds.height, "inspector fills the chosen window height")
            check(detail.frame.width >= 340, "inspector has a usable width")
            check(overviewView.frame == content.bounds, "loaded Overview follows the compact overlay or wide inspector inset")
            visible = false
            update()
            settle()
            check(window.frame == originalFrame, "closing inspector preserves window frame")
            visible = true
            update()
            settle()
            check(window.frame == originalFrame, "reopening inspector preserves window frame")
        }
        window.setContentSize(NSSize(width: initialWidth + 80, height: 720))
        settle()
        check(root.bounds.size == NSSize(width: initialWidth + 80, height: 720), "window remains resizable with inspector open")
        check(controller!.view.frame.height == 720, "inspector follows a subsequent native resize")
        root.onLayout = nil
        window.close()
    }
    print("PASS: loaded Overview with long telemetry, source/status text, recent commits, inspector docking/replacement, close/reopen, compact overlay, wide inset, and native resizing preserve chosen window dimensions")
}
