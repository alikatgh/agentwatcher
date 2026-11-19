import AppKit
import WebKit

// AgentWatcher — a button-based macOS companion that watches and steers a
// continuous background coding agent (OpenCode). Sidebar navigation,
// live activity feed, hourly activity chart, increments table, embedded
// live session viewer and settings. No terminal required.

let home = NSHomeDirectory()

// MARK: - Theme (semantic roles shared by both native appearances)

enum Theme {
    static func hex(_ value: UInt32) -> NSColor {
        NSColor(srgbRed: CGFloat((value >> 16) & 0xFF) / 255.0,
                green: CGFloat((value >> 8) & 0xFF) / 255.0,
                blue: CGFloat(value & 0xFF) / 255.0,
                alpha: 1)
    }
    static func dynamic(light: UInt32, dark: UInt32, alpha: CGFloat = 1,
                        contrastAlpha: CGFloat? = nil) -> NSColor {
        NSColor(name: nil) { appearance in
            let match = appearance.bestMatch(from: [.aqua, .darkAqua,
                                                     .accessibilityHighContrastAqua,
                                                     .accessibilityHighContrastDarkAqua])
            let darkAppearance = match == .darkAqua || match == .accessibilityHighContrastDarkAqua
            let highContrast = match == .accessibilityHighContrastAqua || match == .accessibilityHighContrastDarkAqua
            return hex(darkAppearance ? dark : light)
                .withAlphaComponent(highContrast ? (contrastAlpha ?? alpha) : alpha)
        }
    }
    static let base = dynamic(light: 0xF7F8FA, dark: 0x191C1F)
    static let deep = dynamic(light: 0xFFFFFF, dark: 0x16191C)
    static let layer01 = dynamic(light: 0xEFF1F4, dark: 0x212429)
    static let layer02 = dynamic(light: 0xE5E8EE, dark: 0x282D34)
    static let layer03 = dynamic(light: 0xDDE2E9, dark: 0x303741)
    static let textBase = dynamic(light: 0x242A33, dark: 0xD9DDE4)
    static let textMuted = dynamic(light: 0x485260, dark: 0xB5BDCA)
    static let textFaint = dynamic(light: 0x4C5665, dark: 0xB0B8C6)
    static let iconBase = dynamic(light: 0x485260, dark: 0xBEC6D3)
    static let primary = dynamic(light: 0x34558A, dark: 0xA9C5F0)
    static let accent = dynamic(light: 0x2545B8, dark: 0xA2BCFF)
    static let accentStrong = dynamic(light: 0x3153C7, dark: 0x3B5CF6)
    static let success = dynamic(light: 0x176135, dark: 0x9BCEA9)
    static let warning = dynamic(light: 0x7D5A00, dark: 0xF2CF76)
    static let danger = dynamic(light: 0x9D252F, dark: 0xF0A0A5)
    static let plan = dynamic(light: 0x993660, dark: 0xF799C6)
    static let explore = dynamic(light: 0x745A1A, dark: 0xF7E5B5)
    static let review = dynamic(light: 0x226C3D, dark: 0xB8E9C1)
    static let writer = dynamic(light: 0x5D4CC0, dark: 0xB6B1FF)
    static let border = dynamic(light: 0x252520, dark: 0xFFFFFF, alpha: 0.12, contrastAlpha: 0.30)
    static let borderMuted = dynamic(light: 0x252520, dark: 0xFFFFFF, alpha: 0.08, contrastAlpha: 0.22)
    static let borderStrong = dynamic(light: 0x252520, dark: 0xFFFFFF, alpha: 0.24, contrastAlpha: 0.45)
    static var appearance: NSAppearance? { AppearanceController.preference.appearance }
    // Core Animation holds CGColor snapshots. The appearance owner resolves these
    // same roles again without replacing views, their data, or their selection.
    static var semanticColors: [NSColor] {
        [base, deep, layer01, layer02, layer03, textBase, textMuted, textFaint,
         iconBase, primary, accent, accentStrong, success, warning, danger,
         plan, explore, review, writer, border, borderMuted, borderStrong]
    }

    static func uiFont(_ size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont {
        if weight == .regular, let inter = NSFont(name: "Inter", size: size) { return inter }
        return .systemFont(ofSize: size, weight: weight)
    }
    static func codeFont(_ size: CGFloat) -> NSFont {
        for name in ["JetBrainsMonoNFM-Regular", "JetBrainsMono Nerd Font Mono", "IBMPlexMono-Text", "Menlo"] {
            if let font = NSFont(name: name, size: size) { return font }
        }
        return .monospacedSystemFont(ofSize: size, weight: .regular)
    }
}

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
    static var sidebarVisible: Bool {
        get { defaults.object(forKey: "sidebarVisible") == nil ? true : defaults.bool(forKey: "sidebarVisible") }
        set { defaults.set(newValue, forKey: "sidebarVisible") }
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
    var known = true
    var running = false
    var pausing = false
    var blocked = false

    static func workerPattern(_ worker: String) -> String {
        "(^|[[:space:]])(/[^[:space:]]*/)?(bash|sh|zsh|dash|ksh)([[:space:]]+-[A-Za-z]+)*[[:space:]]+"
            + NSRegularExpression.escapedPattern(for: worker) + "([[:space:]]|$)"
    }

    var title: String {
        if !known { return "Status unavailable" }
        if blocked { return "Needs attention" }
        if running && pausing { return "Pausing…" }
        if running { return "Working" }
        if pausing { return "Paused" }
        return "Stopped"
    }
    var color: NSColor {
        if blocked { return Theme.danger }
        if running && pausing { return Theme.warning }
        if running { return Theme.success }
        if pausing { return Theme.warning }
        return Theme.textMuted
    }
    static func current(project: String = Prefs.project, worker: String = Prefs.worker,
                        stateDir: String = Prefs.stateDir) -> AgentStatus {
        var status = AgentStatus()
        if !worker.isEmpty {
            let result = run("/usr/bin/pgrep", ["-f", workerPattern(worker)], timeout: 2)
            status.known = result.code == 0 || result.code == 1
            status.running = result.code == 0
        }
        status.pausing = FileManager.default.fileExists(atPath: stateDir + "/STOP")
        status.blocked = FileManager.default.fileExists(atPath: project + "/agent-loop-REPORT.md")
        return status
    }
}

// MARK: - Log parsing

struct LogEvent {
    var date: Date
    var line: String
}

func makeLogFormatter() -> DateFormatter {
    let formatter = DateFormatter()
    formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.timeZone = .current
    return formatter
}

func logEvents(atPath path: String = Prefs.logFile) -> [LogEvent] {
    let logFormatter = makeLogFormatter()
    guard let data = FileManager.default.contents(atPath: path),
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
    let result = repositoryCommand(Prefs.project, ["log", "--since=\(since)", "--pretty=format:%ct|%h|%s"])
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
        var color = Theme.textBase
        if (value.hasPrefix("+ ") || value.hasPrefix("+")) && !value.hasPrefix("+++") {
            color = Theme.success
        } else if (value.hasPrefix("- ") || value.hasPrefix("-")) && !value.hasPrefix("---") {
            color = Theme.danger
        } else if value.hasPrefix("@@") {
            color = Theme.primary
        }
        attributed.append(NSAttributedString(string: value, attributes: [.foregroundColor: color, .font: font]))
    }
    return attributed
}

func numberedText(_ text: String, size: CGFloat) -> NSAttributedString {
    let font = Theme.codeFont(size)
    let numberAttributes: [NSAttributedString.Key: Any] = [
        .foregroundColor: Theme.textFaint,
        .font: font,
    ]
    let attributed = NSMutableAttributedString()
    var index = 1
    for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
        let value = String(line)
        var color = Theme.textBase
        if (value.hasPrefix("+ ") || value.hasPrefix("+")) && !value.hasPrefix("+++") {
            color = Theme.success
        } else if (value.hasPrefix("- ") || value.hasPrefix("-")) && !value.hasPrefix("---") {
            color = Theme.danger
        } else if value.hasPrefix("@@") {
            color = Theme.primary
        }
        attributed.append(NSAttributedString(string: String(format: "%5d │ ", index), attributes: numberAttributes))
        attributed.append(NSAttributedString(string: value + "\n", attributes: [.foregroundColor: color, .font: font]))
        index += 1
    }
    return attributed
}

// MARK: - Small views

final class PulseDot: NSView {
    var color: NSColor = Theme.success { didSet { needsDisplay = true } }
    private var phase: CGFloat = 0
    private var timer: Timer?
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        timer?.invalidate()
        timer = nil
        guard window != nil, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            guard let self, self.window?.isVisible == true else { return }
            self.phase += 0.16
            self.needsDisplay = true
        }
        RunLoop.main.add(timer!, forMode: .common)
    }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ dirtyRect: NSRect) {
        let radius = min(bounds.width, bounds.height) * 0.30
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
        layer?.backgroundColor = Theme.layer01.themeCGColor
        layer?.cornerRadius = 4
        layer?.borderWidth = 1
        layer?.borderColor = Theme.border.themeCGColor
    }
    required init?(coder: NSCoder) { fatalError() }
}

final class ClickRow: NSView {
    var onHover: ((Bool) -> Void)?
    private var tracking: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeInKeyWindow],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
    }
    override func mouseEntered(with event: NSEvent) { onHover?(true) }
    override func mouseExited(with event: NSEvent) { onHover?(false) }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .pointingHand)
    }
}

final class InfoBanner: NSView {
    private var key = ""
    init(key: String, text: String) {
        super.init(frame: .zero)
        self.key = key
        wantsLayer = true
        layer?.backgroundColor = Theme.layer01.themeCGColor
        layer?.cornerRadius = 4
        layer?.borderWidth = 1
        layer?.borderColor = Theme.border.themeCGColor
        let icon = NSImageView()
        icon.image = NSImage(systemSymbolName: "info.circle", accessibilityDescription: nil)
        icon.contentTintColor = Theme.primary
        icon.translatesAutoresizingMaskIntoConstraints = false
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: 11)
        label.textColor = Theme.textMuted
        label.translatesAutoresizingMaskIntoConstraints = false
        let close = NSButton(title: "", target: self, action: #selector(closeTapped))
        close.image = NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Hide explanation")
        close.isBordered = false
        close.contentTintColor = Theme.textFaint
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
        layer?.backgroundColor = Theme.danger.withAlphaComponent(0.18).themeCGColor
        layer?.cornerRadius = 9
        let label = NSTextField(labelWithString: "LIVE")
        label.font = .systemFont(ofSize: 10, weight: .bold)
        label.textColor = Theme.danger
        label.translatesAutoresizingMaskIntoConstraints = false
        let dot = NSView()
        dot.wantsLayer = true
        dot.layer?.backgroundColor = Theme.danger.themeCGColor
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

// MARK: - Detail panes

// Collect blocking data on a serial queue. While it runs, retain only the
// newest request, and never publish a superseded result into the UI.
final class LatestRequestLoader<Input: Equatable, Output> {
    private let queue: DispatchQueue
    private let collect: (Input) -> Output
    var apply: ((Input, Output) -> Void)?
    private var active: Input?
    private var pending: Input?
    private var latest: Input?

    init(label: String, collect: @escaping (Input) -> Output) {
        queue = DispatchQueue(label: label, qos: .userInitiated)
        self.collect = collect
    }

    func request(_ input: Input) {
        precondition(Thread.isMainThread)
        latest = input
        if let active {
            pending = active == input ? nil : input
        } else {
            start(input)
        }
    }

    private func start(_ input: Input) {
        active = input
        let collect = self.collect
        queue.async { [weak self] in
            let output = collect(input)
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                if self.latest == input { self.apply?(input, output) }
                self.active = nil
                if let next = self.pending {
                    self.pending = nil
                    self.start(next)
                }
            }
        }
    }
}

protocol Refreshable: AnyObject {
    func refresh()
}

func headerLabel(_ text: String, size: CGFloat, weight: NSFont.Weight = .regular, color: NSColor = Theme.textBase) -> NSTextField {
    let field = NSTextField(labelWithString: text)
    field.font = Theme.uiFont(size, weight: weight)
    field.textColor = color
    field.lineBreakMode = .byTruncatingTail
    field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    field.setContentHuggingPriority(.required, for: .vertical)
    field.setContentCompressionResistancePriority(.required, for: .vertical)
    return field
}

class Pane: NSViewController, Refreshable {
    var onNavigate: ((Int) -> Void)?
    var onInspectCommit: ((String) -> Void)?
    var onInspectRun: ((String) -> Void)?
    var onInspectDetails: ((NSViewController) -> Void)?
    var onCloseDetails: (() -> Void)?
    func refresh() {}

    @discardableResult
    func addBanner(_ stack: NSStackView, key: String, text: String) -> InfoBanner {
        let banner = InfoBanner(key: key, text: text)
        stack.addArrangedSubview(banner)
        banner.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        return banner
    }
    func makeHeader(_ title: String, _ subtitle: String) -> NSStackView {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.distribution = .fill
        stack.alignment = .leading
        stack.spacing = 2
        for field in [headerLabel(title, size: 16, weight: .semibold),
                      headerLabel(subtitle, size: 12, color: Theme.textMuted)] {
            stack.addArrangedSubview(field)
            field.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        return stack
    }
}

final class OverviewPane: Pane {
    var onOperation: (() -> Void)?
    let statusDot = PulseDot(frame: .zero)
    let statusLabel = headerLabel("", size: 13, weight: .semibold)
    let spinner = NSProgressIndicator()
    let metricLabel = headerLabel("…", size: 32, weight: .bold)
    let deltaLabel = headerLabel("", size: 15, weight: .semibold, color: Theme.success)
    let metricSub = headerLabel("", size: 12, color: Theme.textMuted)
    let chart = LineChartView()
    let chartUpdated = headerLabel("", size: 11, color: Theme.textMuted)
    let chartTitle = headerLabel("Git and worker activity", size: 14, weight: .semibold)
    let recentStack = NSStackView()
    let startButton = NSButton(title: "Start", target: nil, action: nil)
    let pauseButton = NSButton(title: "Pause", target: nil, action: nil)
    let stopButton = NSButton(title: "Force Stop", target: nil, action: nil)
    let chipIncrements = NSButton(title: "", target: nil, action: nil)
    let chipRuns = NSButton(title: "", target: nil, action: nil)
    let rangeControl = NSSegmentedControl(labels: ["1D", "5D", "1M"], trackingMode: .selectOne, target: nil, action: nil)
    private var chartTimeline: ChartTimeline?
    let telemetryView = TelemetryDashboardView()
    let telemetrySource = NSPopUpButton(frame: .zero, pullsDown: false)
    let telemetryState = headerLabel("Loading recorded agent activity…", size: 11, color: Theme.textMuted)
    typealias TelemetryRequest = TelemetrySources.Request
    typealias TelemetryResult = TelemetrySources.Result
    private var telemetrySelection = "automatic"
    private var telemetryRequest: TelemetryRequest?
    private var telemetrySessions: [DeepSeekTelemetry.SessionDescriptor] = []
    private var telemetryCodexSessions: [CodexTelemetry.SessionDescriptor] = []
    private var lastTelemetryLoad = Date.distantPast
    private var pendingTelemetryInspection: String?
    private lazy var telemetryLoader = LatestRequestLoader<TelemetryRequest, TelemetryResult>(label: "AgentWatcher.telemetry", collect: TelemetrySources.collect)

    override func loadView() {
        let root = NSView()
        let stack = makePage(in: root, scrolling: true)

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
        stack.addArrangedSubview(headerLabel("Agent activity", size: 12, weight: .semibold, color: Theme.textMuted))

        let controls = NSStackView()
        controls.orientation = .horizontal
        controls.spacing = 10
        styleChip(chipIncrements, title: "Increments", color: Theme.primary)
        styleChip(chipRuns, title: "Runs", color: Theme.accent)
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
        chartStack.distribution = .fill
        chartStack.alignment = .leading
        chartStack.spacing = 10
        chartStack.translatesAutoresizingMaskIntoConstraints = false
        chartCard.addSubview(chartStack)
        let chartHeader = NSStackView()
        chartHeader.orientation = .horizontal
        chartHeader.spacing = 10
        chartHeader.addArrangedSubview(chartTitle)
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
            chart.heightAnchor.constraint(equalToConstant: 190),
        ])
        stack.addArrangedSubview(chartCard)
        chartCard.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        chart.toolTip = "Click either line to inspect commits and agent runs at that time"
        chart.onSelect = { [weak self] index, series, anchor in self?.inspectBucket(index, series: series, anchor: anchor) }

