import AppKit
import WebKit

// AgentWatcher — a button-based macOS companion that watches and steers a
// continuous background coding agent (OpenCode). Sidebar navigation,
// live activity feed, hourly activity chart, increments table, embedded
// live session viewer and settings. No terminal required.

let home = NSHomeDirectory()

// MARK: - Preferences

struct Prefs {
    static let defaults = UserDefaults.standard
    static var defaultProject: String {
        let candidate = "\(home)/Documents/projects/swift_game"
        return FileManager.default.fileExists(atPath: candidate) ? candidate : home
    }
    static var defaultWorker: String {
        let candidate = "\(home)/.local/bin/nyurgun-agent-loop.sh"
        return FileManager.default.fileExists(atPath: candidate) ? candidate : ""
    }
    static var project: String {
        get { defaults.string(forKey: "project") ?? defaultProject }
        set { defaults.set(newValue, forKey: "project") }
    }
    static var worker: String {
        get { defaults.string(forKey: "worker") ?? defaultWorker }
        set { defaults.set(newValue, forKey: "worker") }
    }
    static var stateDir: String {
        get { defaults.string(forKey: "state") ?? "\(home)/.local/state/nyurgun-agent" }
        set { defaults.set(newValue, forKey: "state") }
    }
    static var logFile: String { stateDir + "/loop.log" }
    static var stopFile: String { stateDir + "/STOP" }
    static func bannerHidden(_ key: String) -> Bool { defaults.bool(forKey: "banner." + key) }
    static func setBannerHidden(_ key: String, _ value: Bool) { defaults.set(value, forKey: "banner." + key) }
}

// MARK: - Shell helpers

@discardableResult
func run(_ executable: String, _ arguments: [String], cwd: String? = nil) -> (code: Int32, output: String) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    if let cwd { process.currentDirectoryURL = URL(fileURLWithPath: cwd) }
    var environment = ProcessInfo.processInfo.environment
    environment["PATH"] = "\(home)/.opencode/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    process.environment = environment
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = pipe
    do {
        try process.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (process.terminationStatus, String(data: data, encoding: .utf8) ?? "")
    } catch {
        return (-1, "\(error)")
    }
}

@discardableResult
func bash(_ command: String) -> (code: Int32, output: String) {
    run("/bin/bash", ["-c", command])
}

func capture(_ pattern: String, in text: String) -> String? {
    guard let regex = try? NSRegularExpression(pattern: pattern) else { return nil }
    let range = NSRange(text.startIndex..<text.endIndex, in: text)
    guard let match = regex.firstMatch(in: text, range: range), match.numberOfRanges > 1,
          let captured = Range(match.range(at: 1), in: text) else { return nil }
    return String(text[captured])
}

// MARK: - Status

struct AgentStatus {
    var running = false
    var pausing = false
    var blocked = false

    var title: String {
        if blocked { return "Needs attention" }
        if running && pausing { return "Pausing…" }
        if running { return "Working" }
        if pausing { return "Paused" }
        return "Stopped"
    }
    var color: NSColor {
        if blocked { return .systemRed }
        if running && pausing { return .systemOrange }
        if running { return .systemGreen }
        if pausing { return .systemYellow }
        return .systemGray
    }
    static func current() -> AgentStatus {
        var status = AgentStatus()
        let worker = Prefs.worker
        if !worker.isEmpty {
            let name = (worker as NSString).lastPathComponent
            let result = run("/usr/bin/pgrep", ["-f", name])
            status.running = !result.output.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        status.pausing = FileManager.default.fileExists(atPath: Prefs.stopFile)
        status.blocked = FileManager.default.fileExists(atPath: Prefs.project + "/agent-loop-REPORT.md")
        return status
    }
}

// MARK: - Log parsing

struct LogEvent {
    var date: Date
    var line: String
}

let logFormatter: DateFormatter = {
    let formatter = DateFormatter()
    formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.timeZone = .current
    return formatter
}()

func logEvents() -> [LogEvent] {
    guard let data = FileManager.default.contents(atPath: Prefs.logFile),
          let text = String(data: data, encoding: .utf8) else { return [] }
    var events: [LogEvent] = []
    for raw in text.split(separator: "\n") {
        let line = String(raw)
        guard line.hasPrefix("["), let close = line.firstIndex(of: "]") else { continue }
        let stamp = String(line[line.index(after: line.startIndex)..<close])
        guard let date = logFormatter.date(from: stamp) else { continue }
        events.append(LogEvent(date: date, line: String(line.dropFirst(line.distance(from: line.startIndex, to: close) + 2))))
    }
    return events
}

func commitEvents(since: String) -> [(date: Date, subject: String, hash: String)] {
    let result = run("/usr/bin/git", ["-C", Prefs.project, "log", "--since=\(since)", "--pretty=format:%ct|%h|%s"])
    var items: [(Date, String, String)] = []
    for line in result.output.split(separator: "\n") {
        let parts = line.split(separator: "|", maxSplits: 2).map(String.init)
        guard parts.count == 3, let epoch = Double(parts[0]) else { continue }
        items.append((Date(timeIntervalSince1970: epoch), parts[1], parts[2]))
    }
    return items
}

func webBase() -> String? {
    let url = run("/usr/bin/git", ["-C", Prefs.project, "remote", "get-url", "origin"]).output
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard !url.isEmpty else { return nil }
    var base = url
    if base.hasPrefix("git@") {
        base = base.replacingOccurrences(of: "git@", with: "https://")
        base = base.replacingOccurrences(of: ":", with: "/")
    }
    if base.hasSuffix(".git") { base = String(base.dropLast(4)) }
    return base.hasPrefix("http") ? base : nil
}

func openOnGitHub(commit: String) {
    guard let base = webBase(), let url = URL(string: base + "/commit/" + commit) else { return }
    NSWorkspace.shared.open(url)
}

// MARK: - Small views

final class PulseDot: NSView {
    var color: NSColor = .systemGreen { didSet { needsDisplay = true } }
    private var phase: CGFloat = 0
    private var timer: Timer?
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.phase += 0.08
            self.needsDisplay = true
        }
        RunLoop.main.add(timer!, forMode: .common)
    }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ dirtyRect: NSRect) {
        let radius = bounds.height / 2
        let center = NSPoint(x: bounds.midX, y: bounds.midY)
        let glow = 0.35 + 0.35 * (0.5 + 0.5 * sin(phase))
        color.withAlphaComponent(glow).setFill()
        NSBezierPath(ovalIn: NSRect(x: center.x - radius * 1.6, y: center.y - radius * 1.6,
                                    width: radius * 3.2, height: radius * 3.2)).fill()
        color.setFill()
        NSBezierPath(ovalIn: NSRect(x: center.x - radius * 0.7, y: center.y - radius * 0.7,
                                    width: radius * 1.4, height: radius * 1.4)).fill()
    }
}

