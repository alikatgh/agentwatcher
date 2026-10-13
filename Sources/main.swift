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
    static var project: String {
        get {
            if let stored = defaults.string(forKey: "project"),
               FileManager.default.fileExists(atPath: stored) { return stored }
            let detected = AutoDetect.project().value ?? home
            defaults.set(detected, forKey: "project")
            return detected
        }
        set { defaults.set(newValue, forKey: "project") }
    }
    static var worker: String {
        get {
            if let stored = defaults.string(forKey: "worker"), !stored.isEmpty,
               FileManager.default.fileExists(atPath: stored) { return stored }
            let detected = AutoDetect.worker().value ?? ""
            defaults.set(detected, forKey: "worker")
            return detected
        }
        set { defaults.set(newValue, forKey: "worker") }
    }
    static var stateDir: String {
        get {
            if let stored = defaults.string(forKey: "state"),
               FileManager.default.fileExists(atPath: stored) { return stored }
            let detected = AutoDetect.state(forWorker: worker).value ?? "\(home)/.local/state/agentwatcher"
            defaults.set(detected, forKey: "state")
            return detected
        }
        set { defaults.set(newValue, forKey: "state") }
    }
    static var logFile: String { stateDir + "/loop.log" }
    static var stopFile: String { stateDir + "/STOP" }
    static var autoStart: Bool {
        get { defaults.object(forKey: "autoStart") == nil ? true : defaults.bool(forKey: "autoStart") }
        set { defaults.set(newValue, forKey: "autoStart") }
    }
    static func bannerHidden(_ key: String) -> Bool { defaults.bool(forKey: "banner." + key) }
    static func setBannerHidden(_ key: String, _ value: Bool) { defaults.set(value, forKey: "banner." + key) }
}

struct AutoDetect {
    static func project() -> (value: String?, source: String) {
        let fm = FileManager.default
        let roots = ["\(home)/Documents/projects", "\(home)/Projects", "\(home)/code",
                     "\(home)/Developer", "\(home)/src", "\(home)/work"]
        var best: (String, Int, Date)?
        for root in roots {
            guard let children = try? fm.contentsOfDirectory(atPath: root) else { continue }
            for child in children where !child.hasPrefix(".") {
                let path = root + "/" + child
                var isDir: ObjCBool = false
                guard fm.fileExists(atPath: path, isDirectory: &isDir), isDir.boolValue else { continue }
                var score = 0
                if fm.fileExists(atPath: path + "/.git") { score += 1 }
                if fm.fileExists(atPath: path + "/AGENTS.md") { score += 2 }
                if fm.fileExists(atPath: path + "/opencode.json") || fm.fileExists(atPath: path + "/opencode.jsonc") { score += 1 }
                guard score > 0 else { continue }
                let date = (try? fm.attributesOfItem(atPath: path)[.modificationDate] as? Date) ?? .distantPast
                if best == nil || score > best!.1 || (score == best!.1 && date > best!.2) {
                    best = (path, score, date)
                }
            }
        }
        return (best?.0, best == nil ? "no project found" : "detected a git project with agent instructions")
    }

    static func worker() -> (value: String?, source: String) {
        let fm = FileManager.default
        var candidates: [(String, Date)] = []
        for dir in ["\(home)/.local/bin", "\(home)/bin"] {
            guard let files = try? fm.contentsOfDirectory(atPath: dir) else { continue }
            for file in files {
                let lower = file.lowercased()
                guard lower.hasSuffix(".sh"), lower.contains("loop") || lower.contains("agent") else { continue }
                let path = dir + "/" + file
                guard fm.isExecutableFile(atPath: path) else { continue }
                let date = (try? fm.attributesOfItem(atPath: path)[.modificationDate] as? Date) ?? .distantPast
                candidates.append((path, date))
            }
        }
        let best = candidates.max { $0.1 < $1.1 }
        return (best?.0, best == nil ? "no worker script found" : "found an agent loop script")
    }