        let buttonRow = NSStackView()
        buttonRow.orientation = .horizontal
        buttonRow.spacing = 10
        for button in [startButton, pauseButton, stopButton] {
            button.bezelStyle = .rounded
            button.isEnabled = false
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

        let telemetryControls = NSStackView()
        telemetryControls.orientation = .horizontal
        telemetryControls.spacing = 10
        telemetryControls.addArrangedSubview(headerLabel("Agent trajectory", size: 14, weight: .semibold))
        telemetrySource.addItems(withTitles: ["Latest detected agent", "OpenCode · watched worker"])
        telemetrySource.item(at: 0)?.representedObject = "automatic"
        telemetrySource.item(at: 1)?.representedObject = "opencode"
        telemetrySource.target = self
        telemetrySource.action = #selector(telemetrySourceChanged)
        telemetrySource.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        telemetrySource.widthAnchor.constraint(lessThanOrEqualToConstant: 420).isActive = true
        telemetryControls.addArrangedSubview(telemetrySource)
        let diagram = NSButton(title: "View agent diagram", target: self, action: #selector(goActivity))
        diagram.bezelStyle = .rounded
        diagram.controlSize = .small
        telemetryControls.addArrangedSubview(diagram)
        stack.addArrangedSubview(telemetryControls)
        stack.addArrangedSubview(telemetryState)
        stack.addArrangedSubview(telemetryView)
        telemetryView.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        telemetryView.onOpenSession = { [weak self] id in self?.openTelemetrySession(id) }

        let recentCard = CardView()
        recentCard.translatesAutoresizingMaskIntoConstraints = false
        recentStack.orientation = .vertical
        recentStack.distribution = .fill
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
        recentCard.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

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
            .foregroundColor: Theme.textBase,
            .font: NSFont.systemFont(ofSize: 12, weight: .medium),
        ]))
        button.attributedTitle = attributed
    }

    @objc func chipToggled() { renderSnapshot() }

    @objc func rangeChanged() {
        closeInspector()
        renderSnapshot()
    }

    @objc func goActivity() { onNavigate?(1) }

    @objc func cycleRange() {
        rangeControl.selectedSegment = (rangeControl.selectedSegment + 1) % 3
        renderSnapshot()
    }

    @objc func openRecent(_ sender: NSButton) {
        if let hash = sender.identifier?.rawValue { onInspectCommit?(hash) }
    }

    @objc func openFolder() {
        NSWorkspace.shared.open(URL(fileURLWithPath: Prefs.project))
    }

    @objc func startWork() {
        guard !operationInFlight, let snapshot, snapshot.status.known, !snapshot.status.running,
              snapshot.request.project == Prefs.project, snapshot.request.worker == Prefs.worker,
              snapshot.request.stateDir == Prefs.stateDir else { return }
        beginOperation("Starting…")
        startWorker()
        refresh()
    }
    @objc func pauseWork() {
        guard !operationInFlight, snapshot?.status.running == true else { return }
        beginOperation("Pausing…")
        bash("mkdir -p \"\(Prefs.stateDir)\"; touch \"\(Prefs.stopFile)\"")
        refresh()
    }
    @objc func forceStop() {
        guard !operationInFlight, snapshot?.status.running == true else { return }
        beginOperation("Stopping…")
        if !Prefs.worker.isEmpty { run("/usr/bin/pkill", ["-f", AgentStatus.workerPattern(Prefs.worker)]) }
        refresh()
    }

    struct Request: Equatable {
        var project: String
        var worker: String
        var stateDir: String
        var revision: Int
    }
    struct Snapshot {
        var request: Request
        var status: AgentStatus
        var commits: [(date: Date, hash: String, subject: String)]
        var cards: [ActivityPane.RunCard]
        var updated: Date
        var error: String?
    }
    private var snapshot: Snapshot?
    private var recentSignature = ""
    private var revision = 0
    private(set) var operationInFlight = false
    func beginOperation(_ title: String) {
        operationInFlight = true
        revision += 1
        onOperation?()
        startButton.isEnabled = false
        pauseButton.isEnabled = false
        stopButton.isEnabled = false
        statusLabel.stringValue = title
    }
    private lazy var loader = LatestRequestLoader<Request, Snapshot>(label: "AgentWatcher.overview") { request in
        let status = AgentStatus.current(project: request.project, worker: request.worker, stateDir: request.stateDir)
        let result = repositoryCommand(request.project, ["log", "--since=30.days", "--pretty=format:%ct|%h|%s"])
        let commits = result.output.split(separator: "\n").compactMap { line -> (date: Date, hash: String, subject: String)? in
            let parts = line.split(separator: "|", maxSplits: 2).map(String.init)
            guard parts.count == 3, let epoch = Double(parts[0]) else { return nil }
            return (Date(timeIntervalSince1970: epoch), parts[1], parts[2])
        }
        let log = (try? String(contentsOfFile: request.stateDir + "/loop.log", encoding: .utf8)) ?? ""
        return Snapshot(request: request, status: status, commits: commits,
                        cards: ActivityPane.parseCards(text: log), updated: Date(),
                        error: result.code == 0 ? nil : "Commit history unavailable")
    }

    func renderSnapshot() {
        guard let snapshot else { return }
        let status = snapshot.status
        statusLabel.stringValue = status.title
        statusDot.color = status.color
        spinner.stopAnimation(nil)
        startButton.isEnabled = status.known && !status.running && !snapshot.request.worker.isEmpty
        pauseButton.isEnabled = status.running
        stopButton.isEnabled = status.running
        let today = Calendar.current.startOfDay(for: snapshot.updated)
        let yesterday = Calendar.current.date(byAdding: .day, value: -1, to: today) ?? today.addingTimeInterval(-86400)
        let todayCount = snapshot.commits.filter { $0.date >= today }.count
        let yesterdayCount = snapshot.commits.filter { $0.date >= yesterday && $0.date < today }.count
        metricLabel.stringValue = snapshot.error == nil ? "\(todayCount)" : "—"
        let delta = todayCount - yesterdayCount
        deltaLabel.stringValue = snapshot.error == nil ? "\(delta >= 0 ? "+" : "")\(delta) vs yesterday" : ""
        deltaLabel.textColor = delta >= 0 ? Theme.success : Theme.danger
        let projectName = (snapshot.request.project as NSString).lastPathComponent
        metricSub.stringValue = snapshot.error ?? "increments landed today · \(projectName)"
        chartTitle.stringValue = "Git and worker activity · \(projectName)"
        let stamp = DateFormatter.localizedString(from: snapshot.updated, dateStyle: .none, timeStyle: .short)
        chartUpdated.stringValue = "Updated \(stamp)"
        let range = rangeControl.selectedSegment
        let timeline = ChartTimeline(updated: snapshot.updated, range: range)
        chartTimeline = timeline
        let count = timeline.count
        var commits = [CGFloat](repeating: 0, count: count)
        var runs = commits
        for commit in snapshot.commits {
            if let index = timeline.bucketIndex(for: commit.date) { commits[index] += 1 }
        }
        for card in snapshot.cards where card.title == "Agent run" {
            if let index = timeline.bucketIndex(for: card.date) { runs[index] += 1 }
        }
        chart.series = [
            LineChartView.Series(name: "Increments", color: Theme.primary, values: commits, visible: chipIncrements.state == .on),
            LineChartView.Series(name: "Runs", color: Theme.accent, values: runs, visible: chipRuns.state == .on),
        ]
        chart.bucketLabels = (0..<count).map { timeline.label(for: $0) }
        chart.needsDisplay = true
        let recent = Array(snapshot.commits.prefix(5))
        let signature = recent.map { $0.hash + $0.subject }.joined(separator: "|") + (snapshot.error ?? "")
        if signature != recentSignature || recentStack.arrangedSubviews.isEmpty {
            recentSignature = signature
            recentStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
            recentStack.addArrangedSubview(headerLabel("Recent increments", size: 12, weight: .semibold))
            for commit in recent {
                let button = NSButton(title: "\(commit.hash)  \(commit.subject)", target: self, action: #selector(openRecent(_:)))
                button.isBordered = false
                button.alignment = .left
                button.font = Theme.codeFont(11)
                button.lineBreakMode = .byTruncatingTail
                button.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
                button.toolTip = "Inspect increment \(commit.hash)"
                button.identifier = NSUserInterfaceItemIdentifier(commit.hash)
                recentStack.addArrangedSubview(button)
                button.widthAnchor.constraint(equalTo: recentStack.widthAnchor).isActive = true
            }
            if recent.isEmpty { recentStack.addArrangedSubview(headerLabel(snapshot.error ?? "No increments in the last 30 days.", size: 12, color: Theme.textMuted)) }
        }
    }

    func inspectionItems(bucket index: Int) -> [ChartInspectionItem] {
        guard let snapshot, let timeline = chartTimeline, (0..<timeline.count).contains(index) else { return [] }
        let commits = snapshot.commits.filter { timeline.bucketIndex(for: $0.date) == index }.map {
            ChartInspectionItem(date: $0.date, title: $0.subject, detail: "Increment \($0.hash) · Open diff", kind: .commit, identifier: $0.hash)
        }
        let interval = timeline.interval(for: index)
        let cards = snapshot.cards.filter { card in
            if timeline.bucketIndex(for: card.date) == index { return true }
            // Include a run already active at the clicked time, even if it
            // started in an earlier bucket and made commits in this one.
            guard card.title == "Agent run", card.date <= interval.end else { return false }
            return card.finishedAt.map { $0 > interval.start } ?? true
        }.map {
            ChartInspectionItem(date: $0.date, title: $0.title, detail: $0.subtitle.isEmpty ? "Open activity details" : $0.subtitle + " · Open activity details",
                                kind: $0.title == "Agent run" ? .run : .note, identifier: $0.key)
        }
        return (commits + cards).sorted { $0.date < $1.date }
    }

    func inspectBucket(_ index: Int, series: String, anchor: NSRect) {
        guard let timeline = chartTimeline, let snapshot, (0..<timeline.count).contains(index) else { return }
        let request = snapshot.request
        closeInspector()
        let inspector = ChartInspectorController(title: series + " · " + timeline.label(for: index), items: inspectionItems(bucket: index)) { [weak self] item in
            guard let self, request.project == Prefs.project, request.stateDir == Prefs.stateDir else { return }
            self.closeInspector()
            if item.kind == .commit { self.onInspectCommit?(item.identifier) }
            else { self.onInspectRun?(item.identifier) }
        }
        inspector.onClose = { [weak self] in self?.onCloseDetails?() }
        onInspectDetails?(inspector)
    }

    func closeInspector() { onCloseDetails?() }

    @objc func telemetrySourceChanged() {
        telemetrySelection = telemetrySource.selectedItem?.representedObject as? String ?? "automatic"
        refreshTelemetry(force: true)
    }

    func openTelemetrySession(_ id: String) {
        if telemetryCodexSessions.contains(where: { $0.id == id }) { telemetrySelection = "codex:" + id }
        else if let session = telemetrySessions.first(where: { $0.id == id }) { telemetrySelection = session.url.path }
        else if SessionsPaneData.isSessionID(id) { telemetrySelection = "opencode:" + id }
        else { telemetryState.stringValue = "The linked session is not available in the detected records."; return }
        pendingTelemetryInspection = telemetrySelection
        refreshTelemetry(force: true)
    }

    private func refreshTelemetry(force: Bool = false) {
        let request = TelemetryRequest(project: Prefs.project, logFile: Prefs.logFile, selection: telemetrySelection)
        guard force || request != telemetryRequest || Date().timeIntervalSince(lastTelemetryLoad) >= 15 else { return }
        telemetryRequest = request
        lastTelemetryLoad = Date()
        telemetryState.stringValue = "Reading recorded session activity…"
        telemetryLoader.apply = { [weak self] request, result in
            guard let self, self.telemetryRequest == request, request.project == Prefs.project,
                  request.logFile == Prefs.logFile else { return }
            self.telemetrySessions = result.sessions
            self.telemetryCodexSessions = result.codexSessions
            self.telemetrySource.removeAllItems()
            let options = [("Latest detected agent", "automatic"), ("Codex · latest project session", "codex"), ("OpenCode · watched worker", "opencode")]
                + result.codexSessions.map { ("Codex · " + $0.title, "codex:" + $0.id) }
                + result.sessions.map { ("DSH · " + $0.title, $0.url.path) }
            for (title, key) in options {
                self.telemetrySource.addItem(withTitle: title)
                self.telemetrySource.lastItem?.representedObject = key
                if key == request.selection { self.telemetrySource.select(self.telemetrySource.lastItem) }
            }
            if request.selection.hasPrefix("opencode:") {
                self.telemetrySource.addItem(withTitle: "OpenCode · " + result.snapshot.title)
                self.telemetrySource.lastItem?.representedObject = request.selection
                self.telemetrySource.select(self.telemetrySource.lastItem)
            }
            self.telemetryView.update(result.snapshot)
            self.telemetryState.stringValue = result.snapshot.error ?? result.snapshot.source + " · " + result.snapshot.project
            if self.pendingTelemetryInspection == request.selection {
                self.pendingTelemetryInspection = nil
                self.telemetryView.inspect()
            }
        }
        telemetryLoader.request(request)
    }

    func applySnapshot(_ result: Snapshot) {
        operationInFlight = false
        snapshot = result
        renderSnapshot()
    }

    override func refresh() {
        refreshTelemetry()
        let request = Request(project: Prefs.project, worker: Prefs.worker, stateDir: Prefs.stateDir, revision: revision)
        if snapshot?.request != request {
            closeInspector()
            snapshot = nil
            startButton.isEnabled = false
            pauseButton.isEnabled = false
            stopButton.isEnabled = false
            metricLabel.stringValue = "…"
            if !operationInFlight { statusLabel.stringValue = "Checking agent…" }
            spinner.startAnimation(nil)
            recentSignature = ""
        }
        loader.apply = { [weak self] request, result in
            guard let self, request.project == Prefs.project,
                  request.worker == Prefs.worker, request.stateDir == Prefs.stateDir,
                  request.revision == self.revision else { return }
            self.applySnapshot(result)
        }
        loader.request(request)
    }

}

final class ActivityPane: Pane {
    let workController = AgentWorkController()
    let activityMode = NSSegmentedControl(labels: ["Agent work", "Worker log"], trackingMode: .selectOne, target: nil, action: nil)
    let agentSource = NSPopUpButton(frame: .zero, pullsDown: false)
    private var workerRoot: NSView?
    private var workSelection = "automatic"
    private var workRequest: TelemetrySources.Request?
    private var workSessions: [DeepSeekTelemetry.SessionDescriptor] = []
    private var workCodexSessions: [CodexTelemetry.SessionDescriptor] = []
    private var lastWorkLoad = Date.distantPast
    private lazy var workLoader = LatestRequestLoader<TelemetrySources.Request, TelemetrySources.Result>(label: "AgentWatcher.work-graph", collect: TelemetrySources.collect)
    struct RunCard {
        var key: String
        var title: String
        var subtitle: String
        var date: Date
        var status: Bool?
        var detail: String
        var sessionID: String
        var finishedAt: Date? = nil
    }

    let listScroll = NSScrollView()
    let listStack = NSStackView()
    let summaryLabel = headerLabel("", size: 12, color: Theme.textMuted)
    let tabPicker = NSSegmentedControl(labels: ["Output", "Context"], trackingMode: .selectOne, target: nil, action: nil)
    let rightTitle = headerLabel("", size: 13, weight: .semibold)
    let rightStats = headerLabel("", size: 11, color: Theme.textMuted)
    let filterField = NSSearchField()
    let outputView = NSTextView()
    let contextStack = NSStackView()
    let contextScroll = NSScrollView()
    var outputContainer = NSView()
    var contextContainer = NSView()
    let outputEmpty = EmptyStateView(title: "No captured output", message: "This run has no saved console output. Choose Context to inspect its recorded session.")

    var cards: [RunCard] = []
    var selectedKey = ""
    private var pendingInspection: String?
    private var historyAnchor: String?
    var visibleCards = 20
    var lastSignature = ""
    var filter = ""
    var expandedMessages = Set<String>()
    var sessionCache: [String: (info: [String: Any], messages: [[String: Any]])] = [:]
    var contextLoading = false
    private var lastOutput: (key: String, filter: String, text: String)?
    private var lastContextSignature = ""
    private var activityRequest = ""
    struct ActivitySnapshot { var cards: [RunCard]; var error: String? }
    private lazy var activityLoader = LatestRequestLoader<String, ActivitySnapshot>(label: "AgentWatcher.activity") { path in
        guard FileManager.default.fileExists(atPath: path) else {
            return ActivitySnapshot(cards: [], error: "No activity log yet. Runs appear here when the worker starts writing its log.")
        }
        guard let text = try? String(contentsOfFile: path, encoding: .utf8) else {
            return ActivitySnapshot(cards: [], error: "The activity log could not be read. Check its folder in Settings.")
        }
        return ActivitySnapshot(cards: Self.parseCards(text: text), error: nil)
    }
    private var activityError: String?