final class CardView: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor(calibratedWhite: 0.13, alpha: 1).cgColor
        layer?.cornerRadius = 12
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(calibratedWhite: 0.24, alpha: 1).cgColor
    }
    required init?(coder: NSCoder) { fatalError() }
}

final class InfoBanner: NSView {
    private var key = ""
    init(key: String, text: String) {
        super.init(frame: .zero)
        self.key = key
        wantsLayer = true
        layer?.backgroundColor = NSColor(calibratedWhite: 0.17, alpha: 1).cgColor
        layer?.cornerRadius = 10
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(calibratedWhite: 0.27, alpha: 1).cgColor
        let icon = NSImageView()
        icon.image = NSImage(systemSymbolName: "info.circle", accessibilityDescription: nil)
        icon.contentTintColor = .systemTeal
        icon.translatesAutoresizingMaskIntoConstraints = false
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: 11)
        label.textColor = .secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        let close = NSButton(title: "", target: self, action: #selector(closeTapped))
        close.image = NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Hide explanation")
        close.isBordered = false
        close.contentTintColor = .tertiaryLabelColor
        close.toolTip = "Hide this explanation"
        close.translatesAutoresizingMaskIntoConstraints = false
        addSubview(icon)
        addSubview(label)
        addSubview(close)
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            icon.topAnchor.constraint(equalTo: topAnchor, constant: 9),
            icon.widthAnchor.constraint(equalToConstant: 15),
            label.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 8),
            label.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            label.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
            close.leadingAnchor.constraint(equalTo: label.trailingAnchor, constant: 8),
            close.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            close.topAnchor.constraint(equalTo: topAnchor, constant: 5),
        ])
        translatesAutoresizingMaskIntoConstraints = false
        isHidden = Prefs.bannerHidden(key)
    }
    required init?(coder: NSCoder) { fatalError() }
    @objc func closeTapped() {
        isHidden = true
        Prefs.setBannerHidden(key, true)
    }
}

final class LiveBadge: NSView {
    init() {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.systemRed.withAlphaComponent(0.18).cgColor
        layer?.cornerRadius = 9
        let label = NSTextField(labelWithString: "LIVE")
        label.font = .systemFont(ofSize: 10, weight: .bold)
        label.textColor = .systemRed
        label.translatesAutoresizingMaskIntoConstraints = false
        let dot = NSView()
        dot.wantsLayer = true
        dot.layer?.backgroundColor = NSColor.systemRed.cgColor
        dot.layer?.cornerRadius = 4
        dot.translatesAutoresizingMaskIntoConstraints = false
        let blink = CABasicAnimation(keyPath: "opacity")
        blink.fromValue = 1.0
        blink.toValue = 0.25
        blink.duration = 0.7
        blink.autoreverses = true
        blink.repeatCount = .infinity
        dot.layer?.add(blink, forKey: "blink")
        addSubview(dot)
        addSubview(label)
        NSLayoutConstraint.activate([
            dot.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 7),
            dot.centerYAnchor.constraint(equalTo: centerYAnchor),
            dot.widthAnchor.constraint(equalToConstant: 8),
            dot.heightAnchor.constraint(equalToConstant: 8),
            label.leadingAnchor.constraint(equalTo: dot.trailingAnchor, constant: 5),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 18),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }
}