    static func state(forWorker worker: String) -> (value: String?, source: String) {
        let fm = FileManager.default
        var name = (worker as NSString).lastPathComponent
        if name.hasSuffix(".sh") { name = String(name.dropLast(3)) }
        if name.hasSuffix("-loop") { name = String(name.dropLast(5)) }
        let derived = "\(home)/.local/state/\(name)"
        if fm.fileExists(atPath: derived) { return (derived, "matched the worker script's name") }
        var candidates: [(String, Date)] = []
        if let dirs = try? fm.contentsOfDirectory(atPath: "\(home)/.local/state") {
            for dir in dirs {
                let path = "\(home)/.local/state/\(dir)"
                if fm.fileExists(atPath: path + "/loop.log") {
                    let date = (try? fm.attributesOfItem(atPath: path + "/loop.log")[.modificationDate] as? Date) ?? .distantPast
                    candidates.append((path, date))
                }
            }
        }
        let best = candidates.max { $0.1 < $1.1 }
        return (best?.0, best == nil ? "no state folder found" : "found the most recently active worker state")
    }
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

func startWorker() {
    guard !Prefs.worker.isEmpty else { return }
    bash("mkdir -p \"\(Prefs.stateDir)\"; rm -f \"\(Prefs.stopFile)\"; nohup caffeinate -is /bin/bash \"\(Prefs.worker)\" >/dev/null 2>&1 &")
}

func diffStyledText(_ text: String, font: NSFont) -> NSAttributedString {
    let attributed = NSMutableAttributedString()
    for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
        let value = String(line) + "\n"
        var color = NSColor(calibratedWhite: 0.86, alpha: 1)
        if (value.hasPrefix("+ ") || value.hasPrefix("+")) && !value.hasPrefix("+++") {
            color = .systemGreen
        } else if (value.hasPrefix("- ") || value.hasPrefix("-")) && !value.hasPrefix("---") {
            color = .systemRed
        } else if value.hasPrefix("@@") {
            color = .systemTeal
        }
        attributed.append(NSAttributedString(string: value, attributes: [.foregroundColor: color, .font: font]))
    }
    return attributed
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
        onNavigate?(3)
    }

    @objc func openFolder() {
        NSWorkspace.shared.open(URL(fileURLWithPath: Prefs.project))
    }

    @objc func startWork() {
        startWorker()
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
    struct RunCard {
        var key: String
        var title: String
        var subtitle: String
        var date: Date
        var status: Bool?
        var detail: String
    }

    let scroll = NSScrollView()
    let listStack = NSStackView()
    let summaryLabel = headerLabel("", size: 12, color: .secondaryLabelColor)
    var expanded = Set<String>()
    var visibleCards = 12
    var lastSignature = ""

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
        stack.addArrangedSubview(makeHeader("Live Activity", "Every agent run, packaged — expand for the full output"))
        stack.addArrangedSubview(summaryLabel)

        listStack.orientation = .vertical
        listStack.alignment = .leading
        listStack.spacing = 10
        listStack.edgeInsets = NSEdgeInsets(top: 6, left: 6, bottom: 14, right: 6)
        listStack.translatesAutoresizingMaskIntoConstraints = false
        let document = NSView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(listStack)
        NSLayoutConstraint.activate([
            listStack.topAnchor.constraint(equalTo: document.topAnchor),
            listStack.bottomAnchor.constraint(equalTo: document.bottomAnchor),
            listStack.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            listStack.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            document.widthAnchor.constraint(equalToConstant: 820),
        ])
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.documentView = document
        scroll.drawsBackground = false
        scroll.wantsLayer = true
        scroll.layer?.cornerRadius = 12
        scroll.layer?.borderWidth = 1
        scroll.layer?.borderColor = NSColor(calibratedWhite: 0.24, alpha: 1).cgColor
        stack.addArrangedSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52),
            scroll.bottomAnchor.constraint(equalTo: stack.bottomAnchor, constant: -22),
        ])
        addBanner(stack, key: "activity", text: "The worker's log, packaged: one card per agent run, a green check for successes, expand any card for the full captured output.")
        view = root
    }

    func parseCards() -> [RunCard] {
        guard let data = FileManager.default.contents(atPath: Prefs.logFile),
              let text = String(data: data, encoding: .utf8) else { return [] }
        struct Event {
            var stamp: String
            var date: Date
            var kind: String
            var title: String
            var ok: Bool?
            var detail: [String]
        }
        var events: [Event] = []
        var current: Event?
        func flush() {
            if let event = current { events.append(event) }
            current = nil
        }
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            if line.hasPrefix("["), let close = line.firstIndex(of: "]") {
                let stamp = String(line[line.index(after: line.startIndex)..<close])
                guard let date = logFormatter.date(from: stamp) else { continue }
                flush()
                let message = String(line.dropFirst(line.distance(from: line.startIndex, to: close) + 2))
                var kind = "note"
                var title = String(message.prefix(140))
                var ok: Bool?
                if message.contains("starting run") {
                    kind = "runStart"; title = "Agent run"
                } else if message.contains("run rc=") {
                    kind = "runEnd"; title = "Agent run finished"; ok = message.contains("rc=0")
                } else if message.contains("loop start") {
                    kind = "loopStart"; title = "24/7 loop started"
                } else if message.contains("loop end") {
                    kind = "loopStop"; title = "24/7 loop stopped"
                }
                current = Event(stamp: stamp, date: date, kind: kind, title: title, ok: ok, detail: [message])
            } else if current != nil {
                let size = current!.detail.reduce(0) { $0 + $1.count }
                if size < 12000 {
                    current!.detail.append(line)
                } else if current!.detail.last != "… output truncated …" {
                    current!.detail.append("… output truncated …")
                }
            }
        }
        flush()

        var cards: [RunCard] = []
        var pendingStart: Event?
        var pendingEnd: Event?
        func finishPending() {
            guard let start = pendingStart else { return }
            var lines = start.detail + (pendingEnd?.detail ?? [])
            lines.removeAll { $0 == "starting run" || $0.hasPrefix("run rc=") || $0.hasPrefix("session: ") }
            var detail = lines.joined(separator: "\n")
            if detail.isEmpty { detail = "No captured output for this run." }
            var subtitle = "in progress"
            var status: Bool?
            if let end = pendingEnd {
                let seconds = Int(end.date.timeIntervalSince(start.date))
                subtitle = "finished in \(seconds >= 60 ? "\(seconds / 60)m \(seconds % 60)s" : "\(seconds)s")"
                status = end.ok
            }
            cards.append(RunCard(key: start.stamp + "|run", title: "Agent run", subtitle: subtitle,
                                 date: start.date, status: status, detail: detail))
            pendingStart = nil
            pendingEnd = nil
        }
        for event in events {
            switch event.kind {
            case "runStart":
                finishPending()
                pendingStart = event
            case "runEnd":
                if pendingStart != nil { pendingEnd = event; finishPending() }
            case "loopStart", "loopStop":
                finishPending()
                cards.append(RunCard(key: event.stamp + "|" + event.kind, title: event.title,
                                     subtitle: "", date: event.date, status: nil, detail: ""))
            default:
                cards.append(RunCard(key: event.stamp + "|note", title: event.title,
                                     subtitle: "", date: event.date, status: nil, detail: event.detail.joined(separator: "\n")))
            }
        }
        finishPending()
        return cards
    }

    func makeCard(_ card: RunCard) -> NSView {
        let container = CardView()
        container.translatesAutoresizingMaskIntoConstraints = false
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 12, left: 14, bottom: 12, right: 14)
        stack.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: container.topAnchor),
            stack.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: container.trailingAnchor),
        ])

        let header = NSStackView()
        header.orientation = .horizontal
        header.spacing = 8
        let symbol: String
        let tint: NSColor
        if let ok = card.status, ok {
            symbol = "checkmark.circle.fill"; tint = .systemGreen
        } else if let ok = card.status, !ok {
            symbol = "xmark.circle.fill"; tint = .systemRed
        } else if card.subtitle == "in progress" {
            symbol = "circle.dotted"; tint = .systemOrange
        } else {
            symbol = "circle.fill"; tint = .systemTeal
        }
        let icon = NSImageView()
        icon.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        icon.contentTintColor = tint
        icon.translatesAutoresizingMaskIntoConstraints = false
        icon.widthAnchor.constraint(equalToConstant: 16).isActive = true
        let title = headerLabel(card.title, size: 13, weight: .semibold)
        let time = headerLabel(DateFormatter.localizedString(from: card.date, dateStyle: .none, timeStyle: .medium),
                                size: 11, color: .secondaryLabelColor)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        header.addArrangedSubview(icon)
        header.addArrangedSubview(title)
        if !card.subtitle.isEmpty {
            header.addArrangedSubview(headerLabel("· \(card.subtitle)", size: 11, color: .secondaryLabelColor))
        }
        header.addArrangedSubview(spacer)
        header.addArrangedSubview(time)
        stack.addArrangedSubview(header)
        header.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -28).isActive = true

        if !card.detail.isEmpty {
            let lines = card.detail.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            let preview = lines.first.map { String($0.prefix(130)) } ?? ""
            let previewLabel = headerLabel(preview.isEmpty ? " " : preview, size: 11, color: .tertiaryLabelColor)
            previewLabel.lineBreakMode = .byTruncatingTail
            stack.addArrangedSubview(previewLabel)
            previewLabel.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -28).isActive = true

            let actions = NSStackView()
            actions.orientation = .horizontal
            actions.spacing = 12
            let isExpanded = expanded.contains(card.key)
            let toggle = NSButton(title: isExpanded ? "Hide details" : "Show details", target: self, action: #selector(toggleDetails(_:)))
            toggle.isBordered = false
            toggle.font = .systemFont(ofSize: 11, weight: .medium)
            toggle.contentTintColor = .systemTeal
            toggle.identifier = NSUserInterfaceItemIdentifier(card.key)
            actions.addArrangedSubview(toggle)
            let copy = NSButton(title: "Copy output", target: self, action: #selector(copyOutput(_:)))
            copy.isBordered = false
            copy.font = .systemFont(ofSize: 11, weight: .medium)
            copy.contentTintColor = .secondaryLabelColor
            copy.identifier = NSUserInterfaceItemIdentifier(card.key)
            actions.addArrangedSubview(copy)
            if let sessionLine = card.detail.split(separator: "\n").first(where: { $0.hasPrefix("session: ") }) {
                let sessionID = String(sessionLine.dropFirst("session: ".count)).trimmingCharacters(in: .whitespaces)
                let open = NSButton(title: "Open transcript", target: self, action: #selector(openCardSession(_:)))
                open.isBordered = false
                open.font = .systemFont(ofSize: 11, weight: .medium)
                open.contentTintColor = .systemBlue
                open.identifier = NSUserInterfaceItemIdentifier(sessionID)
                actions.addArrangedSubview(open)
            }
            stack.addArrangedSubview(actions)

            if isExpanded {
                let detail = NSTextView()
                detail.isEditable = false
                detail.isRichText = false
                detail.drawsBackground = true
                detail.backgroundColor = NSColor(calibratedWhite: 0.08, alpha: 1)
                detail.textColor = NSColor(calibratedWhite: 0.85, alpha: 1)
                detail.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
                detail.textContainerInset = NSSize(width: 8, height: 8)
                detail.frame = NSRect(x: 0, y: 0, width: 700, height: 200)
                detail.isVerticallyResizable = true
                detail.autoresizingMask = [.width]
                detail.textContainer?.widthTracksTextView = true
                detail.string = card.detail
                let detailScroll = NSScrollView()
                detailScroll.translatesAutoresizingMaskIntoConstraints = false
                detailScroll.hasVerticalScroller = true
                detailScroll.documentView = detail
                detailScroll.wantsLayer = true
                detailScroll.layer?.cornerRadius = 8
                detailScroll.layer?.borderWidth = 1
                detailScroll.layer?.borderColor = NSColor(calibratedWhite: 0.22, alpha: 1).cgColor
                stack.addArrangedSubview(detailScroll)
                detailScroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -28).isActive = true
                let lineCount = card.detail.split(separator: "\n").count
                let height = min(360, max(80, lineCount * 14 + 24))
                detailScroll.heightAnchor.constraint(equalToConstant: CGFloat(height)).isActive = true
            }
        }
        return container
    }

    @objc func toggleDetails(_ sender: NSButton) {
        guard let key = sender.identifier?.rawValue else { return }
        if expanded.contains(key) {
            expanded.remove(key)
        } else {
            expanded.insert(key)
        }
        rebuild()
    }

    @objc func copyOutput(_ sender: NSButton) {
        guard let key = sender.identifier?.rawValue else { return }
        if let card = parseCards().first(where: { $0.key == key }) {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(card.detail, forType: .string)
        }
    }

    @objc func openCardSession(_ sender: NSButton) {
        guard let sessionID = sender.identifier?.rawValue, !sessionID.isEmpty else { return }
        SessionTranscriptController.open(sessionID: sessionID, title: "Agent run")
    }

    func rebuild() {
        let cards = parseCards()
        let today = Calendar.current.startOfDay(for: Date())
        let runsToday = cards.filter { $0.title == "Agent run" && $0.date >= today }
        let finished = runsToday.filter { $0.status != nil }
        let succeeded = finished.filter { $0.status == true }.count
        let lastFinished = cards.last { $0.status != nil }
        var summary = "runs today: \(runsToday.count)  ·  succeeded: \(succeeded)/\(finished.count)  ·  latest: \(lastFinished?.subtitle ?? "—")"
        if let live = cards.last(where: { $0.title == "Agent run" && $0.subtitle == "in progress" }) {
            let seconds = Int(Date().timeIntervalSince(live.date))
            summary += "  ·  running for \(seconds >= 60 ? "\(seconds / 60)m \(seconds % 60)s" : "\(seconds)s")"
        }
        summaryLabel.stringValue = summary

        let signature = cards.map { $0.key + ($0.status.map { $0 ? "+" : "-" } ?? "?") }.joined(separator: "|")
            + "#\(expanded.sorted().joined(separator: ","))#\(visibleCards)"
        if signature == lastSignature { return }
        lastSignature = signature

        let origin = scroll.contentView.bounds.origin
        listStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        let shown = cards.suffix(visibleCards)
        for card in shown.reversed() {
            let view = makeCard(card)
            listStack.addArrangedSubview(view)
            view.widthAnchor.constraint(equalTo: listStack.widthAnchor, constant: -12).isActive = true
        }
        if cards.count > visibleCards {
            let earlier = NSButton(title: "Show earlier runs", target: self, action: #selector(showEarlier))
            earlier.bezelStyle = .rounded
            listStack.addArrangedSubview(earlier)
        }
        scroll.contentView.scroll(to: origin)
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    @objc func showEarlier() {
        visibleCards += 12
        rebuild()
    }

    override func refresh() {
        rebuild()
    }
}

final class IncrementsPane: Pane {
    let table = NSTableView()
    let detail = NSTextView()
    var rows: [(String, String, String)] = []
    var lastCount = 0
    var currentCommit = ""

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
        stack.addArrangedSubview(makeHeader("Increments", "Click a commit to inspect it — message, files and diff"))

        let columns = NSStackView()
        columns.orientation = .horizontal
        columns.spacing = 14
        columns.alignment = .top
        columns.setContentHuggingPriority(.defaultLow, for: .vertical)

        for (title, width) in [("Hash", CGFloat(64)), ("Increment", CGFloat(300)), ("When", CGFloat(110))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(title))
            column.title = title
            column.width = width
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = true
        table.style = .inset
        table.toolTip = "Click a row to inspect the message, files and diff"
        let listScroll = NSScrollView()
        listScroll.translatesAutoresizingMaskIntoConstraints = false
        listScroll.hasVerticalScroller = true
        listScroll.documentView = table
        listScroll.wantsLayer = true
        listScroll.layer?.cornerRadius = 12
        listScroll.layer?.borderWidth = 1
        listScroll.layer?.borderColor = NSColor(calibratedWhite: 0.24, alpha: 1).cgColor
        listScroll.widthAnchor.constraint(equalToConstant: 500).isActive = true

        detail.isEditable = false
        detail.isRichText = true
        detail.drawsBackground = true
        detail.backgroundColor = NSColor(calibratedWhite: 0.10, alpha: 1)
        detail.textContainerInset = NSSize(width: 12, height: 12)
        detail.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        detail.isVerticallyResizable = true
        detail.isHorizontallyResizable = false
        detail.autoresizingMask = [.width]
        detail.textContainer?.widthTracksTextView = true
        detail.minSize = NSSize(width: 0, height: 0)
        detail.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        let detailScroll = NSScrollView()
        detailScroll.translatesAutoresizingMaskIntoConstraints = false
        detailScroll.hasVerticalScroller = true
        detailScroll.documentView = detail
        detailScroll.wantsLayer = true
        detailScroll.layer?.cornerRadius = 12
        detailScroll.layer?.borderWidth = 1
        detailScroll.layer?.borderColor = NSColor(calibratedWhite: 0.24, alpha: 1).cgColor

        columns.addArrangedSubview(listScroll)
        columns.addArrangedSubview(detailScroll)
        stack.addArrangedSubview(columns)
        NSLayoutConstraint.activate([
            columns.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52),
            listScroll.heightAnchor.constraint(equalTo: columns.heightAnchor),
            detailScroll.heightAnchor.constraint(equalTo: columns.heightAnchor),
        ])
        addBanner(stack, key: "increments", text: "Every committed work increment, newest first. Click a row to see the message, the changed files and the full diff right here.")
        view = root
    }

    @objc func openCommit() {
        let row = table.selectedRow
        if row >= 0 && row < rows.count {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(rows[row].0, forType: .string)
        }
    }

    func loadDetail() {
        let row = table.selectedRow
        guard row >= 0, row < rows.count else { return }
        let hash = rows[row].0
        if hash == currentCommit { return }
        currentCommit = hash
        let result = run("/usr/bin/git", ["-C", Prefs.project, "show", "--stat", "--patch",
                                          "--format=commit %h%nAuthor: %an <%ae>%nDate:   %ad%n%n%s%n%n%b", hash])
        var text = result.output
        if text.count > 160000 { text = String(text.prefix(160000)) + "\n… output truncated …\n" }
        let attributed = NSMutableAttributedString()
        let base = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        let strong = NSFont.monospacedSystemFont(ofSize: 11, weight: .semibold)
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let value = String(line) + "\n"
            var color = NSColor(calibratedWhite: 0.85, alpha: 1)
            var font = base
            if value.hasPrefix("+") && !value.hasPrefix("+++") {
                color = .systemGreen
            } else if value.hasPrefix("-") && !value.hasPrefix("---") {
                color = .systemRed
            } else if value.hasPrefix("@@") {
                color = .systemTeal
            } else if value.hasPrefix("commit ") || value.hasPrefix("Author:") || value.hasPrefix("Date:") || value.hasPrefix("diff --git") {
                font = strong
            }
            attributed.append(NSAttributedString(string: value, attributes: [.foregroundColor: color, .font: font]))
        }
        detail.textStorage?.setAttributedString(attributed)
        detail.scrollToBeginningOfDocument(nil)
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
        if table.selectedRow < 0 && !rows.isEmpty {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            loadDetail()
        }
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
    let search = NSSearchField()
    var filter = ""
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
        let controls = NSStackView()
        controls.orientation = .horizontal
        controls.spacing = 10
        search.placeholderString = "Filter sessions"
        search.translatesAutoresizingMaskIntoConstraints = false
        search.widthAnchor.constraint(equalToConstant: 300).isActive = true
        search.target = self
        search.action = #selector(filterChanged)
        search.sendsSearchStringImmediately = true
        controls.addArrangedSubview(search)
        stack.addArrangedSubview(controls)
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
        table.toolTip = "Double-click a session to open its full step-by-step transcript"
        addBanner(stack, key: "sessions", text: "OpenCode sessions for this project. Double-click one for the full step-by-step transcript — every tool call, edit and thought as expandable cards.")
        view = root
    }

    @objc func openSession() {
        let row = table.selectedRow
        guard row >= 0, row < rows.count else { return }
        SessionTranscriptController.open(sessionID: rows[row].2, title: rows[row].0)
    }

    @objc func filterChanged() {
        filter = search.stringValue
        refresh()
    }

    override func refresh() {
        let result = run("\(home)/.opencode/bin/opencode", ["session", "list"], cwd: Prefs.project)
        let parsed = result.output.split(separator: "\n").map { line -> (String, String, String) in
            let parts = line.split(separator: "\t", maxSplits: 2).map(String.init)
            if parts.count >= 3 { return (parts[1], parts[2], parts[0]) }
            return (String(line), "", "")
        }
        rows = filter.isEmpty ? parsed : parsed.filter { $0.0.lowercased().contains(filter.lowercased()) }
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
            let urlString = capture("(http://\\S+)", in: result.output)
            DispatchQueue.main.async {
                guard result.code == 0, let urlString, let url = URL(string: urlString) else {
                    self.stateLabel.stringValue = "Service unavailable"
                    self.showMessage("Could not create a live link.<br><br>Make sure the OpenCode service is running, then press Reconnect.")
                    return
                }
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
    let projectSource = headerLabel("", size: 10, color: .tertiaryLabelColor)
    let workerSource = headerLabel("", size: 10, color: .tertiaryLabelColor)
    let stateSource = headerLabel("", size: 10, color: .tertiaryLabelColor)
    let infoLabel = headerLabel("", size: 11, color: .secondaryLabelColor)
    let autoStartBox = NSButton(checkboxWithTitle: "Start work automatically when AgentWatcher opens",
                                target: nil, action: nil)

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
        stack.addArrangedSubview(makeHeader("Settings", "Detected automatically — edit any field to override"))
        let fields: [(String, NSTextField, NSTextField)] = [
            ("Project folder", projectField, projectSource),
            ("Worker script", workerField, workerSource),
            ("State folder", stateField, stateSource),
        ]
        for (title, field, source) in fields {
            stack.addArrangedSubview(headerLabel(title, size: 12, weight: .semibold))
            field.translatesAutoresizingMaskIntoConstraints = false
            field.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
            field.target = self
            field.action = #selector(fieldChanged)
            field.toolTip = "Changes apply immediately"
            stack.addArrangedSubview(field)
            field.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -52).isActive = true
            stack.addArrangedSubview(source)
        }
        autoStartBox.target = self
        autoStartBox.action = #selector(autoStartChanged)
        stack.addArrangedSubview(autoStartBox)
        let redetect = NSButton(title: "Re-detect everything", target: self, action: #selector(redetect))
        redetect.bezelStyle = .rounded
        redetect.toolTip = "Forget overrides and detect the project, worker and state again"
        stack.addArrangedSubview(redetect)
        stack.addArrangedSubview(infoLabel)
        addBanner(stack, key: "settings", text: "Everything here is detected automatically: AgentWatcher finds a git project with agent instructions, an agent loop script, and the worker's state folder. Edit a field to override; Re-detect forgets the overrides. No project files are modified.")
        view = root
    }

    override func refresh() {
        if projectField.currentEditor() == nil { projectField.stringValue = Prefs.project }
        if workerField.currentEditor() == nil { workerField.stringValue = Prefs.worker }
        if stateField.currentEditor() == nil { stateField.stringValue = Prefs.stateDir }
        projectSource.stringValue = "auto: " + AutoDetect.project().source
        workerSource.stringValue = "auto: " + AutoDetect.worker().source
        stateSource.stringValue = "auto: " + AutoDetect.state(forWorker: Prefs.worker).source
        autoStartBox.state = Prefs.autoStart ? .on : .off
        let logExists = FileManager.default.fileExists(atPath: Prefs.logFile)
        infoLabel.stringValue = logExists ? "Log found: \(Prefs.logFile)" : "No log yet at \(Prefs.logFile)"
    }

    @objc func fieldChanged() {
        if !projectField.stringValue.isEmpty { Prefs.project = projectField.stringValue }
        Prefs.worker = workerField.stringValue
        if !stateField.stringValue.isEmpty { Prefs.stateDir = stateField.stringValue }
        refresh()
    }

    @objc func autoStartChanged() {
        Prefs.autoStart = autoStartBox.state == .on
    }

    @objc func redetect() {
        Prefs.defaults.removeObject(forKey: "project")
        Prefs.defaults.removeObject(forKey: "worker")
        Prefs.defaults.removeObject(forKey: "state")
        refresh()
    }
}