    override func loadView() {
        let root = NSView()
        let stack = makePage(in: root, scrolling: false)
        let workerHeader = makeHeader("Worker log", "Recorded worker runs, console output and session context")
        stack.addArrangedSubview(workerHeader)
        workerHeader.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        stack.addArrangedSubview(summaryLabel)

        let columns = NSView()
        columns.translatesAutoresizingMaskIntoConstraints = false
        columns.setContentHuggingPriority(.defaultLow, for: .vertical)

        listStack.orientation = .vertical
        listStack.distribution = .fill
        listStack.alignment = .leading
        listStack.spacing = 8
        listStack.edgeInsets = NSEdgeInsets(top: 6, left: 6, bottom: 14, right: 6)
        listStack.translatesAutoresizingMaskIntoConstraints = false
        let listDocument = FlippedView()
        listDocument.translatesAutoresizingMaskIntoConstraints = false
        listDocument.addSubview(listStack)
        NSLayoutConstraint.activate([
            listStack.topAnchor.constraint(equalTo: listDocument.topAnchor),
            listStack.bottomAnchor.constraint(equalTo: listDocument.bottomAnchor),
            listStack.leadingAnchor.constraint(equalTo: listDocument.leadingAnchor),
            listStack.trailingAnchor.constraint(equalTo: listDocument.trailingAnchor),
        ])
        listScroll.translatesAutoresizingMaskIntoConstraints = false
        listScroll.hasVerticalScroller = true
        listScroll.documentView = listDocument
        listScroll.drawsBackground = false
        listScroll.wantsLayer = true
        listScroll.layer?.cornerRadius = 4
        listScroll.layer?.borderWidth = 1
        listScroll.layer?.borderColor = Theme.border.themeCGColor
        NSLayoutConstraint.activate([
            listDocument.leadingAnchor.constraint(equalTo: listScroll.contentView.leadingAnchor),
            listDocument.topAnchor.constraint(equalTo: listScroll.contentView.topAnchor),
            listDocument.widthAnchor.constraint(equalTo: listScroll.contentView.widthAnchor),
        ])

        let right = CardView()
        right.translatesAutoresizingMaskIntoConstraints = false
        let rightStack = NSStackView()
        rightStack.orientation = .vertical
        rightStack.distribution = .fill
        rightStack.alignment = .leading
        rightStack.spacing = 10
        rightStack.edgeInsets = NSEdgeInsets(top: 12, left: 14, bottom: 12, right: 14)
        rightStack.translatesAutoresizingMaskIntoConstraints = false
        right.addSubview(rightStack)
        NSLayoutConstraint.activate([
            rightStack.topAnchor.constraint(equalTo: right.topAnchor),
            rightStack.bottomAnchor.constraint(equalTo: right.bottomAnchor),
            rightStack.leadingAnchor.constraint(equalTo: right.leadingAnchor),
            rightStack.trailingAnchor.constraint(equalTo: right.trailingAnchor),
        ])
        let tabRow = NSStackView()
        tabRow.orientation = .horizontal
        tabRow.spacing = 10
        tabPicker.selectedSegment = 0
        tabPicker.target = self
        tabPicker.action = #selector(tabChanged)
        tabRow.addArrangedSubview(tabPicker)
        tabRow.addArrangedSubview(rightTitle)
        rightStack.addArrangedSubview(tabRow)
        rightStats.lineBreakMode = .byTruncatingTail
        rightStats.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        rightStack.addArrangedSubview(rightStats)
        rightStats.widthAnchor.constraint(equalTo: rightStack.widthAnchor, constant: -28).isActive = true

        filterField.placeholderString = "Filter output"
        filterField.translatesAutoresizingMaskIntoConstraints = false
        filterField.target = self
        filterField.action = #selector(filterChanged)
        filterField.sendsSearchStringImmediately = true
        rightStack.addArrangedSubview(filterField)
        filterField.widthAnchor.constraint(equalTo: rightStack.widthAnchor, constant: -28).isActive = true

        outputView.isEditable = false
        outputView.isRichText = true
        outputView.drawsBackground = true
        outputView.backgroundColor = Theme.deep
        outputView.textContainerInset = NSSize(width: 10, height: 10)
        outputView.frame = NSRect(x: 0, y: 0, width: 700, height: 400)
        outputView.isVerticallyResizable = true
        outputView.isHorizontallyResizable = false
        outputView.autoresizingMask = [.width]
        outputView.textContainer?.widthTracksTextView = true
        outputView.minSize = NSSize(width: 0, height: 0)
        outputView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        let outputScroll = WrappingTextScrollView()
        outputScroll.translatesAutoresizingMaskIntoConstraints = false
        outputScroll.hasVerticalScroller = true
        outputScroll.documentView = outputView
        outputScroll.wantsLayer = true
        outputScroll.layer?.cornerRadius = 3
        outputScroll.layer?.borderWidth = 1
        outputScroll.layer?.borderColor = Theme.borderMuted.themeCGColor
        outputContainer = outputScroll

        contextStack.orientation = .vertical
        contextStack.distribution = .fill
        contextStack.alignment = .leading
        contextStack.spacing = 8
        contextStack.edgeInsets = NSEdgeInsets(top: 8, left: 8, bottom: 12, right: 8)
        contextStack.translatesAutoresizingMaskIntoConstraints = false
        let contextDocument = FlippedView()
        contextDocument.translatesAutoresizingMaskIntoConstraints = false
        contextDocument.addSubview(contextStack)
        NSLayoutConstraint.activate([
            contextStack.topAnchor.constraint(equalTo: contextDocument.topAnchor),
            contextStack.bottomAnchor.constraint(equalTo: contextDocument.bottomAnchor),
            contextStack.leadingAnchor.constraint(equalTo: contextDocument.leadingAnchor),
            contextStack.trailingAnchor.constraint(equalTo: contextDocument.trailingAnchor),
        ])
        contextScroll.translatesAutoresizingMaskIntoConstraints = false
        contextScroll.hasVerticalScroller = true
        contextScroll.documentView = contextDocument
        NSLayoutConstraint.activate([
            contextDocument.leadingAnchor.constraint(equalTo: contextScroll.contentView.leadingAnchor),
            contextDocument.topAnchor.constraint(equalTo: contextScroll.contentView.topAnchor),
            contextDocument.widthAnchor.constraint(equalTo: contextScroll.contentView.widthAnchor),
        ])
        contextScroll.drawsBackground = false
        contextScroll.wantsLayer = true
        contextScroll.layer?.cornerRadius = 3
        contextScroll.layer?.borderWidth = 1
        contextScroll.layer?.borderColor = Theme.borderMuted.themeCGColor
        contextContainer = contextScroll

        rightStack.addArrangedSubview(outputScroll)
        rightStack.addArrangedSubview(contextScroll)
        rightStack.addArrangedSubview(outputEmpty)
        outputEmpty.translatesAutoresizingMaskIntoConstraints = false
        outputEmpty.isHidden = true
        outputEmpty.setContentHuggingPriority(.defaultLow, for: .vertical)
        outputEmpty.heightAnchor.constraint(greaterThanOrEqualToConstant: 160).isActive = true
        for container in [outputScroll, contextScroll, outputEmpty] {
            container.widthAnchor.constraint(equalTo: rightStack.widthAnchor, constant: -28).isActive = true
        }
        contextScroll.isHidden = true

        pinColumns(in: columns, left: listScroll, right: right, leftFraction: 0.34, minimum: 240, maximum: 360)
        stack.addArrangedSubview(columns)
        columns.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        addBanner(stack, key: "activity", text: "Every agent run on the left; the selected run opens in the right sidebar. Output shows the full captured log with a filter; Context shows the session's message list with expandable JSON.")
        workerRoot = root
        let shell = NSView()
        let controls = NSStackView(views: [activityMode, agentSource, NSView()])
        controls.orientation = .horizontal
        controls.spacing = 12
        controls.arrangedSubviews.last?.setContentHuggingPriority(.defaultLow, for: .horizontal)
        activityMode.selectedSegment = 0
        activityMode.target = self
        activityMode.action = #selector(activityModeChanged)
        agentSource.addItems(withTitles: ["Latest detected agent", "OpenCode · watched worker"])
        agentSource.item(at: 0)?.representedObject = "automatic"
        agentSource.item(at: 1)?.representedObject = "opencode"
        agentSource.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        agentSource.widthAnchor.constraint(lessThanOrEqualToConstant: 420).isActive = true
        agentSource.target = self
        agentSource.action = #selector(agentSourceChanged)
        workController.onOpenSession = { [weak self] id in self?.openWorkSession(id) }
        workController.onInspectDetails = { [weak self] controller in self?.onInspectDetails?(controller) }
        addChild(workController)
        for child in [controls, root, workController.view] {
            child.translatesAutoresizingMaskIntoConstraints = false
            shell.addSubview(child)
        }
        NSLayoutConstraint.activate([
            controls.topAnchor.constraint(equalTo: shell.topAnchor, constant: 12),
            controls.leadingAnchor.constraint(equalTo: shell.leadingAnchor, constant: 20),
            controls.trailingAnchor.constraint(equalTo: shell.trailingAnchor, constant: -20),
            controls.heightAnchor.constraint(equalToConstant: 26)
        ])
        for child in [root, workController.view] {
            NSLayoutConstraint.activate([
                child.topAnchor.constraint(equalTo: controls.bottomAnchor, constant: 8),
                child.leadingAnchor.constraint(equalTo: shell.leadingAnchor),
                child.trailingAnchor.constraint(equalTo: shell.trailingAnchor),
                child.bottomAnchor.constraint(equalTo: shell.bottomAnchor)
            ])
        }
        root.isHidden = true
        view = shell
    }

    @objc func activityModeChanged() {
        workerRoot?.isHidden = activityMode.selectedSegment == 0
        workController.view.isHidden = activityMode.selectedSegment != 0
        agentSource.isHidden = activityMode.selectedSegment != 0
        onCloseDetails?()
        refresh()
    }

    @objc func agentSourceChanged() {
        workSelection = agentSource.selectedItem?.representedObject as? String ?? "automatic"
        refreshWork(force: true)
    }

    func openWorkSession(_ id: String) {
        if workCodexSessions.contains(where: { $0.id == id }) { workSelection = "codex:" + id }
        else if let session = workSessions.first(where: { $0.id == id }) { workSelection = session.url.path }
        else if SessionsPaneData.isSessionID(id) { workSelection = "opencode:" + id }
        else { workController.showError("This linked session is not present in the detected records."); return }
        refreshWork(force: true)
    }

    private func refreshWork(force: Bool = false) {
        guard isViewLoaded, activityMode.selectedSegment == 0 else { return }
        let request = TelemetrySources.Request(project: Prefs.project, logFile: Prefs.logFile, selection: workSelection)
        guard force || request != workRequest || Date().timeIntervalSince(lastWorkLoad) >= 15 else { return }
        if request != workRequest { workController.showLoading("Reading recorded agent and tool history…") }
        workRequest = request
        lastWorkLoad = Date()
        workLoader.apply = { [weak self] request, result in
            guard let self, self.workRequest == request, request.project == Prefs.project, request.logFile == Prefs.logFile else { return }
            self.workSessions = result.sessions
            self.workCodexSessions = result.codexSessions
            self.agentSource.removeAllItems()
            let options = [("Latest detected agent", "automatic"), ("Codex · latest project session", "codex"), ("OpenCode · watched worker", "opencode")]
                + result.codexSessions.map { ("Codex · " + $0.title, "codex:" + $0.id) }
                + result.sessions.map { ("DSH · " + $0.title, $0.url.path) }
            for (title, key) in options {
                self.agentSource.addItem(withTitle: title)
                self.agentSource.lastItem?.representedObject = key
                if key == request.selection { self.agentSource.select(self.agentSource.lastItem) }
            }
            if request.selection.hasPrefix("opencode:") {
                self.agentSource.addItem(withTitle: "OpenCode · " + result.snapshot.title)
                self.agentSource.lastItem?.representedObject = request.selection
                self.agentSource.select(self.agentSource.lastItem)
            }
            self.workController.update(result.snapshot)
        }
        workLoader.request(request)
    }

    // MARK: - Parsing

    static func parseCards(text: String) -> [RunCard] {
        let logFormatter = makeLogFormatter()
        struct Event {
            var stamp: String
            var date: Date
            var kind: String
            var title: String
            var ok: Bool?
            var detail: [String]
            var detailSize: Int
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
                current = Event(stamp: stamp, date: date, kind: kind, title: title, ok: ok, detail: [message], detailSize: message.count)
            } else if current != nil {
                if current!.detailSize < 20000 {
                    current!.detail.append(line)
                    current!.detailSize += line.count
                } else if current!.detail.last != "… output truncated …" {
                    current!.detail.append("… output truncated …")
                }
            }
        }
        flush()

