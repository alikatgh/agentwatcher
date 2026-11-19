import AppKit

func runWorkspaceChecks() {
    func require(_ condition: @autoclosure () -> Bool, _ message: String) {
        precondition(condition(), "Workspace: " + message)
    }
    for width: CGFloat in [900, 1080, 1180, 1440, 1920] {
        for sidebar in [true, false] {
            for preferred: CGFloat in [300, 400, 620] {
                let sizing = WorkspaceSizing(windowWidth: width, sidebarVisible: sidebar,
                                             sidebarWidth: 228, inspectorVisible: true, inspectorWidth: preferred)
                require(width - sizing.contentInset - sizing.inspectorWidth >= 480,
                        "open dock must preserve usable central content at \(width)")
                require(sizing.inspectorWidth >= 300, "dock remains readable")
            }
        }
    }
    let delegate = AppDelegate()
    delegate.navHistory = [0, 1, 4]
    delegate.navIndex = 1
    delegate.pushHistory(1)
    require(delegate.navHistory == [0, 1, 4] && delegate.navIndex == 1,
            "history traversal must preserve the forward entry")
    delegate.pushHistory(3)
    require(delegate.navHistory == [0, 1, 3] && delegate.navIndex == 2,
            "new navigation branches from the current history position")
    delegate.pushHistory(3)
    require(delegate.navHistory == [0, 1, 3], "reselection does not duplicate history")

    let palette = WorkspaceCommandPaletteController(commands: [
        WorkspaceCommand(id: "activity", title: "Open Live Activity", subtitle: "Workspace", perform: {}),
        WorkspaceCommand(id: "dark", title: "Appearance: Dark", subtitle: "Theme", perform: {}),
    ])
    palette.updateQuery("LIVE workspace")
    require(palette.filteredCommands.map(\.id) == ["activity"], "query matches title and category tokens")
    palette.updateQuery("no matching action")
    require(palette.filteredCommands.isEmpty && palette.selectedCommandID == nil, "empty search has no executable selection")
    palette.updateQuery("")
    palette.moveSelection(by: 1)
    require(palette.selectedCommandID == "dark", "keyboard movement selects next command")

    let splitWindow = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 600), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
    splitWindow.isReleasedWhenClosed = false
    let split = NSView()
    let left = NSView(), right = NSView()
    pinColumns(in: split, left: left, right: right, leftFraction: 0.34, minimum: 240, maximum: 360)
    splitWindow.contentView = split
    splitWindow.layoutIfNeeded()
    split.layoutSubtreeIfNeeded()
    split.subviews.compactMap { $0 as? WorkspaceDivider }.first!.onDrag?(100)
    splitWindow.layoutIfNeeded()
    splitWindow.setContentSize(NSSize(width: 448, height: 600))
    splitWindow.layoutIfNeeded()
    split.layoutSubtreeIfNeeded()
    require(split.bounds.width == 448 && right.frame.width >= 200,
            "manual split resizing preserves the chosen narrow window and readable detail")
    splitWindow.close()

    for size in [NSSize(width: 300, height: 540), NSSize(width: 400, height: 760)] {
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let dock = ChangesPane(docked: true)
        window.contentView = dock.view
        for _ in 0..<3 { dock.view.layoutSubtreeIfNeeded() }
        let filter = dock.filterField.convert(dock.filterField.bounds, to: dock.view)
        let diff = dock.diffView.enclosingScrollView!
        require(filter.maxY > dock.view.bounds.height - 100, "docked file search stays near the top")
        require(diff.frame.height > 200, "docked diff receives the remaining height")
        require(window.contentView!.bounds.size == size, "dock preserves selected window size")
        window.close()
    }
    print("PASS: workspace navigation history, compact/wide dock geometry, command filtering, and native dock layout")
}