final class LineChartView: NSView {
    struct Series {
        var name: String
        var color: NSColor
        var values: [CGFloat]
        var visible: Bool = true
    }
    var series: [Series] = []
    private var pulse: CGFloat = 0
    private var timer: Timer?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window == nil {
            timer?.invalidate()
            timer = nil
        } else if timer == nil {
            timer = Timer.scheduledTimer(withTimeInterval: 0.06, repeats: true) { [weak self] _ in
                guard let self else { return }
                self.pulse += 0.045
                if self.pulse > 1 { self.pulse -= 1 }
                self.needsDisplay = true
            }
            RunLoop.main.add(timer!, forMode: .common)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let w = bounds.width
        let h = bounds.height
        guard w > 20, h > 20 else { return }
        let rows = 3
        for row in 0...rows {
            let y = h * CGFloat(row) / CGFloat(rows)
            NSColor(calibratedWhite: 0.30, alpha: 0.35).setStroke()
            let path = NSBezierPath()
            path.move(to: NSPoint(x: 0, y: y))
            path.line(to: NSPoint(x: w, y: y))
            path.lineWidth = 1
            path.stroke()
        }
        let visible = series.filter { $0.visible }
        let maxValue = max(1, visible.flatMap { $0.values }.max() ?? 1)
        let count = visible.first?.values.count ?? 0
        guard count > 1 else { return }
        let labelAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 10),
            .foregroundColor: NSColor.secondaryLabelColor,
        ]
        String(format: "%.0f", maxValue).draw(at: NSPoint(x: 4, y: h - 14), withAttributes: labelAttrs)
        "0".draw(at: NSPoint(x: 4, y: 4), withAttributes: labelAttrs)

        for item in visible {
            let points = item.values.enumerated().map { index, value in
                NSPoint(x: w * CGFloat(index) / CGFloat(count - 1),
                        y: (h - 18) * value / maxValue + 9)
            }
            let line = NSBezierPath()
            line.move(to: points[0])
            for i in 0..<points.count - 1 {
                let p0 = i == 0 ? points[0] : points[i - 1]
                let p1 = points[i]
                let p2 = points[i + 1]
                let p3 = i + 2 < points.count ? points[i + 2] : p2
                let clampY = { (value: CGFloat) -> CGFloat in min(h - 2, max(2, value)) }
                let c1 = NSPoint(x: p1.x + (p2.x - p0.x) / 6, y: clampY(p1.y + (p2.y - p0.y) / 6))
                let c2 = NSPoint(x: p2.x - (p3.x - p1.x) / 6, y: clampY(p2.y - (p3.y - p1.y) / 6))
                line.curve(to: p2, controlPoint1: c1, controlPoint2: c2)
            }
            let fill = line.copy() as! NSBezierPath
            fill.line(to: NSPoint(x: points.last!.x, y: 0))
            fill.line(to: NSPoint(x: points[0].x, y: 0))
            fill.close()
            if let gradient = NSGradient(colors: [item.color.withAlphaComponent(0.02),
                                                  item.color.withAlphaComponent(0.30)]) {
                gradient.draw(in: fill, angle: 90)
            }
            item.color.setStroke()
            line.lineWidth = 2
            line.stroke()
            if let last = points.last {
                let ringRadius = 6 + 10 * pulse
                item.color.withAlphaComponent(0.55 * (1 - pulse)).setStroke()
                let ring = NSBezierPath(ovalIn: NSRect(x: last.x - ringRadius, y: last.y - ringRadius,
                                                       width: ringRadius * 2, height: ringRadius * 2))
                ring.lineWidth = 2
                ring.stroke()
                NSColor.white.setStroke()
                item.color.setFill()
                let dot = NSBezierPath(ovalIn: NSRect(x: last.x - 5, y: last.y - 5, width: 10, height: 10))
                dot.fill()
                dot.lineWidth = 2
                dot.stroke()
            }
        }
    }
}

// MARK: - Detail panes

protocol Refreshable: AnyObject {
    func refresh()
}

func headerLabel(_ text: String, size: CGFloat, weight: NSFont.Weight = .regular, color: NSColor = .labelColor) -> NSTextField {
    let field = NSTextField(labelWithString: text)
    field.font = .systemFont(ofSize: size, weight: weight)
    field.textColor = color
    return field
}

class Pane: NSViewController, Refreshable {
    var onNavigate: ((Int) -> Void)?
    func refresh() {}

    @discardableResult
    func addBanner(_ stack: NSStackView, key: String, text: String) -> InfoBanner {
        let banner = InfoBanner(key: key, text: text)
        stack.addArrangedSubview(banner)
        banner.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
        return banner
    }
    func makeHeader(_ title: String, _ subtitle: String) -> NSStackView {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 2
        stack.addArrangedSubview(headerLabel(title, size: 22, weight: .bold))
        stack.addArrangedSubview(headerLabel(subtitle, size: 12, color: .secondaryLabelColor))
        return stack
    }
}

final class OverviewPane: Pane {
    let statusDot = PulseDot(frame: .zero)
    let statusLabel = headerLabel("", size: 13, weight: .semibold)
    let spinner = NSProgressIndicator()
    let metricLabel = headerLabel("0", size: 44, weight: .bold)
    let deltaLabel = headerLabel("", size: 15, weight: .semibold, color: .systemGreen)
    let metricSub = headerLabel("", size: 12, color: .secondaryLabelColor)
    let chart = LineChartView()
    let chartUpdated = headerLabel("", size: 11, color: .secondaryLabelColor)
    let recentStack = NSStackView()
    let startButton = NSButton(title: "Start", target: nil, action: nil)
    let pauseButton = NSButton(title: "Pause", target: nil, action: nil)
    let stopButton = NSButton(title: "Force Stop", target: nil, action: nil)
    let chipIncrements = NSButton(title: "", target: nil, action: nil)
    let chipRuns = NSButton(title: "", target: nil, action: nil)
    let rangeControl = NSSegmentedControl(labels: ["1D", "5D", "1M"], trackingMode: .selectOne, target: nil, action: nil)