final class ProcessesPane: Pane {
    struct ProcessRow {
        var pid: String
        var cpu: String
        var mem: String
        var elapsed: String
        var command: String
    }
    let table = NSTableView()
    let infoLabel = headerLabel("", size: 11, color: .secondaryLabelColor)
    var rows: [ProcessRow] = []

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
        stack.addArrangedSubview(makeHeader("Processes", "The agent's live OS processes"))
        for (title, width) in [("PID", CGFloat(70)), ("CPU %", CGFloat(60)), ("MEM %", CGFloat(60)),
                               ("Elapsed", CGFloat(80)), ("Command", CGFloat(540))] {
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
        let controls = NSStackView()
        controls.orientation = .horizontal
        controls.spacing = 10
        let kill = NSButton(title: "Stop Selected Process", target: self, action: #selector(killSelected))
        kill.bezelStyle = .rounded
        kill.toolTip = "Send SIGTERM to the selected process"
        controls.addArrangedSubview(kill)
        controls.addArrangedSubview(infoLabel)
        stack.addArrangedSubview(controls)
        addBanner(stack, key: "processes", text: "Every live process that belongs to the agent: the worker loop, its keep-awake helper and the OpenCode runs it spawns. Stop Selected Process ends one cleanly with SIGTERM.")
        view = root
    }

    override func refresh() {
        let result = run("/bin/ps", ["-axo", "pid=,pcpu=,pmem=,etime=,command="])
        var patterns: [String] = ["caffeinate", "opencode"]
        let workerName = (Prefs.worker as NSString).lastPathComponent
        if !workerName.isEmpty { patterns.append(workerName) }
        rows = []
        for line in result.output.split(separator: "\n") {
            let parts = line.split(separator: " ", maxSplits: 4, omittingEmptySubsequences: true).map(String.init)
            guard parts.count >= 5 else { continue }
            let command = parts[4]
            guard patterns.contains(where: { command.contains($0) }) else { continue }
            guard !command.contains("AgentWatcher"), !command.contains("ps -axo"), !command.contains("grep") else { continue }
            rows.append(ProcessRow(pid: parts[0], cpu: parts[1], mem: parts[2], elapsed: parts[3],
                                   command: String(command.prefix(220))))
        }
        infoLabel.stringValue = rows.isEmpty ? "no agent processes running" : "\(rows.count) agent processes"
        table.reloadData()
    }

    @objc func killSelected() {
        let row = table.selectedRow
        guard row >= 0, row < rows.count else { return }
        run("/bin/kill", ["-TERM", rows[row].pid])
        refresh()
    }
}

extension ProcessesPane: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView()
        let field = NSTextField(labelWithString: "")
        field.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
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
        case "PID": field.stringValue = rows[row].pid
        case "CPU %": field.stringValue = rows[row].cpu
        case "MEM %": field.stringValue = rows[row].mem
        case "Elapsed": field.stringValue = rows[row].elapsed
        default: field.stringValue = rows[row].command
        }
        return cell
    }
}