        var cards: [RunCard] = []
        var pendingStart: Event?
        var pendingEnd: Event?
        func finishPending(at boundary: Date? = nil) {
            guard let start = pendingStart else { return }
            var lines = start.detail + (pendingEnd?.detail ?? [])
            var sessionID = ""
            for line in lines where line.hasPrefix("session: ") {
                sessionID = String(line.dropFirst("session: ".count)).trimmingCharacters(in: .whitespaces)
            }
            lines.removeAll { $0 == "starting run" || $0.hasPrefix("run rc=") || $0.hasPrefix("session: ") }
            var detail = lines.joined(separator: "\n")
            if detail.isEmpty { detail = "No captured output for this run." }
            var subtitle = "in progress"
            var status: Bool?
            if let end = pendingEnd {
                let seconds = Int(end.date.timeIntervalSince(start.date))
                subtitle = "finished in \(seconds >= 60 ? "\(seconds / 60)m \(seconds % 60)s" : "\(seconds)s")"
                status = end.ok
            } else if boundary != nil {
                subtitle = "interrupted"
                status = false
            }
            cards.append(RunCard(key: start.stamp + "|run", title: "Agent run", subtitle: subtitle,
                                 date: start.date, status: status, detail: detail, sessionID: sessionID, finishedAt: pendingEnd?.date ?? boundary))
            pendingStart = nil
            pendingEnd = nil
        }
        for event in events {
            switch event.kind {
            case "runStart":
                finishPending(at: event.date)
                pendingStart = event
            case "runEnd":
                if pendingStart != nil { pendingEnd = event; finishPending() }
            case "loopStart", "loopStop":
                finishPending(at: event.date)
                cards.append(RunCard(key: event.stamp + "|" + event.kind, title: event.title, subtitle: "",
                                     date: event.date, status: nil, detail: "", sessionID: ""))
            default:
                if let id = capture("^session: (ses_[A-Za-z0-9_-]+)", in: event.title) {
                    if pendingStart != nil {
                        pendingStart?.detail.append("session: " + id)
                    } else if let index = cards.lastIndex(where: { $0.title == "Agent run" }) {
                        cards[index].sessionID = id
                    }
                    continue
                }
                cards.append(RunCard(key: event.stamp + "|note\(cards.count)", title: event.title, subtitle: "",
                                     date: event.date, status: nil,
                                     detail: event.detail.joined(separator: "\n"), sessionID: ""))
            }
        }
        finishPending()
        return cards
    }

    // MARK: - Left list

    func makeRow(_ card: RunCard) -> NSView {
        let row = ClickRow()
        row.wantsLayer = true
        row.translatesAutoresizingMaskIntoConstraints = false
        let selected = card.key == selectedKey
        row.layer?.backgroundColor = (selected ? Theme.layer02 : Theme.layer01).themeCGColor
        row.layer?.cornerRadius = 10
        row.layer?.borderWidth = 1
        row.layer?.borderColor = (selected ? Theme.borderStrong : Theme.border).themeCGColor
        let inner = NSStackView()
        inner.orientation = .horizontal
        inner.spacing = 8
        inner.edgeInsets = NSEdgeInsets(top: 8, left: 10, bottom: 8, right: 10)
        inner.translatesAutoresizingMaskIntoConstraints = false
        row.addSubview(inner)
        NSLayoutConstraint.activate([
            inner.topAnchor.constraint(equalTo: row.topAnchor),
            inner.bottomAnchor.constraint(equalTo: row.bottomAnchor),
            inner.leadingAnchor.constraint(equalTo: row.leadingAnchor),
            inner.trailingAnchor.constraint(equalTo: row.trailingAnchor),
        ])
        let icon = NSImageView()
        let symbol: String
        let tint: NSColor
        if let ok = card.status, ok {
            symbol = "checkmark.circle.fill"; tint = Theme.success
        } else if let ok = card.status, !ok {
            symbol = "xmark.circle.fill"; tint = Theme.danger
        } else if card.subtitle == "in progress" {
            symbol = "circle.dotted"; tint = Theme.primary
        } else {
            symbol = "circle.fill"; tint = Theme.accent
        }
        icon.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        icon.contentTintColor = tint
        icon.translatesAutoresizingMaskIntoConstraints = false
        icon.widthAnchor.constraint(equalToConstant: 15).isActive = true
        let text = NSStackView()
        text.orientation = .vertical
        text.distribution = .fill
        text.alignment = .leading
        text.spacing = 1
        text.addArrangedSubview(headerLabel(card.title, size: 12, weight: .semibold))
        if !card.subtitle.isEmpty {
            text.addArrangedSubview(headerLabel(card.subtitle, size: 11, color: Theme.textMuted))
        }
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let time = headerLabel(DateFormatter.localizedString(from: card.date, dateStyle: .none, timeStyle: .short),
                                size: 10, color: Theme.textFaint)
        inner.addArrangedSubview(icon)
        inner.addArrangedSubview(text)
        inner.addArrangedSubview(spacer)
        inner.addArrangedSubview(time)
        row.identifier = NSUserInterfaceItemIdentifier(card.key)
        row.addGestureRecognizer(NSClickGestureRecognizer(target: self, action: #selector(rowClicked(_:))))
        row.onHover = { [weak self, weak row] hovering in
            guard let self, let row else { return }
            let isSelected = row.identifier?.rawValue == self.selectedKey
            row.layer?.backgroundColor = (hovering || isSelected ? Theme.layer02 : Theme.layer01).themeCGColor
        }
        return row
    }

    @objc func rowClicked(_ sender: NSClickGestureRecognizer) {
        guard let key = sender.view?.identifier?.rawValue else { return }
        selectedKey = key
        expandedMessages.removeAll()
        rebuild()
    }

    @objc func showEarlier() {
        visibleCards += 20
        rebuild()
    }

    @objc func showLatest() {
        historyAnchor = nil
        visibleCards = 20
        selectedKey = cards.suffix(20).last(where: { $0.title == "Agent run" })?.key ?? cards.last?.key ?? ""
        rebuild()
    }

    func inspectRun(_ key: String) {
        _ = view
        activityMode.selectedSegment = 1
        workerRoot?.isHidden = false
        workController.view.isHidden = true
        agentSource.isHidden = true
        pendingInspection = key
        rebuild()
    }

    // MARK: - Right sidebar

    @objc func tabChanged() {
        renderRight()
    }

    @objc func filterChanged() {
        filter = filterField.stringValue
        renderOutput()
    }

    func selectedCard() -> RunCard? {
        cards.first { $0.key == selectedKey }
    }

    func renderRight() {
        let outputTab = tabPicker.selectedSegment == 0
        outputContainer.isHidden = !outputTab
        contextContainer.isHidden = outputTab
        filterField.isHidden = !outputTab
        outputEmpty.isHidden = true
        guard let card = selectedCard() else {
            rightTitle.stringValue = "Run details"
            rightStats.stringValue = "Select a run to inspect its output or context"
            outputView.string = activityError ?? "No runs recorded yet. Start your configured worker to see live activity."
            outputView.font = Theme.codeFont(11)
            outputView.textColor = Theme.textMuted
            tabPicker.isEnabled = false
            filterField.isEnabled = false
            contextStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
            let message = NSTextField(wrappingLabelWithString: activityError ?? "No runs recorded yet. Context appears after a run records a session ID.")
            message.font = Theme.uiFont(12)
            message.textColor = Theme.textMuted
            contextStack.addArrangedSubview(message)
            message.widthAnchor.constraint(equalTo: contextStack.widthAnchor, constant: -16).isActive = true
            lastContextSignature = ""
            outputEmpty.set(title: "No activity recorded", message: activityError ?? "Runs appear here when the configured worker writes to its log.")
            outputEmpty.isHidden = !outputTab
            outputContainer.isHidden = true
            return
        }
        tabPicker.isEnabled = true
        filterField.isEnabled = true
        rightTitle.stringValue = card.title
        let stamp = DateFormatter.localizedString(from: card.date, dateStyle: .medium, timeStyle: .medium)
        if outputTab {
            let lineCount = card.detail.split(separator: "\n").count
            rightStats.stringValue = "\(card.subtitle) · \(stamp) · \(lineCount) lines"
            renderOutput()
        } else {
            rightStats.stringValue = "\(stamp)\(card.sessionID.isEmpty ? " · no session id logged" : "")"
            renderContext()
        }
    }

    func renderOutput() {
        guard let card = selectedCard() else { return }
        if card.detail.isEmpty || card.detail == "No captured output for this run." {
            outputContainer.isHidden = true
            outputEmpty.set(title: "No captured output", message: card.sessionID.isEmpty
                ? "This run did not save console output or a session ID. Check the worker’s log settings."
                : "This run has no saved console output. Choose Context to inspect its recorded session.")
            outputEmpty.isHidden = false
            filterField.isEnabled = false
            return
        }
        outputEmpty.isHidden = true
        outputContainer.isHidden = false
        filterField.isEnabled = true
        var text = card.detail
        if !filter.isEmpty {
            text = text.split(separator: "\n", omittingEmptySubsequences: false)
                .filter { $0.lowercased().contains(filter.lowercased()) }
                .joined(separator: "\n")
        }
        if text.isEmpty { text = "No output lines match “\(filter)”." }
        if lastOutput?.key == card.key, lastOutput?.filter == filter, lastOutput?.text == text { return }
        let sameRun = lastOutput?.key == card.key && lastOutput?.filter == filter
        let scroll = outputView.enclosingScrollView
        let origin = scroll?.contentView.bounds.origin ?? .zero
        outputView.textStorage?.setAttributedString(numberedText(text, size: 11))
        lastOutput = (card.key, filter, text)
        if sameRun, let scroll {
            outputView.layoutManager?.ensureLayout(for: outputView.textContainer!)
            scroll.contentView.scroll(to: origin)
            scroll.reflectScrolledClipView(scroll.contentView)
        } else { outputView.scrollToBeginningOfDocument(nil) }
    }

    func renderContext() {
        guard let card = selectedCard() else { return }
        let cacheKey = Prefs.project + "|" + card.sessionID
        let signature = cacheKey + "|" + expandedMessages.sorted().joined(separator: ",") + "|\(sessionCache[cacheKey]?.messages.count ?? -1)"
        if lastContextSignature == signature { return }
        lastContextSignature = signature
        contextStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        guard !card.sessionID.isEmpty else {
            contextStack.addArrangedSubview(headerLabel("This run has no session id — the worker logs session ids from its next restart onward.",
                                                        size: 11, color: Theme.textMuted))
            return
        }
        if let cached = sessionCache[cacheKey] {
            buildContext(card: card, info: cached.info, messages: cached.messages)
            return
        }
        contextStack.addArrangedSubview(headerLabel("Loading session…", size: 11, color: Theme.textMuted))
        if contextLoading { return }
        contextLoading = true
        let id = card.sessionID
        let project = Prefs.project
        DispatchQueue.global().async {
            let result = run("\(home)/.opencode/bin/opencode", ["session", "export", id], cwd: project)
            let data = result.output.data(using: .utf8) ?? Data()
            let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            let info = json?["info"] as? [String: Any] ?? [:]
            let messages = json?["messages"] as? [[String: Any]] ?? []
            DispatchQueue.main.async {
                self.contextLoading = false
                self.lastContextSignature = ""
                guard project == Prefs.project else { return }
                if result.code == 0, json != nil {
                    self.sessionCache[cacheKey] = (info, messages)
                    self.renderContext()
                } else if self.selectedCard()?.sessionID == id {
                    self.contextStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
                    self.contextStack.addArrangedSubview(headerLabel("Could not load this session. Select the run again to retry.", size: 11, color: Theme.danger))
                }
            }
        }
    }

    func buildContext(card: RunCard, info: [String: Any], messages: [[String: Any]]) {
        let title = info["title"] as? String ?? "Session"
        let summary = headerLabel("\(title) · \(messages.count) messages",
                                  size: 11, color: Theme.textMuted)
        summary.lineBreakMode = .byTruncatingTail
        contextStack.addArrangedSubview(summary)
        summary.widthAnchor.constraint(equalTo: contextStack.widthAnchor, constant: -16).isActive = true
        if messages.isEmpty {
            contextStack.addArrangedSubview(headerLabel("No messages in this session yet.", size: 12, color: Theme.textMuted))
        }
        for message in messages.suffix(80) {
            let id = message["id"] as? String ?? ""
            let created = (message["time"] as? [String: Any])?["created"] as? Double ?? 0
            let when = created > 0 ? DateFormatter.localizedString(from: Date(timeIntervalSince1970: created / 1000), dateStyle: .short, timeStyle: .medium) : ""
            let open = expandedMessages.contains(id)
            let row = NSStackView()
            row.orientation = .vertical
            row.distribution = .fill
            row.alignment = .leading
            row.spacing = 6
            let header = NSStackView()
            header.orientation = .horizontal
            header.spacing = 8
            header.addArrangedSubview(headerLabel((message["type"] as? String ?? "message").capitalized, size: 11, weight: .semibold))
            header.toolTip = id
            let spacer = NSView()
            spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
            header.addArrangedSubview(spacer)
            header.addArrangedSubview(headerLabel(when, size: 10, color: Theme.textFaint))
            let toggle = NSButton(title: open ? "Hide" : "Show", target: self, action: #selector(toggleMessage(_:)))
            toggle.isBordered = false
            toggle.font = .systemFont(ofSize: 10, weight: .medium)
            toggle.contentTintColor = Theme.primary
            toggle.identifier = NSUserInterfaceItemIdentifier(id)
            header.addArrangedSubview(toggle)
            row.addArrangedSubview(header)
            header.widthAnchor.constraint(equalTo: row.widthAnchor).isActive = true
            if open {
                let jsonText: String
                if let data = try? JSONSerialization.data(withJSONObject: message, options: [.prettyPrinted, .sortedKeys]),
                   let text = String(data: data, encoding: .utf8) {
                    jsonText = String(text.prefix(20000))
                } else {
                    jsonText = "\(message)"
                }
                let view = NSTextView()
                view.isEditable = false
                view.isRichText = true
                view.drawsBackground = true
                view.backgroundColor = Theme.deep
                view.textContainerInset = NSSize(width: 8, height: 8)
                view.frame = NSRect(x: 0, y: 0, width: 600, height: 200)
                view.isVerticallyResizable = true
                view.autoresizingMask = [.width]
                view.textContainer?.widthTracksTextView = true
                view.textStorage?.setAttributedString(numberedText(jsonText, size: 10))
                let scroll = WrappingTextScrollView()
                scroll.translatesAutoresizingMaskIntoConstraints = false
                scroll.hasVerticalScroller = true
                scroll.documentView = view
                scroll.wantsLayer = true
                scroll.layer?.cornerRadius = 3
                scroll.layer?.borderWidth = 1
                scroll.layer?.borderColor = Theme.borderMuted.themeCGColor
                row.addArrangedSubview(scroll)
                scroll.widthAnchor.constraint(equalTo: row.widthAnchor).isActive = true
                scroll.heightAnchor.constraint(equalToConstant: 240).isActive = true
            }
            contextStack.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: contextStack.widthAnchor, constant: -16).isActive = true
        }
    }

    @objc func toggleMessage(_ sender: NSButton) {
        guard let id = sender.identifier?.rawValue else { return }
        if expandedMessages.contains(id) { expandedMessages.remove(id) } else { expandedMessages.insert(id) }
        renderContext()
    }

    // MARK: - Refresh

    func rebuild() {
        var inspectionRow: String?
        if let key = pendingInspection, let index = cards.firstIndex(where: { $0.key == key }) {
            selectedKey = key
            historyAnchor = key
            visibleCards = 20
            tabPicker.selectedSegment = cards[index].sessionID.isEmpty ? 0 : 1
            expandedMessages.removeAll()
            filter = ""
            filterField.stringValue = ""
            pendingInspection = nil
            inspectionRow = key
            lastSignature = ""
        }
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

        if selectedKey.isEmpty || !cards.contains(where: { $0.key == selectedKey }) {
            selectedKey = cards.last(where: { $0.title == "Agent run" })?.key ?? cards.last?.key ?? ""
        }
        let endIndex = historyAnchor.flatMap { key in cards.firstIndex(where: { $0.key == key }).map { min(cards.count, $0 + 11) } } ?? cards.count
        let shownCards = cards.prefix(endIndex).suffix(visibleCards)
        let signature = (activityError ?? "") + cards.map { $0.key + ($0.status.map { $0 ? "+" : "-" } ?? "?") }.joined(separator: "|")
            + "#\(visibleCards)#\(selectedKey)#\(endIndex)"
        if signature == lastSignature {
            renderRight()
            return
        }
        lastSignature = signature

        let origin = listScroll.contentView.bounds.origin
        listStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        if cards.isEmpty {
            listStack.addArrangedSubview(headerLabel("No activity recorded", size: 13, weight: .semibold))
            let message = NSTextField(wrappingLabelWithString: activityError ?? "Runs will appear here as the worker writes its log.")
            message.textColor = Theme.textMuted
            message.font = Theme.uiFont(12)
            listStack.addArrangedSubview(message)
            message.widthAnchor.constraint(equalTo: listStack.widthAnchor, constant: -12).isActive = true
        }
        if endIndex < cards.count {
            let latest = NSButton(title: "Show latest activity", target: self, action: #selector(showLatest))
            latest.bezelStyle = .rounded
            listStack.addArrangedSubview(latest)
        }
        for card in shownCards.reversed() {
            let row = makeRow(card)
            listStack.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: listStack.widthAnchor, constant: -12).isActive = true
        }
        if endIndex > visibleCards {
            let earlier = NSButton(title: "Show earlier runs", target: self, action: #selector(showEarlier))
            earlier.bezelStyle = .rounded
            listStack.addArrangedSubview(earlier)
        }
        listScroll.contentView.scroll(to: origin)
        listScroll.reflectScrolledClipView(listScroll.contentView)
        renderRight()
        if let key = inspectionRow, let row = listStack.arrangedSubviews.first(where: { $0.identifier?.rawValue == key }) {
            listStack.layoutSubtreeIfNeeded()
            row.scrollToVisible(row.bounds)
        }
    }

    override func refresh() {
        refreshWork()
        let path = Prefs.logFile
        if activityRequest != path {
            activityRequest = path
            cards = []
            selectedKey = ""
            historyAnchor = nil
            lastSignature = ""
            lastOutput = nil
            lastContextSignature = ""
            sessionCache.removeAll()
            activityError = "Loading activity…"
            rebuild()
        }
        activityLoader.apply = { [weak self] path, snapshot in
            guard let self, path == Prefs.logFile else { return }
            self.cards = snapshot.cards
            self.activityError = snapshot.error
            self.rebuild()
        }
        activityLoader.request(path)
    }
}

final class SettingsPane: Pane {
    let projectField = NSTextField()
    let workerField = NSTextField()
    let stateField = NSTextField()
    let projectSource = headerLabel("", size: 10, color: Theme.textFaint)
    let workerSource = headerLabel("", size: 10, color: Theme.textFaint)
    let stateSource = headerLabel("", size: 10, color: Theme.textFaint)
    let infoLabel = headerLabel("", size: 11, color: Theme.textMuted)
    let autoStartBox = NSButton(checkboxWithTitle: "Start work automatically when AgentWatcher opens",
                                target: nil, action: nil)

    override func loadView() {
        let root = NSView()
        let stack = makePage(in: root, scrolling: true)
        let pageHeader = makeHeader("Settings", "Project and worker configuration")
        stack.addArrangedSubview(pageHeader)
        pageHeader.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        let appearanceGroup = NSStackView()
        appearanceGroup.orientation = .vertical
        appearanceGroup.alignment = .leading
        appearanceGroup.spacing = 6
        appearanceGroup.addArrangedSubview(headerLabel("Appearance", size: 12, weight: .semibold))
        appearanceGroup.addArrangedSubview(AppearancePicker())
        appearanceGroup.addArrangedSubview(headerLabel("System follows the appearance selected in macOS.", size: 11, color: Theme.textMuted))
        stack.addArrangedSubview(appearanceGroup)
        let form = NSStackView()
        form.orientation = .vertical
        form.distribution = .fill
        form.alignment = .leading
        form.spacing = 18
        stack.addArrangedSubview(form)
        let width = form.widthAnchor.constraint(equalToConstant: 760)
        width.priority = .defaultHigh
        NSLayoutConstraint.activate([width, form.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor)])
        let fields: [(String, NSTextField, NSTextField)] = [
            ("Project folder", projectField, projectSource),
            ("Worker script", workerField, workerSource),
            ("State folder", stateField, stateSource),
        ]
        for (index, item) in fields.enumerated() {
            let (title, field, source) = item
            let group = NSStackView()
            group.orientation = .vertical
            group.distribution = .fill
            group.alignment = .leading
            group.spacing = 6
            group.addArrangedSubview(headerLabel(title, size: 12, weight: .semibold))
            let row = NSStackView()
            row.orientation = .horizontal
            row.spacing = 8
            field.font = Theme.codeFont(12)
            field.bezelStyle = .roundedBezel
            field.isBezeled = true
            field.drawsBackground = true
            field.backgroundColor = Theme.layer01
            field.target = self
            field.action = #selector(fieldChanged)
            field.toolTip = "Press Return to apply this path"
            field.setContentHuggingPriority(.defaultLow, for: .horizontal)
            field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            row.addArrangedSubview(field)
            let choose = NSButton(title: "Choose…", target: self, action: #selector(choosePath(_:)))
            choose.tag = index
            choose.bezelStyle = .rounded
            row.addArrangedSubview(choose)
            group.addArrangedSubview(row)
            group.addArrangedSubview(source)
            form.addArrangedSubview(group)
            group.widthAnchor.constraint(equalTo: form.widthAnchor).isActive = true
            row.widthAnchor.constraint(equalTo: group.widthAnchor).isActive = true
            field.heightAnchor.constraint(greaterThanOrEqualToConstant: 28).isActive = true
        }
        autoStartBox.target = self
        autoStartBox.action = #selector(autoStartChanged)
        form.addArrangedSubview(autoStartBox)
        let redetect = NSButton(title: "Re-detect paths", target: self, action: #selector(redetect))
        redetect.bezelStyle = .rounded
        redetect.toolTip = "Forget overrides and detect the project, worker and state again"
        form.addArrangedSubview(redetect)
        infoLabel.lineBreakMode = .byTruncatingMiddle
        form.addArrangedSubview(infoLabel)
        infoLabel.widthAnchor.constraint(equalTo: form.widthAnchor).isActive = true
        addBanner(stack, key: "settings", text: "Choose the project, worker script and activity folder. Paths apply when you press Return. Re-detect paths restores automatic discovery.")
        view = root
    }

    override func refresh() {
        if projectField.currentEditor() == nil { projectField.stringValue = Prefs.project }
        if workerField.currentEditor() == nil { workerField.stringValue = Prefs.worker }
        if stateField.currentEditor() == nil { stateField.stringValue = Prefs.stateDir }
        projectSource.stringValue = FileManager.default.fileExists(atPath: Prefs.project + "/.git")
            ? "Git repository" : "Choose a Git repository to watch its changes and increments."
        workerSource.stringValue = FileManager.default.isExecutableFile(atPath: Prefs.worker)
            ? "Executable worker script" : "The worker script is unavailable. Choose an executable script."
        let logExists = FileManager.default.fileExists(atPath: Prefs.logFile)
        stateSource.stringValue = logExists ? "Activity log available" : "No loop.log in this folder yet."
        autoStartBox.state = Prefs.autoStart ? .on : .off
        infoLabel.stringValue = "Activity log: \(Prefs.logFile)"
        infoLabel.toolTip = Prefs.logFile
    }

    @objc func choosePath(_ sender: NSButton) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = sender.tag != 1
        panel.canChooseFiles = sender.tag == 1
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        switch sender.tag {
        case 0: Prefs.project = url.path
        case 1: Prefs.worker = url.path
        default: Prefs.stateDir = url.path
        }
        refresh()
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



final class SessionTranscriptController: NSObject, NSWindowDelegate {
    static var windows: [SessionTranscriptController] = []

    let window: NSWindow
    let scroll = NSScrollView()
    let stack = NSStackView()
    let titleLabel = NSTextField(labelWithString: "")
    let statusLabel = headerLabel("Loading transcript…", size: 11, color: Theme.textMuted)
    let sessionID: String
    let project: String
    private var loadInFlight = false
    var expanded = Set<String>()
    var parts: [(key: String, kind: String, title: String, subtitle: String, body: String, tint: NSColor, monospaced: Bool, openByDefault: Bool)] = []

    static func open(sessionID: String, title: String) {
        let controller = SessionTranscriptController(sessionID: sessionID, title: title)
        windows.append(controller)
        controller.window.makeKeyAndOrderFront(nil)
    }

    init(sessionID: String, title: String, loadAutomatically: Bool = true) {
        self.sessionID = sessionID
        self.project = Prefs.project
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1040, height: 780),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        super.init()
        window.title = "Session — \(title)"
        window.isReleasedWhenClosed = false
        AppearanceController.apply(to: window)
        window.minSize = NSSize(width: 700, height: 480)
        window.delegate = self
        window.center()
        buildUI()
        if loadAutomatically { load() }
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
        stack.distribution = .fill
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 6, left: 6, bottom: 14, right: 6)
        stack.translatesAutoresizingMaskIntoConstraints = false
        let document = FlippedView()
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
            document.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            document.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
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
        guard !loadInFlight else { return }
        loadInFlight = true
        statusLabel.stringValue = "Loading transcript…"
        let id = sessionID
        let project = self.project
        DispatchQueue.global().async {
            let result = run("\(home)/.opencode/bin/opencode", ["session", "export", id], cwd: project)
            let data = result.output.data(using: .utf8) ?? Data()
            let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            let messages = json?["messages"] as? [[String: Any]] ?? []
            DispatchQueue.main.async {
                self.loadInFlight = false
                self.build(messages: messages, error: json == nil ? -1 : result.code)
            }
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
        let previousKeys = Set(parts.map { $0.key })
        parts = []
        var counter = 0
        for message in messages {
            let type = message["type"] as? String ?? ""
            if type == "user" {
                let text = message["text"] as? String ?? ""
                parts.append((key: "u\(counter)", kind: "user", title: "You", subtitle: firstLine(text),
                              body: text, tint: Theme.accent, monospaced: false, openByDefault: true))
                counter += 1
            } else {
                for part in message["content"] as? [[String: Any]] ?? [] {
                    let kind = part["type"] as? String ?? ""
                    if kind == "text" {
                        let text = part["text"] as? String ?? ""
                        parts.append((key: "a\(counter)", kind: "text", title: "Agent", subtitle: firstLine(text),
                                      body: text, tint: Theme.primary, monospaced: false, openByDefault: true))
                    } else if kind == "reasoning" {
                        let text = part["text"] as? String ?? ""
                        parts.append((key: "r\(counter)", kind: "reasoning", title: "Thinking", subtitle: firstLine(text),
                                      body: text, tint: Theme.writer, monospaced: false, openByDefault: false))
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
                        if let failure = state["error"] as? String { body += "\n\n── error ──\n" + failure }
                        if body.count > 9000 { body = String(body.prefix(9000)) + "\n… truncated …" }
                        let tint: NSColor = status == "completed" ? Theme.success : (status == "failed" ? Theme.danger : Theme.warning)
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
        let keys = Set(parts.map { $0.key })
        expanded.formIntersection(keys)
        expanded.formUnion(parts.filter { $0.openByDefault && !previousKeys.contains($0.key) }.map { $0.key })
        render()
    }

    func render() {
        let origin = scroll.contentView.bounds.origin
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        if parts.isEmpty {
            stack.addArrangedSubview(headerLabel(statusLabel.stringValue.contains("failed")
                ? "This transcript could not be loaded. Use Reload to try again."
                : "This session has no transcript steps yet.", size: 12, color: Theme.textMuted))
        }
        for part in parts {
            let open = expanded.contains(part.key)
            stack.addArrangedSubview(makeCard(part, open: open))
        }
        for view in stack.arrangedSubviews {
            view.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -12).isActive = true
        }
        window.contentView?.layoutSubtreeIfNeeded()
        scroll.contentView.scroll(to: origin)
        scroll.reflectScrolledClipView(scroll.contentView)
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
        inner.distribution = .fill
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
            let subtitle = headerLabel(part.subtitle, size: 11, color: Theme.textMuted)
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
            toggle.contentTintColor = Theme.textMuted
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
            text.backgroundColor = Theme.deep
            text.textColor = Theme.textBase
            text.font = part.monospaced
                ? Theme.codeFont(11)
                : .systemFont(ofSize: 12)
            text.textContainerInset = NSSize(width: 8, height: 8)
            text.frame = NSRect(x: 0, y: 0, width: 800, height: 160)
            text.isVerticallyResizable = true
            text.autoresizingMask = [.width]
            text.textContainer?.widthTracksTextView = true
            text.string = part.body
            let size = min(360, max(60, part.body.split(separator: "\n").count * 15 + 20))
            let textScroll = WrappingTextScrollView()
            textScroll.translatesAutoresizingMaskIntoConstraints = false
            textScroll.hasVerticalScroller = true
            textScroll.documentView = text
            textScroll.wantsLayer = true
            textScroll.layer?.cornerRadius = 3
            textScroll.layer?.borderWidth = 1
            textScroll.layer?.borderColor = Theme.borderMuted.themeCGColor
            inner.addArrangedSubview(textScroll)
            textScroll.widthAnchor.constraint(equalTo: inner.widthAnchor, constant: -24).isActive = true
            textScroll.heightAnchor.constraint(equalToConstant: CGFloat(size)).isActive = true
        }
        return container
    }
}


final class ChangesPane: Pane {
    private let docked: Bool
    init(docked: Bool = false) { self.docked = docked; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError() }
    struct ChangedFile: Equatable {
        var path: String
        var status: String
        var adds: String
        var dels: String
    }

    struct Snapshot {
        var branch: String
        var files: [ChangedFile]
        var error: String?
    }

    struct DiffRequest: Equatable {
        var project: String
        var file: ChangedFile
    }

    private lazy var snapshotLoader = LatestRequestLoader<String, Snapshot>(
        label: "AgentWatcher.changes", collect: Self.collectSnapshot)
    private lazy var diffLoader = LatestRequestLoader<DiffRequest, (text: String, error: Bool)>(
        label: "AgentWatcher.diff", collect: Self.collectDiff)
    private var loadedProject = ""
    private var applyingFiles = false
    private var displayedDiff: (project: String, path: String, text: String)?
    private var hasSnapshot = false
    private var snapshotError: String?

    let branchLabel = headerLabel("", size: 12, weight: .semibold)
    let statsLabel = headerLabel("", size: 11, color: Theme.textMuted)
    let filterField = NSSearchField()
    let table = NSTableView()
    let diffView = NSTextView()
    let diffHeader = headerLabel("", size: 11, color: Theme.textMuted)
    let prevButton = NSButton(title: "", target: nil, action: nil)
    let nextButton = NSButton(title: "", target: nil, action: nil)
    var files: [ChangedFile] = []
    var allFiles: [ChangedFile] = []
    var filter = ""
    var selectedFile = ""

    override func loadView() {
        let root = NSView()
        let stack = makePage(in: root, scrolling: false, inset: docked ? 12 : 16)
        let pageHeader = makeHeader("Changes", "The agent's working tree, live")
        if !docked {
            stack.addArrangedSubview(pageHeader)
            pageHeader.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        let branchRow = NSStackView()
        branchRow.orientation = .horizontal
        branchRow.spacing = 10
        branchRow.addArrangedSubview(branchLabel)
        branchRow.addArrangedSubview(statsLabel)
        stack.addArrangedSubview(branchRow)
        branchRow.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        branchRow.heightAnchor.constraint(equalToConstant: 22).isActive = true
        branchLabel.lineBreakMode = .byTruncatingMiddle

        let columns = NSView()
        columns.translatesAutoresizingMaskIntoConstraints = false
        columns.setContentHuggingPriority(.defaultLow, for: .vertical)
        let bodyMinimum = columns.heightAnchor.constraint(greaterThanOrEqualToConstant: 240)
        bodyMinimum.priority = .defaultHigh
        bodyMinimum.isActive = true

        filterField.placeholderString = "Filter files"
        filterField.translatesAutoresizingMaskIntoConstraints = false
        filterField.target = self
        filterField.action = #selector(filterChanged)
        filterField.sendsSearchStringImmediately = true
        filterField.delegate = self
        filterField.heightAnchor.constraint(equalToConstant: 24).isActive = true

        for (title, width) in [("File", CGFloat(260)), ("+", CGFloat(50)), ("−", CGFloat(50))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(title))
            column.title = title
            column.width = width
            if title != "File" {
                column.minWidth = width
                column.maxWidth = width
                column.resizingMask = []
            }
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = false
        table.backgroundColor = Theme.base
        table.style = .plain
        table.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
        table.tableColumns.first?.minWidth = 100
        let listScroll = FittingTableScrollView()
        listScroll.translatesAutoresizingMaskIntoConstraints = false
        listScroll.hasVerticalScroller = true
        listScroll.documentView = table
        listScroll.wantsLayer = true
        listScroll.layer?.cornerRadius = 4
        listScroll.layer?.borderWidth = 1
        listScroll.layer?.borderColor = Theme.border.themeCGColor
        let leftColumn = NSStackView()
        leftColumn.orientation = .vertical
        leftColumn.distribution = .fill
        leftColumn.alignment = .leading
        leftColumn.spacing = 8
        leftColumn.addArrangedSubview(filterField)
        leftColumn.addArrangedSubview(listScroll)
        filterField.widthAnchor.constraint(equalTo: leftColumn.widthAnchor).isActive = true
        listScroll.widthAnchor.constraint(equalTo: leftColumn.widthAnchor).isActive = true


        let rightColumn = NSStackView()
        rightColumn.orientation = .vertical
        rightColumn.distribution = .fill
        rightColumn.alignment = .leading
        rightColumn.spacing = 8
        rightColumn.setContentHuggingPriority(.defaultLow, for: .horizontal)
        diffHeader.lineBreakMode = .byTruncatingMiddle
        diffHeader.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let diffRow = NSStackView()
        diffRow.orientation = .horizontal
        diffRow.spacing = 8
        diffRow.addArrangedSubview(diffHeader)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        diffRow.addArrangedSubview(spacer)
        prevButton.image = NSImage(systemSymbolName: "chevron.left", accessibilityDescription: "Previous file")
        prevButton.bezelStyle = .rounded
        prevButton.target = self
        prevButton.action = #selector(prevFile)
        nextButton.image = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: "Next file")
        nextButton.bezelStyle = .rounded
        nextButton.target = self
        nextButton.action = #selector(nextFile)
        prevButton.isEnabled = false
        nextButton.isEnabled = false
        diffRow.addArrangedSubview(prevButton)
        diffRow.addArrangedSubview(nextButton)
        rightColumn.addArrangedSubview(diffRow)
        diffRow.heightAnchor.constraint(equalToConstant: 24).isActive = true

        diffView.isEditable = false
        diffView.isRichText = true
        diffView.drawsBackground = true
        diffView.backgroundColor = Theme.deep
        diffView.textContainerInset = NSSize(width: 10, height: 10)
        diffView.frame = NSRect(x: 0, y: 0, width: 700, height: 400)
        diffView.isVerticallyResizable = true
        diffView.isHorizontallyResizable = false
        diffView.autoresizingMask = [.width]
        diffView.textContainer?.widthTracksTextView = true
        diffView.minSize = NSSize(width: 0, height: 0)
        diffView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        let diffScroll = WrappingTextScrollView()
        diffScroll.translatesAutoresizingMaskIntoConstraints = false
        diffScroll.hasVerticalScroller = true
        diffScroll.documentView = diffView
        diffScroll.wantsLayer = true
        diffScroll.layer?.cornerRadius = 4
        diffScroll.layer?.borderWidth = 1
        diffScroll.layer?.borderColor = Theme.border.themeCGColor
        rightColumn.addArrangedSubview(diffScroll)
        diffScroll.widthAnchor.constraint(equalTo: rightColumn.widthAnchor).isActive = true
        diffRow.widthAnchor.constraint(equalTo: rightColumn.widthAnchor).isActive = true

        if docked {
            for column in [leftColumn, rightColumn] {
                column.translatesAutoresizingMaskIntoConstraints = false
                columns.addSubview(column)
            }
            let listHeight = leftColumn.heightAnchor.constraint(equalToConstant: 164)
            listHeight.priority = .defaultHigh
            NSLayoutConstraint.activate([
                leftColumn.leadingAnchor.constraint(equalTo: columns.leadingAnchor),
                leftColumn.trailingAnchor.constraint(equalTo: columns.trailingAnchor),
                leftColumn.topAnchor.constraint(equalTo: columns.topAnchor), listHeight,
                rightColumn.leadingAnchor.constraint(equalTo: columns.leadingAnchor),
                rightColumn.trailingAnchor.constraint(equalTo: columns.trailingAnchor),
                rightColumn.topAnchor.constraint(equalTo: leftColumn.bottomAnchor, constant: 12),
                rightColumn.bottomAnchor.constraint(equalTo: columns.bottomAnchor),
            ])
        } else {
            pinColumns(in: columns, left: leftColumn, right: rightColumn, leftFraction: 0.38, minimum: 220, maximum: 380)
        }
        stack.addArrangedSubview(columns)
        columns.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        if !docked {
            addBanner(stack, key: "changes", text: "Everything the agent has changed and not yet committed: branch, per-file counts and the live diff. Updates every couple of seconds while the agent works.")
        }
        view = root
    }

    static func collectSnapshot(project: String) -> Snapshot {
        let status = repositoryCommand(project, ["status", "--porcelain=v1", "-z", "--no-renames"])
        guard status.code == 0 else {
            return Snapshot(branch: "", files: [], error: "Could not read changes: \(status.output.trimmingCharacters(in: .whitespacesAndNewlines))")
        }
        let branch = repositoryCommand(project, ["branch", "--show-current"]).output
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let upstream = repositoryCommand(project, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        let upstreamName = upstream.code == 0 ? upstream.output.trimmingCharacters(in: .whitespacesAndNewlines) : ""
        let branchName = branch.isEmpty ? "detached HEAD" : branch
        let branchText = upstreamName.isEmpty ? "Branch: \(branchName)" : "Branch: \(branchName) → \(upstreamName)"
        // Compare the whole working tree with HEAD, including staged edits.
        let numstat = repositoryCommand(project, ["diff", "--no-ext-diff", "--no-textconv", "--numstat", "-z", "--no-renames", "HEAD", "--"])
        let unborn = numstat.code != 0 && repositoryCommand(project, ["rev-parse", "--verify", "HEAD"]).code != 0
        guard numstat.code == 0 || unborn else {
            return Snapshot(branch: branchText, files: [], error: "Could not read change counts: \(numstat.output.trimmingCharacters(in: .whitespacesAndNewlines))")
        }
        var counts: [String: (String, String)] = [:]
        for line in (unborn ? "" : numstat.output).split(separator: "\0") {
            let parts = line.split(separator: "\t", maxSplits: 2).map(String.init)
            if parts.count == 3 { counts[parts[2]] = (parts[0], parts[1]) }
        }
        var parsed: [ChangedFile] = []
        for line in status.output.split(separator: "\0") {
            let entry = String(line)
            guard entry.count > 3 else { continue }
            let statusCode = String(entry.prefix(2)).trimmingCharacters(in: .whitespaces)
            let path = String(entry.dropFirst(3))
            // Before the first commit, every existing tracked file is an
            // addition. Read its current contents, including edits after add.
            if unborn && statusCode != "??" {
                let absolute = project + "/" + path
                if pathExists(absolute) {
                    let added = repositoryCommand(project, ["diff", "--no-ext-diff", "--no-textconv", "--no-index", "--numstat", "-z", "--", "/dev/null", absolute])
                    guard added.code == 0 || added.code == 1 else {
                        return Snapshot(branch: branchText, files: [], error: "Could not read change counts for \(path): \(added.output)")
                    }
                    let parts = added.output.split(separator: "\t", maxSplits: 2).map(String.init)
                    counts[path] = parts.count == 3 ? (parts[0], parts[1]) : ("0", "0")
                } else {
                    counts[path] = ("0", "0")
                }
            }
            let count = counts[path] ?? (statusCode == "??" ? ("new", "new") : ("0", "0"))
            parsed.append(ChangedFile(path: path, status: statusCode.isEmpty ? "M" : statusCode, adds: count.0, dels: count.1))
        }
        return Snapshot(branch: branchText, files: parsed, error: nil)
    }

    static func collectDiff(_ request: DiffRequest) -> (text: String, error: Bool) {
        let project = request.project
        let file = request.file
        let wholeFile = file.status == "??" || repositoryCommand(project, ["rev-parse", "--verify", "HEAD"]).code != 0
        let result: (code: Int32, output: String)
        if wholeFile && !pathExists(project + "/" + file.path) {
            result = (0, "")
        } else if wholeFile {
            result = repositoryCommand(project, ["diff", "--no-ext-diff", "--no-textconv", "--no-index", "--", "/dev/null", project + "/" + file.path])
        } else {
            result = repositoryCommand(project, ["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--", file.path])
        }
        let failed = wholeFile ? result.code != 0 && result.code != 1 : result.code != 0
        var text = failed ? "Could not read diff.\n\(result.output)" : result.output
        if text.isEmpty { text = "No textual diff for \(file.path) (binary, mode change or edits that cancel out)." }
        if text.count > 120000 { text = String(text.prefix(120000)) + "\n… diff truncated …\n" }
        return (text, failed)
    }

    private static func pathExists(_ path: String) -> Bool {
        FileManager.default.fileExists(atPath: path)
            || (try? FileManager.default.destinationOfSymbolicLink(atPath: path)) != nil
    }

    override func refresh() {
        let project = Prefs.project
        if loadedProject != project {
            loadedProject = project
            allFiles = []
            selectedFile = ""
            displayedDiff = nil
            hasSnapshot = false
            snapshotError = nil
            applyFilter()
            branchLabel.stringValue = "Loading changes…"
            statsLabel.stringValue = ""
        }
        snapshotLoader.apply = { [weak self] project, snapshot in
            guard let self, project == Prefs.project else { return }
            if let error = snapshot.error {
                self.snapshotError = error
                if !self.hasSnapshot { self.branchLabel.stringValue = "Changes unavailable" }
                self.statsLabel.stringValue = "Refresh failed — showing last available changes"
                self.statsLabel.textColor = Theme.danger
                self.statsLabel.toolTip = error
                if self.allFiles.isEmpty { self.renderDiff() }
                return
            }
            self.branchLabel.stringValue = snapshot.branch
            self.hasSnapshot = true
            self.snapshotError = nil
            self.allFiles = snapshot.files
            self.statsLabel.textColor = Theme.textMuted
            self.statsLabel.toolTip = nil
            let adds = snapshot.files.compactMap { Int($0.adds) }.reduce(0, +)
            let dels = snapshot.files.compactMap { Int($0.dels) }.reduce(0, +)
            self.statsLabel.stringValue = snapshot.files.isEmpty
                ? "clean — no uncommitted changes"
                : "\(snapshot.files.count) changed files  ·  +\(adds)  ·  −\(dels)"
            self.applyFilter()
        }
        snapshotLoader.request(project)
    }

    func applyFilter() {
        let filtered = filter.isEmpty ? allFiles : allFiles.filter { $0.path.localizedCaseInsensitiveContains(filter) }
        let changed = filtered != files
        files = filtered

        if selectedFile.isEmpty || !files.contains(where: { $0.path == selectedFile }) {
            selectedFile = files.first?.path ?? ""
        }
        applyingFiles = true
        if changed { table.reloadData() }
        if let index = files.firstIndex(where: { $0.path == selectedFile }) {
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        } else {
            table.deselectAll(nil)
        }
        applyingFiles = false
        renderDiff()
    }

    @objc func filterChanged() {
        filter = filterField.stringValue
        applyFilter()
    }

    @objc func prevFile() {
        guard let index = files.firstIndex(where: { $0.path == selectedFile }), index > 0 else { return }
        selectedFile = files[index - 1].path
        applyFilter()
    }

    @objc func nextFile() {
        guard let index = files.firstIndex(where: { $0.path == selectedFile }), index + 1 < files.count else { return }
        selectedFile = files[index + 1].path
        applyFilter()
    }

    func showDiffMessage(_ text: String) {
        displayedDiff = nil
        diffView.string = text
        diffView.font = Theme.codeFont(11)
        diffView.textColor = Theme.textMuted
    }

    func renderDiff() {
        let index = files.firstIndex(where: { $0.path == selectedFile })
        prevButton.isEnabled = (index ?? 0) > 0
        nextButton.isEnabled = index.map { $0 + 1 < files.count } ?? false
        guard let file = files.first(where: { $0.path == selectedFile }) else {
            if allFiles.isEmpty, let error = snapshotError {
                diffHeader.stringValue = "Changes unavailable"
                showDiffMessage(error)
                return
            }
            if !hasSnapshot {
                diffHeader.stringValue = "Loading changes…"
                showDiffMessage("Loading changes…")
                return
            }
            diffHeader.stringValue = allFiles.isEmpty ? "No file selected" : "No matching files"
            showDiffMessage(allFiles.isEmpty
                ? "The working tree is clean — nothing to show."
                : "No files match “\(filter)”. Clear the filter to see all changes.")
            return
        }
        diffHeader.stringValue = "\(file.status)  \(file.path)  +\(file.adds) −\(file.dels)"
        let project = loadedProject
        if displayedDiff?.project != project || displayedDiff?.path != file.path {
            showDiffMessage("Loading diff…")
        }
        diffLoader.apply = { [weak self] request, result in
            guard let self, request.project == Prefs.project,
                  request.file.path == self.selectedFile,
                  self.files.contains(request.file) else { return }
            if self.displayedDiff?.project == request.project,
               self.displayedDiff?.path == request.file.path,
               self.displayedDiff?.text == result.text { return }
            let sameFile = self.displayedDiff?.project == request.project && self.displayedDiff?.path == request.file.path
            let origin = self.diffView.enclosingScrollView?.contentView.bounds.origin ?? .zero
            let selection = self.diffView.selectedRange()
            self.diffView.textStorage?.setAttributedString(numberedText(result.text, size: 11))
            self.displayedDiff = (request.project, request.file.path, result.text)
            if result.error { self.diffHeader.stringValue = "Could not read \(request.file.path) — retrying on refresh" }
            if sameFile, let scroll = self.diffView.enclosingScrollView {
                self.diffView.layoutManager?.ensureLayout(for: self.diffView.textContainer!)
                let length = self.diffView.string.utf16.count
                if NSMaxRange(selection) <= length { self.diffView.setSelectedRange(selection) }
                scroll.contentView.scroll(to: origin)
                scroll.reflectScrolledClipView(scroll.contentView)
            } else {
                self.diffView.scrollToBeginningOfDocument(nil)
            }
        }
        diffLoader.request(DiffRequest(project: project, file: file))
    }
}

extension ChangesPane: NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    func controlTextDidChange(_ notification: Notification) { filterChanged() }
    func numberOfRows(in tableView: NSTableView) -> Int { files.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView()
        let field = NSTextField(labelWithString: "")
        field.font = Theme.codeFont(11)
        field.lineBreakMode = .byTruncatingMiddle
        field.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(field)
        cell.textField = field
        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
            field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
            field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        let file = files[row]
        switch tableColumn?.identifier.rawValue {
        case "+": field.stringValue = file.adds; field.textColor = Theme.success
        case "−": field.stringValue = file.dels; field.textColor = Theme.danger
        default: field.stringValue = "\(file.status)  \(file.path)"
        }
        return cell
    }
    func tableViewSelectionDidChange(_ notification: Notification) {
        let row = table.selectedRow
        guard !applyingFiles, row >= 0, row < files.count else { return }
        if files[row].path != selectedFile {
            selectedFile = files[row].path
            renderDiff()
        }
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
    struct SummaryRequest: Equatable {
        var project: String
        var logFile: String
        var worker: String
        var stateDir: String
        var revision: Int
    }
    struct Summary {
        var todayCount: Int?
        var finishedRuns: Int?
        var status: AgentStatus
        var latestCommit: String
        var updated: Date
    }
    private lazy var summaryLoader = LatestRequestLoader<SummaryRequest, Summary>(label: "AgentWatcher.summary") { request in
        let status = AgentStatus.current(project: request.project, worker: request.worker, stateDir: request.stateDir)
        let commits = repositoryCommand(request.project, ["log", "--since=midnight", "--format=%h %s"])
        let count = commits.code == 0 ? commits.output.split(separator: "\n").count : nil
        let latest = repositoryCommand(request.project, ["log", "-1", "--format=%h %s"])
        let log = try? String(contentsOfFile: request.logFile, encoding: .utf8)
        let finished = log.map { $0.split(separator: "\n").filter { $0.hasPrefix("[") && $0.contains("] ") && $0.contains("run rc=") }.count }
        return Summary(todayCount: count, finishedRuns: finished, status: status,
                       latestCommit: latest.code == 0 ? String(latest.output.split(separator: "\n").first ?? "") : "",
                       updated: Date())
    }
    private var summaryRequest: SummaryRequest?
    private var summary: Summary?
    private var summaryRevision = 0
    private var autoStartPending = false
    var window: NSWindow!
    var sidebar: NSTableView!
    var content = NSView()
    let navigationHost = NSView()
    let projectButton = NSButton()
    let workspaceStatus = headerLabel("Checking agent…", size: 11, color: Theme.textMuted)
    let workspaceFreshness = headerLabel("", size: 11, color: Theme.textMuted)
    let workspaceProject = NSButton()
    let sidebarHeading = headerLabel("WORKSPACE", size: 10, weight: .medium, color: Theme.textMuted)
    let latestIncrement = NSButton()
    let latestHeading = headerLabel("LATEST INCREMENT", size: 10, weight: .medium, color: Theme.textMuted)
    let sidebarDivider = WorkspaceDivider()
    let inspectorDivider = WorkspaceDivider()
    let inspectorBody = NSView()
    let changesTab = NSButton()
    let detailsTab = NSButton()
    let changesDock = ChangesPane(docked: true)
    private var palette: WorkspaceCommandPaletteController?
    private var sidebarPreferredWidth: CGFloat = 228
    private var inspectorPreferredWidth: CGFloat = 400
    private var sidebarCompact = false
    private var applyingNavigation = false
    private var inspectorMode = "changes"
    private var backItem: NSToolbarItem?
    private var forwardItem: NSToolbarItem?
    let inspectorHost = NSView()
    private var inspectorWidth: NSLayoutConstraint!
    private var contentTrailing: NSLayoutConstraint!
    private var contentLeading: NSLayoutConstraint!
    private var inspectorController: NSViewController?
    private var inspectorVisible = true
    var panes: [Pane] = []
    var current = 0
    var timer: Timer?
    var sidebarScroll: NSScrollView!
    var sidebarWidth: NSLayoutConstraint!
    var sidebarFooter: NSStackView!
    var sidebarSeparator: NSBox!
    var statusItem: NSStatusItem!
    var navHistory: [Int] = []
    var navIndex = -1
    var titleItem: NSToolbarItem?
    var badgeItem: NSToolbarItem?
    var lastFinishedRuns = -1
    // The review bundle reads real local records but never starts a worker on launch.
    var isUIReview: Bool { Bundle.main.object(forInfoDictionaryKey: "AgentWatcherUIReview") as? Bool == true }

    func buildToolbar() {
        let toolbar = NSToolbar(identifier: "AgentWatcher.Toolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = false
        window.toolbar = toolbar
        window.toolbarStyle = .unifiedCompact
        window.titleVisibility = .hidden
    }

    func pushHistory(_ index: Int) {
        if navIndex < 0 || navHistory.indices.contains(navIndex) && navHistory[navIndex] != index {
            navHistory = Array(navHistory.prefix(navIndex + 1))
            navHistory.append(index)
            navIndex = navHistory.count - 1
        }
    }

    @objc func navBack() {
        guard navIndex > 0 else { return }
        navIndex -= 1
        let target = navHistory[navIndex]
        displaySection(target)
    }

    @objc func navForward() {
        guard navIndex + 1 < navHistory.count else { return }
        navIndex += 1
        let target = navHistory[navIndex]
        displaySection(target)
    }

    @objc func toggleSidebar() {
        Prefs.sidebarVisible.toggle()
        updateInspectorLayout()
    }

    func showInspector(_ controller: NSViewController) {
        inspectorController = controller
        controller.preferredContentSize = .zero
        inspectorMode = "details"
        mountInspector()
        inspectorVisible = true
        updateInspectorLayout()
    }

    private func mountInspector() {
        inspectorBody.subviews.forEach { $0.removeFromSuperview() }
        let controller: NSViewController = inspectorMode == "details" ? (inspectorController ?? changesDock) : changesDock
        let detail = controller.view
        detail.translatesAutoresizingMaskIntoConstraints = false
        inspectorBody.addSubview(detail)
        NSLayoutConstraint.activate([
            detail.leadingAnchor.constraint(equalTo: inspectorBody.leadingAnchor),
            detail.trailingAnchor.constraint(equalTo: inspectorBody.trailingAnchor),
            detail.topAnchor.constraint(equalTo: inspectorBody.topAnchor),
            detail.bottomAnchor.constraint(equalTo: inspectorBody.bottomAnchor),
        ])
        changesTab.contentTintColor = inspectorMode == "changes" ? Theme.textBase : Theme.textMuted
        detailsTab.contentTintColor = inspectorMode == "details" ? Theme.textBase : Theme.textMuted
        changesTab.state = inspectorMode == "changes" ? .on : .off
        detailsTab.state = inspectorMode == "details" ? .on : .off
        detailsTab.isEnabled = inspectorController != nil
        if inspectorMode == "changes" { changesDock.refresh() }
    }

    @objc func showChangesDock() { inspectorMode = "changes"; mountInspector() }
    @objc func showDetailsDock() {
        guard inspectorController != nil else { return }
        inspectorMode = "details"; mountInspector()
    }

    @objc func toggleInspector() {
        inspectorVisible.toggle()
        Prefs.defaults.set(inspectorVisible, forKey: "workspace.inspectorVisible")
        updateInspectorLayout()
    }

    private func updateInspectorLayout() {
        guard inspectorWidth != nil, let root = window?.contentView else { return }
        if inspectorHost.isHidden == inspectorVisible { inspectorHost.isHidden = !inspectorVisible }
        let sizing = WorkspaceSizing(windowWidth: root.bounds.width, sidebarVisible: Prefs.sidebarVisible,
                                     sidebarWidth: sidebarPreferredWidth, inspectorVisible: inspectorVisible,
                                     inspectorWidth: inspectorPreferredWidth)
        let compact = sizing.compactNavigation
        if compact != sidebarCompact {
            sidebarCompact = compact
            let wasApplying = applyingNavigation
            applyingNavigation = true
            sidebar.reloadData()
            sidebar.selectRowIndexes(IndexSet(integer: current), byExtendingSelection: false)
            applyingNavigation = wasApplying
        }
        navigationHost.isHidden = !Prefs.sidebarVisible
        sidebarDivider.isHidden = !Prefs.sidebarVisible
        let navWidth = sizing.contentInset
        sidebarWidth.constant = sizing.navigationWidth
        contentLeading.constant = navWidth
        projectButton.isHidden = compact
        sidebarHeading.isHidden = compact
        latestIncrement.isHidden = compact
        latestHeading.isHidden = compact
        sidebarFooter.arrangedSubviews.last?.isHidden = compact
        let width = sizing.inspectorWidth
        let targetWidth = width
        if abs(inspectorWidth.constant - targetWidth) > 0.5 { inspectorWidth.constant = targetWidth }
        inspectorDivider.isHidden = !inspectorVisible
        let trailing: CGFloat = inspectorVisible ? -width : 0
        if abs(contentTrailing.constant - trailing) > 0.5 { contentTrailing.constant = trailing }
    }

    func selectSection(_ index: Int) {
        guard index >= 0 && index < sections.count else { return }
        pushHistory(index)
        displaySection(index)
    }

    private func displaySection(_ index: Int) {
        applyingNavigation = true
        sidebar.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        showPane(index)
        applyingNavigation = false
        backItem?.isEnabled = navIndex > 0
        forwardItem?.isEnabled = navIndex + 1 < navHistory.count
        Prefs.defaults.set(index, forKey: "workspace.section")
    }

    @objc func menuSelect(_ sender: NSMenuItem) {
        selectSection(sender.tag)
    }

    @objc func refreshAllNow() {
        refreshCurrent()
    }

    let sections: [Section] = [
        Section(title: "Overview", symbol: "house", tint: Theme.primary) { OverviewPane() },
        Section(title: "Live Activity", symbol: "waveform.path.ecg", tint: Theme.accent) { ActivityPane() },
        Section(title: "Processes", symbol: "clock.arrow.circlepath", tint: Theme.plan) { ProcessesPane() },
        Section(title: "Increments", symbol: "square.grid.2x2", tint: Theme.success) { IncrementsPane() },
        Section(title: "Changes", symbol: "arrow.triangle.branch", tint: Theme.primary) { ChangesPane() },
        Section(title: "Sessions", symbol: "clock", tint: Theme.writer) { SessionsPane() },
        Section(title: "Settings", symbol: "gearshape", tint: Theme.textMuted) { SettingsPane() },
    ]

    func buildStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = statusItem.button {
            button.image = NSImage(systemSymbolName: "eye.circle", accessibilityDescription: "AgentWatcher")
            button.image?.isTemplate = true
            button.toolTip = "AgentWatcher"
        }
        let menu = NSMenu()
        menu.delegate = self
        statusItem.menu = menu
    }

    @objc func showAbout() {
        NSApp.orderFrontStandardAboutPanel(options: [
            .applicationName: "AgentWatcher",
            .applicationVersion: "0.1.0",
            .version: "1",
            .credits: NSAttributedString(string: "A watcher for continuous background coding agents.\nDesign language and theme from OpenCode (MIT)."),
        ])
    }

    @objc func startWorkFromMenu() {
        guard let overview = panes.first as? OverviewPane, !overview.operationInFlight,
              let status = actionableStatus, status.known, !status.running, !Prefs.worker.isEmpty else { return }
        overview.beginOperation("Starting…")
        startWorker()
        overview.refresh()
        refreshAllNow()
    }

    @objc func pauseWorkFromMenu() {
        guard let overview = panes.first as? OverviewPane, !overview.operationInFlight,
              actionableStatus?.running == true else { return }
        overview.beginOperation("Pausing…")
        bash("mkdir -p \"\(Prefs.stateDir)\"; touch \"\(Prefs.stopFile)\"")
        overview.refresh()
        refreshAllNow()
    }

    private var actionableStatus: AgentStatus? {
        guard let summary, let request = summaryRequest,
              request.project == Prefs.project, request.worker == Prefs.worker,
              request.stateDir == Prefs.stateDir, request.revision == summaryRevision,
              Date().timeIntervalSince(summary.updated) < 10 else { return nil }
        return summary.status
    }

    @objc func openAppFromMenu() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        AppearanceController.start()
        buildMenu()
        panes = sections.map { $0.make() }
        (panes.first as? OverviewPane)?.onOperation = { [weak self] in
            guard let self else { return }
            self.summaryRevision += 1
            self.summary = nil
            self.summaryRequest = nil
            self.autoStartPending = false
        }
        buildWindow()
        buildStatusItem()
        autoStartPending = !isUIReview && Prefs.autoStart && !Prefs.worker.isEmpty
        if !isUIReview && Prefs.worker.isEmpty {
            DispatchQueue.main.async {
                let alert = NSAlert()
                alert.messageText = "Welcome to AgentWatcher"
                alert.informativeText = "No agent worker script was detected. Open Settings to choose one, or place an agent loop script in ~/.local/bin and it will be found automatically."
                alert.addButton(withTitle: "Open Settings")
                alert.addButton(withTitle: "Later")
                if alert.runModal() == .alertFirstButtonReturn {
                    self.selectSection(self.sections.count - 1)
                }
            }
        }
        refreshCurrent()
        let timer = Timer(timeInterval: 2.5, repeats: true) { [weak self] _ in self?.refreshCurrent() }
        RunLoop.main.add(timer, forMode: .common)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            window.makeKeyAndOrderFront(nil)
        }
        NSApp.activate(ignoringOtherApps: true)
        return true
    }

    func buildMenu() {
        let mainMenu = NSMenu()
        let appItem = NSMenuItem()
        mainMenu.addItem(appItem)
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "About AgentWatcher", action: #selector(showAbout), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Hide AgentWatcher", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Quit AgentWatcher", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu

        let fileItem = NSMenuItem()
        let fileMenu = NSMenu(title: "File")
        let openWorkspace = NSMenuItem(title: "Open Workspace…", action: #selector(chooseProject), keyEquivalent: "o")
        openWorkspace.target = self
        fileMenu.addItem(openWorkspace)
        fileItem.submenu = fileMenu
        mainMenu.addItem(fileItem)

        let editItem = NSMenuItem()
        mainMenu.addItem(editItem)
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu

        let viewItem = NSMenuItem()
        mainMenu.addItem(viewItem)
        let viewMenu = NSMenu(title: "View")
        let paletteItem = NSMenuItem(title: "Command Palette…", action: #selector(showCommandPalette), keyEquivalent: "p")
        paletteItem.keyEquivalentModifierMask = [.command, .shift]
        paletteItem.target = self
        viewMenu.addItem(paletteItem)
        viewMenu.addItem(.separator())
        for (index, section) in sections.enumerated() {
            let item = NSMenuItem(title: section.title, action: #selector(menuSelect(_:)), keyEquivalent: String(index + 1))
            item.keyEquivalentModifierMask = [.command]
            item.tag = index
            item.target = self
            viewMenu.addItem(item)
        }
        viewMenu.addItem(.separator())
        let refreshItem = NSMenuItem(title: "Refresh", action: #selector(refreshAllNow), keyEquivalent: "r")
        refreshItem.target = self
        viewMenu.addItem(refreshItem)
        let panelItem = NSMenuItem(title: "Toggle Sidebar", action: #selector(toggleSidebar), keyEquivalent: "b")
        panelItem.target = self
        viewMenu.addItem(panelItem)
        let inspectorItem = NSMenuItem(title: "Toggle Inspector", action: #selector(toggleInspector), keyEquivalent: "i")
        inspectorItem.keyEquivalentModifierMask = [.command, .option]
        inspectorItem.target = self
        viewMenu.addItem(inspectorItem)
        viewMenu.addItem(.separator())
        for (title, action, key) in [("Back", #selector(navBack), "["), ("Forward", #selector(navForward), "]")] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
            item.target = self
            viewMenu.addItem(item)
        }
        viewItem.submenu = viewMenu
        NSApp.mainMenu = mainMenu
    }

    func buildWindow() {
        sidebarPreferredWidth = CGFloat(Prefs.defaults.double(forKey: "workspace.sidebarWidth"))
        if !(190...320).contains(sidebarPreferredWidth) { sidebarPreferredWidth = 228 }
        inspectorPreferredWidth = CGFloat(Prefs.defaults.double(forKey: "workspace.inspectorWidth"))
        if !(300...620).contains(inspectorPreferredWidth) { inspectorPreferredWidth = 400 }
        inspectorVisible = Prefs.defaults.object(forKey: "workspace.inspectorVisible") as? Bool ?? true
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1440, height: 860),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.title = "AgentWatcher"
        window.backgroundColor = Theme.base
        AppearanceController.apply(to: window)
        window.minSize = NSSize(width: 900, height: 640)
        window.center()
        let root = InspectorRootView()
        root.onLayout = { [weak self] in self?.updateInspectorLayout() }
        root.wantsLayer = true
        root.layer?.backgroundColor = Theme.base.themeCGColor
        navigationHost.translatesAutoresizingMaskIntoConstraints = false
        navigationHost.wantsLayer = true
        navigationHost.layer?.backgroundColor = Theme.layer01.themeCGColor
        root.addSubview(navigationHost)
        sidebarWidth = navigationHost.widthAnchor.constraint(equalToConstant: sidebarPreferredWidth)

        projectButton.title = (Prefs.project as NSString).lastPathComponent
        projectButton.image = NSImage(systemSymbolName: "folder", accessibilityDescription: nil)
        projectButton.imagePosition = .imageLeft
        projectButton.alignment = .left
        projectButton.isBordered = false
        projectButton.font = Theme.uiFont(13, weight: .medium)
        projectButton.contentTintColor = Theme.textBase
        projectButton.target = self
        projectButton.action = #selector(chooseProject)
        projectButton.toolTip = "Open workspace… (⌘O)\n" + Prefs.project
        projectButton.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        projectButton.translatesAutoresizingMaskIntoConstraints = false
        sidebarHeading.translatesAutoresizingMaskIntoConstraints = false
        navigationHost.addSubview(projectButton)
        navigationHost.addSubview(sidebarHeading)

        sidebar = NSTableView()
        sidebar.style = .plain
        sidebar.backgroundColor = .clear
        sidebar.intercellSpacing = .zero
        sidebar.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        sidebar.headerView = nil
        sidebar.rowHeight = 32
        sidebar.selectionHighlightStyle = .regular
        sidebar.allowsEmptySelection = false
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("main"))
        column.width = sidebarPreferredWidth
        column.minWidth = 0
        sidebar.addTableColumn(column)
        sidebar.dataSource = self
        sidebar.delegate = self
        sidebar.setAccessibilityLabel("Workspace navigation")
        sidebarScroll = NSScrollView()
        sidebarScroll.translatesAutoresizingMaskIntoConstraints = false
        sidebarScroll.documentView = sidebar
        sidebarScroll.drawsBackground = false
        sidebarScroll.hasVerticalScroller = true
        sidebarScroll.autohidesScrollers = true
        navigationHost.addSubview(sidebarScroll)

        latestIncrement.isBordered = false
        latestIncrement.alignment = .left
        latestIncrement.font = Theme.uiFont(11)
        latestIncrement.contentTintColor = Theme.textMuted
        latestIncrement.target = self
        latestIncrement.action = #selector(openLatestIncrement)
        latestIncrement.title = "No increments recorded"
        latestIncrement.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        latestIncrement.translatesAutoresizingMaskIntoConstraints = false
        navigationHost.addSubview(latestIncrement)
        latestHeading.translatesAutoresizingMaskIntoConstraints = false
        navigationHost.addSubview(latestHeading)
        sidebarFooter = NSStackView()
        sidebarFooter.orientation = .horizontal
        sidebarFooter.spacing = 8
        sidebarFooter.translatesAutoresizingMaskIntoConstraints = false
        let about = chromeButton("eye", title: "AgentWatcher", action: #selector(showAbout))
        let command = chromeButton("command", title: "Command Palette", action: #selector(showCommandPalette))
        sidebarFooter.addArrangedSubview(about)
        sidebarFooter.addArrangedSubview(NSView())
        sidebarFooter.arrangedSubviews[1].setContentHuggingPriority(.defaultLow, for: .horizontal)
        sidebarFooter.addArrangedSubview(command)
        navigationHost.addSubview(sidebarFooter)
        sidebarSeparator = NSBox()
        sidebarSeparator.boxType = .separator
        sidebarSeparator.translatesAutoresizingMaskIntoConstraints = false
        navigationHost.addSubview(sidebarSeparator)

        content.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(content)
        inspectorHost.translatesAutoresizingMaskIntoConstraints = false
        inspectorHost.wantsLayer = true
        inspectorHost.layer?.backgroundColor = Theme.base.themeCGColor
        root.addSubview(inspectorHost)
        inspectorWidth = inspectorHost.widthAnchor.constraint(equalToConstant: 0)
        contentTrailing = content.trailingAnchor.constraint(equalTo: root.trailingAnchor)
        contentLeading = content.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: sidebarPreferredWidth)
        inspectorBody.translatesAutoresizingMaskIntoConstraints = false
        inspectorHost.addSubview(inspectorBody)
        let inspectorHeader = NSStackView()
        inspectorHeader.orientation = .horizontal
        inspectorHeader.spacing = 16
        inspectorHeader.edgeInsets = NSEdgeInsets(top: 0, left: 12, bottom: 0, right: 8)
        inspectorHeader.translatesAutoresizingMaskIntoConstraints = false
        for (button, title, action) in [(changesTab, "Changes", #selector(showChangesDock)),
                                        (detailsTab, "Details", #selector(showDetailsDock))] {
            button.title = title
            button.isBordered = false
            button.font = Theme.uiFont(12, weight: .medium)
            button.target = self
            button.action = action
            button.setButtonType(.toggle)
            inspectorHeader.addArrangedSubview(button)
        }
        inspectorHeader.addArrangedSubview(NSView())
        inspectorHeader.arrangedSubviews.last?.setContentHuggingPriority(.defaultLow, for: .horizontal)
        inspectorHeader.addArrangedSubview(chromeButton("xmark", title: "Close right dock (⌥⌘I)", action: #selector(toggleInspector)))
        inspectorHost.addSubview(inspectorHeader)
        let inspectorRule = NSBox()
        inspectorRule.boxType = .separator
        inspectorRule.translatesAutoresizingMaskIntoConstraints = false
        inspectorHost.addSubview(inspectorRule)

        let statusBar = NSStackView()
        statusBar.orientation = .horizontal
        statusBar.spacing = 12
        statusBar.edgeInsets = NSEdgeInsets(top: 0, left: 12, bottom: 0, right: 10)
        statusBar.translatesAutoresizingMaskIntoConstraints = false
        statusBar.wantsLayer = true
        statusBar.layer?.backgroundColor = Theme.layer01.themeCGColor
        workspaceProject.title = (Prefs.project as NSString).lastPathComponent
        workspaceProject.image = NSImage(systemSymbolName: "folder", accessibilityDescription: nil)
        workspaceProject.imagePosition = .imageLeft
        workspaceProject.font = Theme.uiFont(11)
        workspaceProject.isBordered = false
        workspaceProject.contentTintColor = Theme.textMuted
        workspaceProject.target = self
        workspaceProject.action = #selector(revealProject)
        workspaceProject.toolTip = Prefs.project + " · Show in Finder"
        statusBar.addArrangedSubview(workspaceProject)
        statusBar.addArrangedSubview(workspaceStatus)
        statusBar.addArrangedSubview(NSView())
        statusBar.arrangedSubviews.last?.setContentHuggingPriority(.defaultLow, for: .horizontal)
        statusBar.addArrangedSubview(workspaceFreshness)
        statusBar.addArrangedSubview(chromeButton("arrow.clockwise", title: "Refresh (⌘R)", action: #selector(refreshAllNow)))
        root.addSubview(statusBar)
        let statusRule = NSBox()
        statusRule.boxType = .separator
        statusRule.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(statusRule)

        for divider in [sidebarDivider, inspectorDivider] {
            divider.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(divider)
        }
        sidebarDivider.onDrag = { [weak self] delta in
            guard let self, !self.sidebarCompact else { return }
            self.sidebarPreferredWidth = min(320, max(190, self.sidebarPreferredWidth + delta))
            Prefs.defaults.set(Double(self.sidebarPreferredWidth), forKey: "workspace.sidebarWidth")
            self.updateInspectorLayout()
        }
        sidebarDivider.onReset = { [weak self] in
            self?.sidebarPreferredWidth = 228
            Prefs.defaults.set(228, forKey: "workspace.sidebarWidth")
            self?.updateInspectorLayout()
        }
        inspectorDivider.onDrag = { [weak self] delta in
            guard let self else { return }
            self.inspectorPreferredWidth = min(620, max(300, self.inspectorPreferredWidth - delta))
            Prefs.defaults.set(Double(self.inspectorPreferredWidth), forKey: "workspace.inspectorWidth")
            self.updateInspectorLayout()
        }
        inspectorDivider.onReset = { [weak self] in
            self?.inspectorPreferredWidth = 400
            Prefs.defaults.set(400, forKey: "workspace.inspectorWidth")
            self?.updateInspectorLayout()
        }
        NSLayoutConstraint.activate([
            navigationHost.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            navigationHost.topAnchor.constraint(equalTo: root.topAnchor),
            navigationHost.bottomAnchor.constraint(equalTo: statusBar.topAnchor), sidebarWidth,
            projectButton.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor, constant: 12),
            projectButton.trailingAnchor.constraint(equalTo: navigationHost.trailingAnchor, constant: -12),
            projectButton.topAnchor.constraint(equalTo: navigationHost.topAnchor, constant: 10),
            projectButton.heightAnchor.constraint(equalToConstant: 28),
            sidebarHeading.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor, constant: 14),
            sidebarHeading.topAnchor.constraint(equalTo: projectButton.bottomAnchor, constant: 14),
            sidebarScroll.topAnchor.constraint(equalTo: sidebarHeading.bottomAnchor, constant: 8),
            sidebarScroll.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor),
            sidebarScroll.trailingAnchor.constraint(equalTo: navigationHost.trailingAnchor),
            sidebarScroll.bottomAnchor.constraint(equalTo: latestHeading.topAnchor, constant: -20),
            latestHeading.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor, constant: 14),
            latestHeading.bottomAnchor.constraint(equalTo: latestIncrement.topAnchor, constant: -4),
            latestIncrement.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor, constant: 12),
            latestIncrement.trailingAnchor.constraint(equalTo: navigationHost.trailingAnchor, constant: -12),
            latestIncrement.bottomAnchor.constraint(equalTo: sidebarSeparator.topAnchor, constant: -12),
            latestIncrement.heightAnchor.constraint(equalToConstant: 26),
            sidebarSeparator.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor),
            sidebarSeparator.trailingAnchor.constraint(equalTo: navigationHost.trailingAnchor),
            sidebarSeparator.bottomAnchor.constraint(equalTo: sidebarFooter.topAnchor, constant: -6),
            sidebarFooter.leadingAnchor.constraint(equalTo: navigationHost.leadingAnchor, constant: 12),
            sidebarFooter.trailingAnchor.constraint(equalTo: navigationHost.trailingAnchor, constant: -8),
            sidebarFooter.bottomAnchor.constraint(equalTo: navigationHost.bottomAnchor, constant: -6),
            sidebarFooter.heightAnchor.constraint(equalToConstant: 26),
            content.topAnchor.constraint(equalTo: root.topAnchor),
            content.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            contentLeading, contentTrailing,
            inspectorHost.topAnchor.constraint(equalTo: root.topAnchor),
            inspectorHost.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            inspectorHost.trailingAnchor.constraint(equalTo: root.trailingAnchor), inspectorWidth,
            inspectorHeader.topAnchor.constraint(equalTo: inspectorHost.topAnchor),
            inspectorHeader.leadingAnchor.constraint(equalTo: inspectorHost.leadingAnchor),
            inspectorHeader.trailingAnchor.constraint(equalTo: inspectorHost.trailingAnchor),
            inspectorHeader.heightAnchor.constraint(equalToConstant: 36),
            inspectorRule.topAnchor.constraint(equalTo: inspectorHeader.bottomAnchor),
            inspectorRule.leadingAnchor.constraint(equalTo: inspectorHost.leadingAnchor),
            inspectorRule.trailingAnchor.constraint(equalTo: inspectorHost.trailingAnchor),
            inspectorRule.heightAnchor.constraint(equalToConstant: 1),
            inspectorBody.topAnchor.constraint(equalTo: inspectorRule.bottomAnchor),
            inspectorBody.leadingAnchor.constraint(equalTo: inspectorHost.leadingAnchor),
            inspectorBody.trailingAnchor.constraint(equalTo: inspectorHost.trailingAnchor),
            inspectorBody.bottomAnchor.constraint(equalTo: inspectorHost.bottomAnchor),
            statusBar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            statusBar.heightAnchor.constraint(equalToConstant: 28),
            statusRule.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            statusRule.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            statusRule.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            sidebarDivider.centerXAnchor.constraint(equalTo: navigationHost.trailingAnchor),
            sidebarDivider.widthAnchor.constraint(equalToConstant: 5),
            sidebarDivider.topAnchor.constraint(equalTo: root.topAnchor),
            sidebarDivider.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            inspectorDivider.centerXAnchor.constraint(equalTo: inspectorHost.leadingAnchor),
            inspectorDivider.widthAnchor.constraint(equalToConstant: 5),
            inspectorDivider.topAnchor.constraint(equalTo: root.topAnchor),
            inspectorDivider.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
        ])
        window.contentView = root
        if !isUIReview { window.setFrameAutosaveName("AgentWatcher.Workspace") }
        buildToolbar()
        if let screen = window.screen, !screen.visibleFrame.contains(window.frame) {
            let visible = screen.visibleFrame
            let size = NSSize(width: min(1440, visible.width), height: min(900, visible.height))
            window.setFrame(NSRect(x: visible.midX - size.width / 2, y: visible.midY - size.height / 2,
                                   width: size.width, height: size.height), display: false)
        }
        mountInspector()
        updateInspectorLayout()
        let saved = Prefs.defaults.object(forKey: "workspace.section") as? Int ?? 1
        let initial = sections.indices.contains(saved) ? saved : 1
        navHistory = [initial]
        navIndex = 0
        displaySection(initial)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func chromeButton(_ symbol: String, title: String, action: Selector) -> NSButton {
        let button = NSButton(title: "", target: self, action: action)
        button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: title)
        button.isBordered = false
        button.contentTintColor = Theme.textMuted
        button.toolTip = title
        button.setAccessibilityLabel(title)
        button.translatesAutoresizingMaskIntoConstraints = false
        button.widthAnchor.constraint(equalToConstant: 24).isActive = true
        button.heightAnchor.constraint(equalToConstant: 24).isActive = true
        return button
    }

    @objc func chooseProject() {
        let picker = NSOpenPanel()
        picker.title = "Open Workspace"
        picker.prompt = "Watch Workspace"
        picker.canChooseDirectories = true
        picker.canChooseFiles = false
        picker.allowsMultipleSelection = false
        picker.directoryURL = URL(fileURLWithPath: Prefs.project)
        picker.beginSheetModal(for: window) { [weak self] response in
            guard let self, response == .OK, let url = picker.url else { return }
            Prefs.project = url.path
            self.inspectorController = nil
            self.inspectorMode = "changes"
            self.mountInspector()
            self.refreshAllNow()
        }
    }

    @objc func revealProject() { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: Prefs.project)]) }
    @objc func openLatestIncrement() { selectSection(3) }

    @objc func showCommandPalette() {
        palette?.close()
        var commands = sections.enumerated().map { index, section in
            WorkspaceCommand(id: "section-\(index)", title: "Open \(section.title)", subtitle: "Workspace", shortcut: "⌘\(index + 1)", perform: { [weak self] in self?.selectSection(index) })
        }
        commands += [
            WorkspaceCommand(id: "refresh", title: "Refresh workspace", subtitle: "Read the latest local activity", shortcut: "⌘R", perform: { [weak self] in self?.refreshAllNow() }),
            WorkspaceCommand(id: "workspace", title: "Open workspace…", subtitle: "Choose a project to watch", shortcut: "⌘O", perform: { [weak self] in self?.chooseProject() }),
            WorkspaceCommand(id: "navigation", title: "Toggle sidebar", subtitle: "Layout", shortcut: "⌘B", perform: { [weak self] in self?.toggleSidebar() }),
            WorkspaceCommand(id: "dock", title: "Toggle right dock", subtitle: "Changes and details", shortcut: "⌥⌘I", perform: { [weak self] in self?.toggleInspector() }),
            WorkspaceCommand(id: "back", title: "Go back", subtitle: "Navigation", shortcut: "⌘[", perform: { [weak self] in self?.navBack() }),
            WorkspaceCommand(id: "forward", title: "Go forward", subtitle: "Navigation", shortcut: "⌘]", perform: { [weak self] in self?.navForward() }),
        ]
        for preference in [AppearancePreference.system, .light, .dark] {
            commands.append(WorkspaceCommand(id: "theme-\(preference.rawValue)", title: "Appearance: \(preference.rawValue.capitalized)", subtitle: "Theme", shortcut: "", perform: { AppearanceController.preference = preference }))
        }
        palette = WorkspaceCommandPaletteController(commands: commands)
        palette?.show(relativeTo: window)
    }

    func showPane(_ index: Int) {
        if current != index {
            (panes[current] as? OverviewPane)?.closeInspector()
            inspectorController = nil
            inspectorMode = "changes"
            mountInspector()
        }
        current = index
        content.subviews.forEach { $0.removeFromSuperview() }
        (titleItem?.view as? NSTextField)?.stringValue = "\((Prefs.project as NSString).lastPathComponent)  /  \(sections[index].title)"
        let pane = panes[index]
        pane.onInspectDetails = { [weak self] controller in
            guard let self, self.current == index else { return }
            self.showInspector(controller)
        }
        pane.onCloseDetails = { [weak self] in
            guard let self, self.current == index else { return }
            self.inspectorController = nil
            self.inspectorMode = "changes"
            self.mountInspector()
        }
        pane.onNavigate = { [weak self] index in
            guard let self else { return }
            self.selectSection(index)
        }
        pane.onInspectCommit = { [weak self] hash in
            guard let self else { return }
            self.selectSection(3)
            (self.panes[3] as? IncrementsPane)?.inspectCommit(hash)
        }
        pane.onInspectRun = { [weak self] key in
            guard let self else { return }
            self.selectSection(1)
            (self.panes[1] as? ActivityPane)?.inspectRun(key)
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
        let wasApplying = applyingNavigation
        applyingNavigation = true
        sidebar.reloadData()
        sidebar.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        applyingNavigation = wasApplying
    }

    func refreshCurrent() {
        if panes.count > current { panes[current].refresh() }
        if inspectorVisible && inspectorMode == "changes" { changesDock.refresh() }
        let project = (Prefs.project as NSString).lastPathComponent
        projectButton.title = project
        projectButton.toolTip = "Open workspace… (⌘O)\n" + Prefs.project
        workspaceProject.title = project
        workspaceProject.toolTip = Prefs.project + " · Show in Finder"
        (titleItem?.view as? NSTextField)?.stringValue = "\(project)  /  \(sections[current].title)"
        let request = SummaryRequest(project: Prefs.project, logFile: Prefs.logFile,
                                     worker: Prefs.worker, stateDir: Prefs.stateDir, revision: summaryRevision)
        if summaryRequest != request {
            summaryRequest = request
            summary = nil
            workspaceStatus.stringValue = "Checking agent…"
            workspaceStatus.textColor = Theme.textMuted
            workspaceFreshness.stringValue = "Reading local activity…"
            lastFinishedRuns = -1
            (badgeItem?.view as? NSTextField)?.stringValue = "…"
            NSApp.dockTile.badgeLabel = nil
        }
        summaryLoader.apply = { [weak self] request, summary in
            guard let self, request == self.summaryRequest,
                  request.project == Prefs.project, request.logFile == Prefs.logFile,
                  request.worker == Prefs.worker, request.stateDir == Prefs.stateDir,
                  request.revision == self.summaryRevision else { return }
            self.summary = summary
            self.workspaceStatus.stringValue = summary.status.title
            self.workspaceStatus.textColor = !summary.status.known ? Theme.textMuted : summary.status.blocked ? Theme.warning : summary.status.running ? Theme.success : Theme.textMuted
            let stamp = DateFormatter.localizedString(from: summary.updated, dateStyle: .none, timeStyle: .short)
            self.workspaceFreshness.stringValue = "Updated \(stamp)"
            self.latestIncrement.title = summary.latestCommit.isEmpty ? "No increments recorded" : summary.latestCommit
            self.latestIncrement.toolTip = summary.latestCommit
            self.latestIncrement.isEnabled = !summary.latestCommit.isEmpty
            (self.badgeItem?.view as? NSTextField)?.stringValue = summary.todayCount.map(String.init) ?? "—"
            NSApp.dockTile.badgeLabel = summary.todayCount.flatMap { $0 > 0 ? String($0) : nil }
            if let finishedRuns = summary.finishedRuns {
                if self.lastFinishedRuns >= 0 && finishedRuns > self.lastFinishedRuns {
                    NSApp.requestUserAttention(.informationalRequest)
                }
                self.lastFinishedRuns = finishedRuns
            }
            if self.autoStartPending && summary.status.known {
                self.autoStartPending = false
                if Prefs.autoStart && !summary.status.running { self.startWorkFromMenu() }
            }
        }
        summaryLoader.request(request)
    }
}

extension AppDelegate: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { sections.count }
    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { WorkspaceNavigationRowView() }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView()
        let image = NSImageView()
        image.image = NSImage(systemSymbolName: sections[row].symbol, accessibilityDescription: sections[row].title)
        image.contentTintColor = row == current ? Theme.textBase : Theme.textMuted
        image.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(image)
        NSLayoutConstraint.activate([
            image.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: sidebarCompact ? 17 : 14),
            image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            image.widthAnchor.constraint(equalToConstant: 18),
            image.heightAnchor.constraint(equalToConstant: 18),
        ])
        if !sidebarCompact {
            let label = headerLabel(sections[row].title, size: 12, weight: row == current ? .medium : .regular,
                                    color: row == current ? Theme.textBase : Theme.textMuted)
            let shortcut = headerLabel("⌘\(row + 1)", size: 10, color: Theme.textMuted)
            for field in [label, shortcut] { field.translatesAutoresizingMaskIntoConstraints = false; cell.addSubview(field) }
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 10),
                label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                label.trailingAnchor.constraint(lessThanOrEqualTo: shortcut.leadingAnchor, constant: -8),
                shortcut.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -14),
                shortcut.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        cell.toolTip = "\(sections[row].title) (⌘\(row + 1))"
        cell.setAccessibilityLabel(sections[row].title)
        return cell
    }
    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !applyingNavigation else { return }
        let row = sidebar.selectedRow
        if row >= 0 && row != current { selectSection(row) }
    }

}

extension AppDelegate: NSMenuDelegate {
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let status = actionableStatus
        let statusLine = NSMenuItem(title: "Agent: \(status?.title ?? "Checking…")", action: nil, keyEquivalent: "")
        statusLine.isEnabled = false
        menu.addItem(statusLine)
        let latest = summary?.latestCommit ?? ""
        if !latest.isEmpty {
            let latestItem = NSMenuItem(title: "Latest: \(String(latest.prefix(60)))", action: nil, keyEquivalent: "")
            latestItem.isEnabled = false
            menu.addItem(latestItem)
        }
        menu.addItem(.separator())
        let canAct = status?.known == true && (panes.first as? OverviewPane)?.operationInFlight == false
        if status?.running == true {
            let pause = NSMenuItem(title: "Pause After Current Run", action: #selector(pauseWorkFromMenu), keyEquivalent: "")
            pause.target = self
            pause.isEnabled = canAct
            menu.addItem(pause)
        } else {
            let start = NSMenuItem(title: "Start 24/7 Work", action: #selector(startWorkFromMenu), keyEquivalent: "")
            start.target = self
            start.isEnabled = canAct && !Prefs.worker.isEmpty
            menu.addItem(start)
        }
        let open = NSMenuItem(title: "Open AgentWatcher", action: #selector(openAppFromMenu), keyEquivalent: "")
        open.target = self
        menu.addItem(open)
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit AgentWatcher", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
    }
}

extension NSToolbarItem.Identifier {
    static let navBack = NSToolbarItem.Identifier("AgentWatcher.navBack")
    static let navForward = NSToolbarItem.Identifier("AgentWatcher.navForward")
    static let navTitle = NSToolbarItem.Identifier("AgentWatcher.navTitle")
    static let toggleSidebar = NSToolbarItem.Identifier("AgentWatcher.toggleSidebar")
    static let toggleInspector = NSToolbarItem.Identifier("AgentWatcher.toggleInspector")
    static let badge = NSToolbarItem.Identifier("AgentWatcher.badge")
    static let commandPalette = NSToolbarItem.Identifier("AgentWatcher.commandPalette")
    static let refreshWorkspace = NSToolbarItem.Identifier("AgentWatcher.refreshWorkspace")
}

extension AppDelegate: NSToolbarDelegate, NSToolbarItemValidation {
    func validateToolbarItem(_ item: NSToolbarItem) -> Bool {
        if item.itemIdentifier == .navBack { return navIndex > 0 }
        if item.itemIdentifier == .navForward { return navIndex + 1 < navHistory.count }
        return true
    }
    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier itemIdentifier: NSToolbarItem.Identifier,
                 willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        switch itemIdentifier {
        case .navBack:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.image = NSImage(systemSymbolName: "chevron.left", accessibilityDescription: "Back")
            item.target = self
            item.action = #selector(navBack)
            item.label = "Back"
            item.toolTip = "Back (⌘[)"
            item.isEnabled = false
            backItem = item
            return item
        case .navForward:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.image = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: "Forward")
            item.target = self
            item.action = #selector(navForward)
            item.label = "Forward"
            item.toolTip = "Forward (⌘])"
            item.isEnabled = false
            forwardItem = item
            return item
        case .navTitle:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            let label = NSTextField(labelWithString: "Overview")
            label.font = .systemFont(ofSize: 13, weight: .semibold)
            item.view = label
            item.label = "View"
            titleItem = item
            return item
        case .toggleSidebar:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.image = NSImage(systemSymbolName: "sidebar.left", accessibilityDescription: "Toggle sidebar")
            item.target = self
            item.action = #selector(toggleSidebar)
            item.label = "Toggle sidebar"
            item.toolTip = "Show or hide the sidebar (⌘B)"
            return item
        case .toggleInspector:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.image = NSImage(systemSymbolName: "sidebar.right", accessibilityDescription: "Toggle inspector")
            item.target = self
            item.action = #selector(toggleInspector)
            item.label = "Toggle inspector"
            item.toolTip = "Show or hide selected activity (⌥⌘I)"
            return item
        case .commandPalette:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: "Command Palette")
            item.target = self
            item.action = #selector(showCommandPalette)
            item.label = "Command Palette"
            item.toolTip = "Search commands (⇧⌘P)"
            return item
        case .refreshWorkspace:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.image = NSImage(systemSymbolName: "arrow.clockwise", accessibilityDescription: "Refresh workspace")
            item.target = self
            item.action = #selector(refreshAllNow)
            item.label = "Refresh"
            item.toolTip = "Refresh workspace (⌘R)"
            return item
        case .badge:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            let label = NSTextField(labelWithString: "0")
            label.font = .monospacedDigitSystemFont(ofSize: 11, weight: .semibold)
            label.textColor = Theme.primary
            label.toolTip = "Increments landed today"
            item.view = label
            item.label = "Today"
            item.toolTip = "Increments landed today"
            badgeItem = item
            return item
        default:
            return nil
        }
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.toggleSidebar, .commandPalette, .navBack, .navForward, .navTitle, .flexibleSpace, .refreshWorkspace, .toggleInspector]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
NSSetUncaughtExceptionHandler { exception in
    let text = "UNCAUGHT \(exception.name.rawValue): \(exception.reason ?? "?")\n" + exception.callStackSymbols.joined(separator: "\n") + "\n"
    try? text.write(toFile: NSHomeDirectory() + "/agentwatcher-crash.log", atomically: true, encoding: .utf8)
    FileHandle.standardError.write(text.data(using: .utf8)!)
}
app.run()