    override func loadView() {
        let root = NSView()
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 22, right: 26)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: root.bottomAnchor),
        ])

        let headline = NSStackView()
        headline.orientation = .horizontal
        headline.spacing = 12
        headline.addArrangedSubview(metricLabel)
        headline.addArrangedSubview(deltaLabel)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        headline.addArrangedSubview(spacer)
        statusDot.translatesAutoresizingMaskIntoConstraints = false
        statusDot.widthAnchor.constraint(equalToConstant: 18).isActive = true
        statusDot.heightAnchor.constraint(equalToConstant: 18).isActive = true
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        headline.addArrangedSubview(statusDot)
        headline.addArrangedSubview(statusLabel)
        headline.addArrangedSubview(spinner)
        statusLabel.toolTip = "Click to open the live activity feed"
        statusDot.toolTip = "Click to open the live activity feed"
        statusLabel.addGestureRecognizer(NSClickGestureRecognizer(target: self, action: #selector(goActivity)))
        statusDot.addGestureRecognizer(NSClickGestureRecognizer(target: self, action: #selector(goActivity)))
        stack.addArrangedSubview(headline)
        stack.addArrangedSubview(metricSub)
        stack.addArrangedSubview(headerLabel("Agent activity", size: 12, weight: .semibold, color: .secondaryLabelColor))

        let controls = NSStackView()
        controls.orientation = .horizontal
        controls.spacing = 10
        styleChip(chipIncrements, title: "Increments", color: .systemTeal)
        styleChip(chipRuns, title: "Runs", color: .systemBlue)
        chipIncrements.target = self
        chipIncrements.action = #selector(chipToggled)
        chipRuns.target = self
        chipRuns.action = #selector(chipToggled)
        controls.addArrangedSubview(chipIncrements)
        controls.addArrangedSubview(chipRuns)
        let controlsSpacer = NSView()
        controlsSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        controls.addArrangedSubview(controlsSpacer)
        rangeControl.selectedSegment = 0
        rangeControl.target = self
        rangeControl.action = #selector(rangeChanged)
        controls.addArrangedSubview(rangeControl)
        stack.addArrangedSubview(controls)

        let chartCard = CardView()
        chartCard.translatesAutoresizingMaskIntoConstraints = false
        let chartStack = NSStackView()
        chartStack.orientation = .vertical
        chartStack.alignment = .leading
        chartStack.spacing = 10
        chartStack.translatesAutoresizingMaskIntoConstraints = false
        chartCard.addSubview(chartStack)
        let chartHeader = NSStackView()
        chartHeader.orientation = .horizontal
        chartHeader.spacing = 10
        chartHeader.addArrangedSubview(headerLabel("Agent at work", size: 14, weight: .semibold))
        chartHeader.addArrangedSubview(LiveBadge())
        let chartSpacer = NSView()
        chartSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        chartHeader.addArrangedSubview(chartSpacer)
        chartHeader.addArrangedSubview(chartUpdated)
        chart.translatesAutoresizingMaskIntoConstraints = false
        chartStack.addArrangedSubview(chartHeader)
        chartStack.addArrangedSubview(chart)
        NSLayoutConstraint.activate([
            chartStack.topAnchor.constraint(equalTo: chartCard.topAnchor, constant: 14),
            chartStack.bottomAnchor.constraint(equalTo: chartCard.bottomAnchor, constant: -14),
            chartStack.leadingAnchor.constraint(equalTo: chartCard.leadingAnchor, constant: 18),
            chartStack.trailingAnchor.constraint(equalTo: chartCard.trailingAnchor, constant: -18),
            chartHeader.widthAnchor.constraint(equalTo: chartStack.widthAnchor),
            chart.widthAnchor.constraint(equalTo: chartStack.widthAnchor),
            chart.heightAnchor.constraint(equalToConstant: 280),
        ])
        stack.addArrangedSubview(chartCard)
        chartCard.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
        chartCard.toolTip = "Click to cycle the range (1D / 5D / 1M)"
        chartCard.addGestureRecognizer(NSClickGestureRecognizer(target: self, action: #selector(cycleRange)))

        let buttonRow = NSStackView()
        buttonRow.orientation = .horizontal
        buttonRow.spacing = 10
        for button in [startButton, pauseButton, stopButton] {
            button.bezelStyle = .rounded
            buttonRow.addArrangedSubview(button)
        }
        startButton.target = self
        startButton.action = #selector(startWork)
        startButton.keyEquivalent = "\r"
        pauseButton.target = self
        pauseButton.action = #selector(pauseWork)
        stopButton.target = self
        stopButton.action = #selector(forceStop)
        let openButton = NSButton(title: "Open Project Folder", target: self, action: #selector(openFolder))
        openButton.bezelStyle = .rounded
        buttonRow.addArrangedSubview(openButton)
        stack.addArrangedSubview(buttonRow)

        let recentCard = CardView()
        recentCard.translatesAutoresizingMaskIntoConstraints = false
        recentStack.orientation = .vertical
        recentStack.alignment = .leading
        recentStack.spacing = 4
        recentStack.translatesAutoresizingMaskIntoConstraints = false
        recentCard.addSubview(recentStack)
        NSLayoutConstraint.activate([
            recentStack.topAnchor.constraint(equalTo: recentCard.topAnchor, constant: 12),
            recentStack.bottomAnchor.constraint(equalTo: recentCard.bottomAnchor, constant: -12),
            recentStack.leadingAnchor.constraint(equalTo: recentCard.leadingAnchor, constant: 18),
            recentStack.trailingAnchor.constraint(equalTo: recentCard.trailingAnchor, constant: -18),
        ])
        stack.addArrangedSubview(recentCard)
        recentCard.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true

        addBanner(stack, key: "overview", text: "Watching your background agent: the big number counts increments (commits) landed today, the chart shows activity for the selected range, and the buttons start, pause or stop the worker. Everything here is clickable — try the commits, the status dot and the chart.")
        view = root
    }

    func styleChip(_ button: NSButton, title: String, color: NSColor) {
        button.setButtonType(.pushOnPushOff)
        button.bezelStyle = .rounded
        button.state = .on
        let attributed = NSMutableAttributedString(string: "● ", attributes: [
            .foregroundColor: color,
            .font: NSFont.systemFont(ofSize: 12, weight: .semibold),
        ])
        attributed.append(NSAttributedString(string: title, attributes: [
            .foregroundColor: NSColor.labelColor,
            .font: NSFont.systemFont(ofSize: 12, weight: .medium),
        ]))
        button.attributedTitle = attributed
    }

    @objc func chipToggled() { refresh() }

    @objc func rangeChanged() { refresh() }

    @objc func goActivity() { onNavigate?(1) }

    @objc func cycleRange() {
        rangeControl.selectedSegment = (rangeControl.selectedSegment + 1) % 3
        refresh()
    }

    @objc func openRecent(_ sender: NSButton) {
        if let commit = sender.identifier?.rawValue { openOnGitHub(commit: commit) }
    }

    @objc func openFolder() {
        NSWorkspace.shared.open(URL(fileURLWithPath: Prefs.project))
    }

    @objc func startWork() {
        bash("mkdir -p \"\(Prefs.stateDir)\"; rm -f \"\(Prefs.stopFile)\"; nohup caffeinate -is /bin/bash \"\(Prefs.worker)\" >/dev/null 2>&1 &")
        refresh()
    }
    @objc func pauseWork() {
        bash("mkdir -p \"\(Prefs.stateDir)\"; touch \"\(Prefs.stopFile)\"")
        refresh()
    }
    @objc func forceStop() {
        let name = (Prefs.worker as NSString).lastPathComponent
        if !name.isEmpty { bash("pkill -f \"\(name)\"") }
        refresh()
    }

    func buckets(_ range: Int) -> (commits: [CGFloat], runs: [CGFloat]) {
        let since: String
        let count: Int
        let seconds: Double
        switch range {
        case 1: since = "5.days"; count = 5; seconds = 86400
        case 2: since = "30.days"; count = 30; seconds = 86400
        default: since = "24.hours"; count = 96; seconds = 900
        }
        var commits = [CGFloat](repeating: 0, count: count)
        for item in commitEvents(since: since) {
            let index = count - 1 - Int(Date().timeIntervalSince(item.date) / seconds)
            if index >= 0 && index < count { commits[index] += 1 }
        }
        var runs = [CGFloat](repeating: 0, count: count)
        for event in logEvents() where event.line.contains("starting run") {
            let index = count - 1 - Int(Date().timeIntervalSince(event.date) / seconds)
            if index >= 0 && index < count { runs[index] += 1 }
        }
        return (commits, runs)
    }

    override func refresh() {
        let status = AgentStatus.current()
        statusLabel.stringValue = status.title
        statusDot.color = status.color
        if status.running { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
        startButton.isEnabled = !status.running && !Prefs.worker.isEmpty
        pauseButton.isEnabled = status.running
        stopButton.isEnabled = status.running

        let recentCommits = commitEvents(since: "48.hours")
        let todayStart = Calendar.current.startOfDay(for: Date())
        let yesterdayStart = todayStart.addingTimeInterval(-86400)
        let todayCount = recentCommits.filter { $0.date >= todayStart }.count
        let yesterdayCount = recentCommits.filter { $0.date >= yesterdayStart && $0.date < todayStart }.count
        metricLabel.stringValue = "\(todayCount)"
        let diff = todayCount - yesterdayCount
        deltaLabel.stringValue = "\(diff >= 0 ? "+" : "")\(diff) vs yesterday"
        deltaLabel.textColor = diff >= 0 ? .systemGreen : .systemRed
        let stamp = DateFormatter.localizedString(from: Date(), dateStyle: .none, timeStyle: .medium)
        metricSub.stringValue = "increments landed today  ·  updated \(stamp)"
        chartUpdated.stringValue = "updated \(stamp)"

        let data = buckets(rangeControl.selectedSegment)
        chart.series = [
            LineChartView.Series(name: "Increments", color: .systemTeal,
                                 values: data.commits, visible: chipIncrements.state == .on),
            LineChartView.Series(name: "Runs", color: .systemBlue,
                                 values: data.runs, visible: chipRuns.state == .on),
        ]
        chart.needsDisplay = true

        let recent = run("/usr/bin/git", ["-C", Prefs.project, "log", "-5", "--pretty=format:%h|%s|%cr"]).output
        recentStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for line in recent.split(separator: "\n") {
            let parts = line.split(separator: "|", maxSplits: 2).map(String.init)
            guard parts.count == 3 else { continue }
            let button = NSButton(title: "\(parts[0])  \(parts[1])  (\(parts[2]))", target: self, action: #selector(openRecent(_:)))
            button.isBordered = false
            button.alignment = .left
            button.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
            button.toolTip = "Open \(parts[0]) on GitHub"
            button.identifier = NSUserInterfaceItemIdentifier(parts[0])
            recentStack.addArrangedSubview(button)
        }
        if recentStack.arrangedSubviews.isEmpty {
            recentStack.addArrangedSubview(headerLabel("No commits yet.", size: 12))
        }
    }
}

final class ActivityPane: Pane {
    let feed = NSTextView()
    let follow = NSButton(checkboxWithTitle: "Follow", target: nil, action: nil)
    let updatedLabel = headerLabel("", size: 12, color: .secondaryLabelColor)

    override func loadView() {
        let root = NSView()
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 22, right: 26)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        stack.addArrangedSubview(makeHeader("Live Activity", "Driver log stream"))
        let controls = NSStackView()
        controls.orientation = .horizontal
        controls.spacing = 12
        follow.state = .on
        controls.addArrangedSubview(follow)
        controls.addArrangedSubview(updatedLabel)
        stack.addArrangedSubview(controls)

        feed.isEditable = false
        feed.isRichText = false
        feed.drawsBackground = true
        feed.backgroundColor = NSColor(calibratedWhite: 0.10, alpha: 1)
        feed.textColor = NSColor(calibratedWhite: 0.9, alpha: 1)
        feed.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        feed.textContainerInset = NSSize(width: 10, height: 10)
        let scroll = NSScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.documentView = feed
        scroll.wantsLayer = true
        scroll.layer?.cornerRadius = 12
        scroll.layer?.borderWidth = 1
        scroll.layer?.borderColor = NSColor(calibratedWhite: 0.24, alpha: 1).cgColor
        stack.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
        addBanner(stack, key: "activity", text: "The worker's own log, streamed live. Follow keeps the newest lines in view.")
        view = root
    }

    override func refresh() {
        let result = run("/usr/bin/tail", ["-n", "220", Prefs.logFile])
        let lines = result.output.split(separator: "\n").suffix(120).map { String($0.prefix(200)) }
        feed.string = lines.joined(separator: "\n")
        let stamp = DateFormatter.localizedString(from: Date(), dateStyle: .none, timeStyle: .medium)
        updatedLabel.stringValue = "updated \(stamp)"
        if follow.state == .on { feed.scrollToEndOfDocument(nil) }
    }
}

final class IncrementsPane: Pane {
    let table = NSTableView()
    var rows: [(String, String, String)] = []
    var lastCount = 0

    override func loadView() {
        let root = NSView()
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 22, right: 26)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        stack.addArrangedSubview(makeHeader("Increments", "Committed work, newest first"))
        for (title, width) in [("Hash", CGFloat(70)), ("Increment", CGFloat(520)), ("When", CGFloat(140))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(title))
            column.title = title
            column.width = width
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = true
        table.style = .inset
        let scroll = NSScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.documentView = table
        scroll.wantsLayer = true
        scroll.layer?.cornerRadius = 12
        scroll.layer?.borderWidth = 1
        scroll.layer?.borderColor = NSColor(calibratedWhite: 0.24, alpha: 1).cgColor
        stack.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
        table.target = self
        table.doubleAction = #selector(openCommit)
        table.toolTip = "Double-click a row to open the commit on GitHub"
        addBanner(stack, key: "increments", text: "Every committed work increment, newest first. Double-click a row to open it on GitHub.")
        view = root
    }

    @objc func openCommit() {
        let row = table.selectedRow
        if row >= 0 && row < rows.count { openOnGitHub(commit: rows[row].0) }
    }

    override func refresh() {
        let result = run("/usr/bin/git", ["-C", Prefs.project, "log", "-40", "--pretty=format:%h|%s|%cr"])
        rows = result.output.split(separator: "\n").map { line in
            let parts = line.split(separator: "|", maxSplits: 2).map(String.init)
            return parts.count == 3 ? (parts[0], parts[1], parts[2]) : ("", String(line), "")
        }
        if rows.count > lastCount && lastCount > 0 {
            table.insertRows(at: IndexSet(0..<(rows.count - lastCount)), withAnimation: .effectFade)
        } else {
            table.reloadData()
        }
        lastCount = rows.count
    }
}

extension IncrementsPane: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView()
        let field = NSTextField(labelWithString: "")
        field.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        field.lineBreakMode = .byTruncatingTail
        field.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(field)
        cell.textField = field
        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
            field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
            field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        switch tableColumn?.identifier.rawValue {
        case "Hash": field.stringValue = rows[row].0
        case "Increment": field.stringValue = rows[row].1
        default: field.stringValue = rows[row].2
        }
        return cell
    }
}

final class SessionsPane: Pane {
    let table = NSTableView()
    var rows: [(String, String, String)] = []

    override func loadView() {
        let root = NSView()
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 22, right: 26)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        stack.addArrangedSubview(makeHeader("Sessions", "OpenCode sessions for this project"))
        for (title, width) in [("Title", CGFloat(460)), ("Updated", CGFloat(200)), ("Session", CGFloat(260))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(title))
            column.title = title
            column.width = width
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = true
        table.style = .inset
        let scroll = NSScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.documentView = table
        scroll.wantsLayer = true
        scroll.layer?.cornerRadius = 12
        scroll.layer?.borderWidth = 1
        scroll.layer?.borderColor = NSColor(calibratedWhite: 0.24, alpha: 1).cgColor
        stack.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
        table.target = self
        table.doubleAction = #selector(openSession)
        table.toolTip = "Double-click a session to open the live interface"
        addBanner(stack, key: "sessions", text: "OpenCode sessions for this project. Double-click one to open the live interface.")
        view = root
    }

    @objc func openSession() { onNavigate?(4) }

    override func refresh() {
        let result = run("\(home)/.opencode/bin/opencode", ["session", "list"], cwd: Prefs.project)
        rows = result.output.split(separator: "\n").map { line in
            let parts = line.split(separator: "\t", maxSplits: 2).map(String.init)
            if parts.count >= 3 { return (parts[1], parts[2], parts[0]) }
            return (String(line), "", "")
        }
        table.reloadData()
    }
}

extension SessionsPane: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView()
        let field = NSTextField(labelWithString: "")
        field.font = .systemFont(ofSize: 12)
        field.lineBreakMode = .byTruncatingTail
        field.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(field)
        cell.textField = field
        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
            field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
            field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        switch tableColumn?.identifier.rawValue {
        case "Title": field.stringValue = rows[row].0
        case "Updated": field.stringValue = rows[row].1
        default: field.stringValue = rows[row].2
        }
        return cell
    }
}