final class SessionTranscriptController: NSObject, NSWindowDelegate {
    static var windows: [SessionTranscriptController] = []

    let window: NSWindow
    let scroll = NSScrollView()
    let stack = NSStackView()
    let titleLabel = NSTextField(labelWithString: "")
    let statusLabel = headerLabel("Loading transcript…", size: 11, color: .secondaryLabelColor)
    let sessionID: String
    var expanded = Set<String>()
    var parts: [(key: String, kind: String, title: String, subtitle: String, body: String, tint: NSColor, monospaced: Bool, openByDefault: Bool)] = []

    static func open(sessionID: String, title: String) {
        let controller = SessionTranscriptController(sessionID: sessionID, title: title)
        windows.append(controller)
        controller.window.makeKeyAndOrderFront(nil)
    }

    init(sessionID: String, title: String) {
        self.sessionID = sessionID
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1040, height: 780),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        super.init()
        window.title = "Session — \(title)"
        window.appearance = NSAppearance(named: .darkAqua)
        window.delegate = self
        window.center()
        buildUI()
        load()
    }

    func windowWillClose(_ notification: Notification) {
        SessionTranscriptController.windows.removeAll { $0 === self }
    }

    func buildUI() {
        let root = NSView()
        let header = NSStackView()
        header.orientation = .horizontal
        header.spacing = 10
        titleLabel.stringValue = "Session transcript"
        titleLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        header.addArrangedSubview(titleLabel)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        header.addArrangedSubview(spacer)
        header.addArrangedSubview(statusLabel)
        let reload = NSButton(title: "Reload", target: self, action: #selector(reloadTapped))
        reload.bezelStyle = .rounded
        let copy = NSButton(title: "Copy ID", target: self, action: #selector(copyID))
        copy.bezelStyle = .rounded
        header.addArrangedSubview(copy)
        header.addArrangedSubview(reload)
        header.translatesAutoresizingMaskIntoConstraints = false

        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 6, left: 6, bottom: 14, right: 6)
        stack.translatesAutoresizingMaskIntoConstraints = false
        let document = NSView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: document.topAnchor),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: document.trailingAnchor),
        ])
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.documentView = document
        scroll.drawsBackground = false
        root.addSubview(header)
        root.addSubview(scroll)
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: root.topAnchor, constant: 14),
            header.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 18),
            header.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -18),
            scroll.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 10),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            scroll.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -12),
            document.widthAnchor.constraint(equalTo: scroll.widthAnchor),
        ])
        window.contentView = root
    }

    @objc func reloadTapped() { load() }
    @objc func copyID() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(sessionID, forType: .string)
        statusLabel.stringValue = "Session id copied"
    }

    func load() {
        statusLabel.stringValue = "Loading transcript…"
        let id = sessionID
        DispatchQueue.global().async {
            let result = run("\(home)/.opencode/bin/opencode", ["session", "export", id], cwd: Prefs.project)
            let data = result.output.data(using: .utf8) ?? Data()
            let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            let messages = json?["messages"] as? [[String: Any]] ?? []
            DispatchQueue.main.async { self.build(messages: messages, error: result.code) }
        }
    }

    func firstLine(_ text: String) -> String {
        let line = text.split(separator: "\n").first.map(String.init) ?? ""
        return String(line.prefix(140))
    }

    func summarize(name: String, input: [String: Any]) -> String {
        func value(_ key: String) -> String? { input[key] as? String }
        switch name {
        case "bash", "shell": return firstLine(value("command") ?? "")
        case "read", "edit", "write", "patch": return value("filePath") ?? value("path") ?? ""
        case "grep", "glob": return value("pattern") ?? ""
        default:
            if let key = input.keys.sorted().first, let value = input[key] as? String { return "\(key): \(String(value.prefix(100)))" }
            return ""
        }
    }

    func build(messages: [[String: Any]], error: Int32) {
        parts = []
        var counter = 0
        for message in messages {
            let type = message["type"] as? String ?? ""
            if type == "user" {
                let text = message["text"] as? String ?? ""
                parts.append((key: "u\(counter)", kind: "user", title: "You", subtitle: firstLine(text),
                              body: text, tint: .systemBlue, monospaced: false, openByDefault: true))
                counter += 1
            } else {
                for part in message["content"] as? [[String: Any]] ?? [] {
                    let kind = part["type"] as? String ?? ""
                    if kind == "text" {
                        let text = part["text"] as? String ?? ""
                        parts.append((key: "a\(counter)", kind: "text", title: "Agent", subtitle: firstLine(text),
                                      body: text, tint: .systemTeal, monospaced: false, openByDefault: true))
                    } else if kind == "reasoning" {
                        let text = part["text"] as? String ?? ""
                        parts.append((key: "r\(counter)", kind: "reasoning", title: "Thinking", subtitle: firstLine(text),
                                      body: text, tint: .systemPurple, monospaced: false, openByDefault: false))
                    } else if kind == "tool" {
                        let name = part["name"] as? String ?? "tool"
                        let state = part["state"] as? [String: Any] ?? [:]
                        let status = state["status"] as? String ?? ""
                        let input = state["input"] as? [String: Any] ?? [:]
                        let output = (state["content"] as? [[String: Any]] ?? [])
                            .compactMap { $0["text"] as? String }.joined(separator: "\n")
                        var body = ""
                        if let data = try? JSONSerialization.data(withJSONObject: input, options: [.prettyPrinted, .sortedKeys]),
                           let text = String(data: data, encoding: .utf8) {
                            body = text
                        }
                        if !output.isEmpty {
                            body += "\n\n── output ──\n" + output
                        }
                        if body.count > 9000 { body = String(body.prefix(9000)) + "\n… truncated …" }
                        let tint: NSColor = status == "completed" ? .systemGreen : (status == "failed" ? .systemRed : .systemOrange)
                        parts.append((key: "t\(counter)", kind: "tool", title: name,
                                      subtitle: summarize(name: name, input: input),
                                      body: body, tint: tint, monospaced: true, openByDefault: false))
                    }
                    counter += 1
                }
            }
        }
        // Bound rendering to the most recent parts.
        if parts.count > 400 {
            let skipped = parts.count - 400
            parts = Array(parts.suffix(400))
            statusLabel.stringValue = "\(skipped) earlier steps hidden · showing the last 400"
        } else {
            statusLabel.stringValue = error == 0 ? "\(parts.count) steps" : "export failed (rc=\(error))"
        }
        render()
    }

    func render() {
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for part in parts {
            let open = part.openByDefault || expanded.contains(part.key)
            stack.addArrangedSubview(makeCard(part, open: open))
        }
        if let last = stack.arrangedSubviews.last {
            last.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -12).isActive = true
        }
        for view in stack.arrangedSubviews {
            view.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -12).isActive = true
        }
    }

    @objc func togglePart(_ sender: NSButton) {
        guard let key = sender.identifier?.rawValue else { return }
        if expanded.contains(key) { expanded.remove(key) } else { expanded.insert(key) }
        render()
    }

    func makeCard(_ part: (key: String, kind: String, title: String, subtitle: String, body: String, tint: NSColor, monospaced: Bool, openByDefault: Bool), open: Bool) -> NSView {
        let container = CardView()
        container.translatesAutoresizingMaskIntoConstraints = false
        let inner = NSStackView()
        inner.orientation = .vertical
        inner.alignment = .leading
        inner.spacing = 6
        inner.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
        inner.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(inner)
        NSLayoutConstraint.activate([
            inner.topAnchor.constraint(equalTo: container.topAnchor),
            inner.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            inner.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            inner.trailingAnchor.constraint(equalTo: container.trailingAnchor),
        ])

        let header = NSStackView()
        header.orientation = .horizontal
        header.spacing = 8
        let icon = NSImageView()
        let symbol: String
        switch part.kind {
        case "user": symbol = "person.crop.circle"
        case "text": symbol = "text.bubble"
        case "reasoning": symbol = "brain"
        default: symbol = part.monospaced ? "terminal" : "circle"
        }
        icon.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        icon.contentTintColor = part.tint
        icon.translatesAutoresizingMaskIntoConstraints = false
        icon.widthAnchor.constraint(equalToConstant: 15).isActive = true
        header.addArrangedSubview(icon)
        header.addArrangedSubview(headerLabel(part.title, size: 12, weight: .semibold))
        if !part.subtitle.isEmpty {
            let subtitle = headerLabel(part.subtitle, size: 11, color: .secondaryLabelColor)
            subtitle.lineBreakMode = .byTruncatingTail
            header.addArrangedSubview(subtitle)
        }
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        header.addArrangedSubview(spacer)
        if !part.body.isEmpty {
            let toggle = NSButton(title: open ? "Hide" : "Show", target: self, action: #selector(togglePart(_:)))
            toggle.isBordered = false
            toggle.font = .systemFont(ofSize: 11, weight: .medium)
            toggle.contentTintColor = .secondaryLabelColor
            toggle.identifier = NSUserInterfaceItemIdentifier(part.key)
            header.addArrangedSubview(toggle)
        }
        inner.addArrangedSubview(header)
        header.widthAnchor.constraint(equalTo: inner.widthAnchor, constant: -24).isActive = true

        if open && !part.body.isEmpty {
            let text = NSTextView()
            text.isEditable = false
            text.isRichText = false
            text.drawsBackground = true
            text.backgroundColor = NSColor(calibratedWhite: 0.08, alpha: 1)
            text.textColor = NSColor(calibratedWhite: 0.86, alpha: 1)
            text.font = part.monospaced
                ? .monospacedSystemFont(ofSize: 11, weight: .regular)
                : .systemFont(ofSize: 12)
            text.textContainerInset = NSSize(width: 8, height: 8)
            text.frame = NSRect(x: 0, y: 0, width: 800, height: 160)
            text.isVerticallyResizable = true
            text.autoresizingMask = [.width]
            text.textContainer?.widthTracksTextView = true
            text.string = part.body
            let size = min(360, max(60, part.body.split(separator: "\n").count * 15 + 20))
            let textScroll = NSScrollView()
            textScroll.translatesAutoresizingMaskIntoConstraints = false
            textScroll.hasVerticalScroller = true
            textScroll.documentView = text
            textScroll.wantsLayer = true
            textScroll.layer?.cornerRadius = 8
            textScroll.layer?.borderWidth = 1
            textScroll.layer?.borderColor = NSColor(calibratedWhite: 0.22, alpha: 1).cgColor
            inner.addArrangedSubview(textScroll)
            textScroll.widthAnchor.constraint(equalTo: inner.widthAnchor, constant: -24).isActive = true
            textScroll.heightAnchor.constraint(equalToConstant: CGFloat(size)).isActive = true
        }
        return container
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
        Section(title: "Processes", symbol: "cpu", tint: .systemPink) { ProcessesPane() },
        Section(title: "Increments", symbol: "checklist", tint: .systemGreen) { IncrementsPane() },
        Section(title: "Sessions", symbol: "bubble.left.and.bubble.right", tint: .systemPurple) { SessionsPane() },
        Section(title: "Live Sessions", symbol: "globe", tint: .systemOrange) { LivePane() },
        Section(title: "Settings", symbol: "gearshape", tint: .systemGray) { SettingsPane() },
    ]

    func applicationDidFinishLaunching(_ notification: Notification) {
        buildMenu()
        panes = sections.map { $0.make() }
        buildWindow()
        if Prefs.autoStart && !Prefs.worker.isEmpty && !AgentStatus.current().running {
            startWorker()
        }
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
        let pane = panes[index]
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