final class LivePane: Pane, WKNavigationDelegate {
    let web = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
    let stateLabel = headerLabel("Connecting…", size: 12, color: .secondaryLabelColor)
    var credential: URLCredential?
    var loaded = false

    override func loadView() {
        let root = NSView()
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 22, right: 26)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        stack.addArrangedSubview(makeHeader("Live Sessions", "The full OpenCode interface, embedded"))
        let controls = NSStackView()
        controls.orientation = .horizontal
        controls.spacing = 12
        let reload = NSButton(title: "Reconnect", target: self, action: #selector(reconnect))
        reload.bezelStyle = .rounded
        controls.addArrangedSubview(reload)
        controls.addArrangedSubview(stateLabel)
        stack.addArrangedSubview(controls)
        web.navigationDelegate = self
        web.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(web)
        web.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
        addBanner(stack, key: "live", text: "The full OpenCode web interface, embedded and paired automatically. Press Reconnect if the connection drops.")
        view = root
    }

    override func refresh() {
        if !loaded { loaded = true; reconnect() }
    }

    @objc func reconnect() {
        stateLabel.stringValue = "Connecting…"
        showMessage("Connecting to the OpenCode service…")
        DispatchQueue.global().async {
            let result = run("\(home)/.opencode/bin/opencode", ["pair"], cwd: Prefs.project)
            let urlString = capture("URLs\\s+(http\\S+)", in: result.output)
            let username = capture("Username\\s+(\\S+)", in: result.output) ?? "opencode"
            let password = capture("Password\\s+(\\S+)", in: result.output)
            DispatchQueue.main.async {
                guard result.code == 0, let urlString, let url = URL(string: urlString) else {
                    self.stateLabel.stringValue = "Service unavailable"
                    self.showMessage("Could not create a live link.<br><br>Make sure the OpenCode service is running, then press Reconnect.")
                    return
                }
                self.credential = URLCredential(user: username, password: password ?? "", persistence: .none)
                self.stateLabel.stringValue = "Connected — live"
                self.web.load(URLRequest(url: url))
            }
        }
    }

    func showMessage(_ message: String) {
        let html = """
        <html><body style="font-family:-apple-system;background:#16181c;color:#e6e6e6;display:flex;align-items:center;justify-content:center;height:100vh;margin:0">
        <div style="text-align:center;font-size:15px;max-width:520px">\(message)</div></body></html>
        """
        web.loadHTMLString(html, baseURL: nil)
    }

    func webView(_ webView: WKWebView, didReceive challenge: URLAuthenticationChallenge,
                 completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        if challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodHTTPBasic,
           let credential {
            completionHandler(.useCredential, credential)
        } else {
            completionHandler(.performDefaultHandling, nil)
        }
    }
}

final class SettingsPane: Pane {
    let projectField = NSTextField()
    let workerField = NSTextField()
    let stateField = NSTextField()
    let infoLabel = headerLabel("", size: 11, color: .secondaryLabelColor)

    override func loadView() {
        let root = NSView()
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 22, right: 26)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
        ])
        stack.addArrangedSubview(makeHeader("Settings", "Where AgentWatcher looks and what it starts"))
        for (title, field) in [("Project folder", projectField), ("Worker script", workerField), ("State folder", stateField)] {
            stack.addArrangedSubview(headerLabel(title, size: 12, weight: .semibold))
            field.translatesAutoresizingMaskIntoConstraints = false
            field.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
            stack.addArrangedSubview(field)
            field.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
        }
        let save = NSButton(title: "Save", target: self, action: #selector(saveSettings))
        save.bezelStyle = .rounded
        stack.addArrangedSubview(save)
        stack.addArrangedSubview(infoLabel)
        addBanner(stack, key: "settings", text: "Point AgentWatcher at your project, the worker script it should run, and where the worker keeps its state. No project files are modified.")
        view = root
    }

    override func refresh() {
        if projectField.stringValue.isEmpty {
            projectField.stringValue = Prefs.project
            workerField.stringValue = Prefs.worker
            stateField.stringValue = Prefs.stateDir
        }
        let logExists = FileManager.default.fileExists(atPath: Prefs.logFile)
        infoLabel.stringValue = logExists ? "Log found: \(Prefs.logFile)" : "No log yet at \(Prefs.logFile)"
    }

    @objc func saveSettings() {
        if !projectField.stringValue.isEmpty { Prefs.project = projectField.stringValue }
        Prefs.worker = workerField.stringValue
        if !stateField.stringValue.isEmpty { Prefs.stateDir = stateField.stringValue }
        refresh()
    }
}

// MARK: - Sidebar + window

struct Section {
    let title: String
    let symbol: String
    let tint: NSColor
    let make: () -> Pane
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var sidebar: NSTableView!
    var content = NSView()
    var panes: [Pane] = []
    var current = 0
    var timer: Timer?

    let sections: [Section] = [
        Section(title: "Overview", symbol: "gauge", tint: .systemTeal) { OverviewPane() },
        Section(title: "Live Activity", symbol: "waveform.path.ecg", tint: .systemBlue) { ActivityPane() },
        Section(title: "Increments", symbol: "checklist", tint: .systemGreen) { IncrementsPane() },
        Section(title: "Sessions", symbol: "bubble.left.and.bubble.right", tint: .systemPurple) { SessionsPane() },
        Section(title: "Live Sessions", symbol: "globe", tint: .systemOrange) { LivePane() },
        Section(title: "Settings", symbol: "gearshape", tint: .systemGray) { SettingsPane() },
    ]

    func applicationDidFinishLaunching(_ notification: Notification) {
        buildMenu()
        buildWindow()
        refreshCurrent()
        let timer = Timer(timeInterval: 2.5, repeats: true) { [weak self] _ in self?.refreshCurrent() }
        RunLoop.main.add(timer, forMode: .common)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func buildMenu() {
        let mainMenu = NSMenu()
        let appItem = NSMenuItem()
        mainMenu.addItem(appItem)
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "About AgentWatcher", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Hide AgentWatcher", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Quit AgentWatcher", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        NSApp.mainMenu = mainMenu
    }

    func buildWindow() {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1080, height: 720),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        window.title = "AgentWatcher"
        window.appearance = NSAppearance(named: .darkAqua)
        window.minSize = NSSize(width: 900, height: 600)
        window.center()

        let root = NSView()
        sidebar = NSTableView()
        sidebar.style = .sourceList
        sidebar.headerView = nil
        sidebar.rowHeight = 34
        sidebar.selectionHighlightStyle = .regular
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("main"))
        column.width = 220
        sidebar.addTableColumn(column)
        sidebar.dataSource = self
        sidebar.delegate = self
        sidebar.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        let sidebarScroll = NSScrollView()
        sidebarScroll.translatesAutoresizingMaskIntoConstraints = false
        sidebarScroll.documentView = sidebar
        sidebarScroll.drawsBackground = false
        sidebarScroll.widthAnchor.constraint(equalToConstant: 224).isActive = true

        content.translatesAutoresizingMaskIntoConstraints = false

        root.addSubview(sidebarScroll)
        root.addSubview(content)
        NSLayoutConstraint.activate([
            sidebarScroll.topAnchor.constraint(equalTo: root.topAnchor),
            sidebarScroll.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            sidebarScroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            content.topAnchor.constraint(equalTo: root.topAnchor),
            content.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            content.leadingAnchor.constraint(equalTo: sidebarScroll.trailingAnchor),
            content.trailingAnchor.constraint(equalTo: root.trailingAnchor),
        ])
        window.contentView = root
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        showPane(0)
    }

    func showPane(_ index: Int) {
        current = index
        content.subviews.forEach { $0.removeFromSuperview() }
        let pane = panes.count > index ? panes[index] : sections[index].make()
        if panes.count <= index {
            panes.append(pane)
        }
        pane.onNavigate = { [weak self] index in
            guard let self else { return }
            self.sidebar.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            self.showPane(index)
        }
        let paneView = pane.view
        paneView.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(paneView)
        NSLayoutConstraint.activate([
            paneView.topAnchor.constraint(equalTo: content.topAnchor),
            paneView.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            paneView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            paneView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
        ])
        pane.refresh()
    }

    func refreshCurrent() {
        if panes.count > current { panes[current].refresh() }
    }
}

extension AppDelegate: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { sections.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView()
        let image = NSImageView()
        image.image = NSImage(systemSymbolName: sections[row].symbol, accessibilityDescription: nil)
        image.contentTintColor = sections[row].tint
        image.translatesAutoresizingMaskIntoConstraints = false
        let field = NSTextField(labelWithString: sections[row].title)
        field.font = .systemFont(ofSize: 13)
        field.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(image)
        cell.addSubview(field)
        cell.textField = field
        NSLayoutConstraint.activate([
            image.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 8),
            image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            image.widthAnchor.constraint(equalToConstant: 20),
            field.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 8),
            field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }
    func tableViewSelectionDidChange(_ notification: Notification) {
        let row = sidebar.selectedRow
        if row >= 0 && row != current { showPane(row) }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
