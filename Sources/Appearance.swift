import AppKit
import QuartzCore

enum AppearancePreference: String, CaseIterable {
    case system, light, dark

    var title: String {
        switch self {
        case .system: return "System"
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }

    var appearance: NSAppearance? {
        switch self {
        case .system: return nil
        case .light: return NSAppearance(named: .aqua)
        case .dark: return NSAppearance(named: .darkAqua)
        }
    }
}

extension Notification.Name {
    static let agentWatcherAppearanceChanged = Notification.Name("AgentWatcherAppearanceChanged")
}

extension NSColor {
    /// Unlike a retained NSColor, CGColor cannot follow appearance changes. A
    /// layer may also be built on a timer, outside AppKit's drawing context.
    /// Resolve its initial value against the app, rather than the default Aqua
    /// drawing appearance; AppearanceController maintains it after switches.
    var themeCGColor: CGColor {
        let appearance = NSApp?.effectiveAppearance ?? NSAppearance(named: .aqua)!
        return AppearanceController.resolved(self, in: appearance).cgColor
    }
}

/// One appearance preference for the application, including secondary windows.
/// Native colors stay dynamic; cached Core Animation colors are re-resolved in
/// place so changing appearance preserves editors, scroll positions, and state.
enum AppearanceController {
    static let preferenceKey = "appearancePreference"
    // Kept injectable so checks can use their own domain without changing the
    // person's saved appearance. The application uses standard defaults.
    static var preferenceStore = UserDefaults.standard
    private static var started = false
    private static var applicationObservation: NSKeyValueObservation?
    private static var windowObservations: [ObjectIdentifier: NSKeyValueObservation] = [:]
    private static var notifications: [NSObjectProtocol] = []
    private static var distributedObservation: NSObjectProtocol?
    private static var pendingRefresh = false

    static var preference: AppearancePreference {
        get {
            AppearancePreference(rawValue: preferenceStore.string(forKey: preferenceKey) ?? "") ?? .system
        }
        set {
            preferenceStore.set(newValue.rawValue, forKey: preferenceKey)
            start()
            NSApp.appearance = newValue.appearance
            for window in NSApp.windows { apply(to: window) }
            refreshAll()
            NotificationCenter.default.post(name: .agentWatcherAppearanceChanged, object: nil)
        }
    }

    static func start() {
        guard !started else { return }
        started = true
        NSApp.appearance = preference.appearance
        applicationObservation = NSApp.observe(\.effectiveAppearance, options: [.new]) { _, _ in
            scheduleRefresh()
        }
        notifications.append(NotificationCenter.default.addObserver(forName: NSWindow.didBecomeKeyNotification,
                                                                     object: nil, queue: .main) { notification in
            guard let window = notification.object as? NSWindow else { return }
            apply(to: window)
        })
        notifications.append(NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification,
                                                                     object: nil, queue: .main) { notification in
            guard let window = notification.object as? NSWindow else { return }
            windowObservations.removeValue(forKey: ObjectIdentifier(window))
        })
        distributedObservation = DistributedNotificationCenter.default().addObserver(
            forName: Notification.Name("AppleInterfaceThemeChangedNotification"), object: nil, queue: .main
        ) { _ in scheduleRefresh() }
    }

    static func apply(to window: NSWindow) {
        start()
        window.appearance = preference.appearance
        window.backgroundColor = Theme.base
        let identity = ObjectIdentifier(window)
        if windowObservations[identity] == nil {
            windowObservations[identity] = window.observe(\.effectiveAppearance, options: [.new]) { [weak window] _, _ in
                guard window != nil else { return }
                scheduleRefresh()
            }
        }
        // Content may be installed just after this call when a window is built.
        scheduleRefresh()
    }

    private static func scheduleRefresh() {
        guard !pendingRefresh else { return }
        pendingRefresh = true
        DispatchQueue.main.async {
            pendingRefresh = false
            refreshAll()
        }
    }

    static func refreshAll() {
        precondition(Thread.isMainThread)
        for window in NSApp.windows {
            let appearance = window.effectiveAppearance
            let palette = makePalette(for: appearance)
            var visitedLayers = Set<ObjectIdentifier>()
            appearance.performAsCurrentDrawingAppearance {
                CATransaction.begin()
                CATransaction.setDisableActions(true)
                if let root = window.contentView {
                    refresh(root, palette: palette, visitedLayers: &visitedLayers)
                }
                CATransaction.commit()
            }
            window.invalidateShadow()
        }
    }

    private static func refresh(_ view: NSView, palette: [PaletteEntry], visitedLayers: inout Set<ObjectIdentifier>) {
        if let layer = view.layer { refresh(layer, palette: palette, visitedLayers: &visitedLayers) }
        view.needsDisplay = true
        if let text = view as? NSTextView {
            text.layoutManager?.invalidateDisplay(forCharacterRange: NSRange(location: 0, length: text.string.utf16.count))
        }
        for child in view.subviews { refresh(child, palette: palette, visitedLayers: &visitedLayers) }
    }

    private static func refresh(_ layer: CALayer, palette: [PaletteEntry], visitedLayers: inout Set<ObjectIdentifier>) {
        guard visitedLayers.insert(ObjectIdentifier(layer)).inserted else { return }
        layer.backgroundColor = remap(layer.backgroundColor, palette: palette)
        layer.borderColor = remap(layer.borderColor, palette: palette)
        layer.shadowColor = remap(layer.shadowColor, palette: palette)
        if let shape = layer as? CAShapeLayer {
            shape.fillColor = remap(shape.fillColor, palette: palette)
            shape.strokeColor = remap(shape.strokeColor, palette: palette)
        }
        if let gradient = layer as? CAGradientLayer, let colors = gradient.colors as? [CGColor] {
            gradient.colors = colors.map { remap($0, palette: palette) ?? $0 }
        }
        for child in layer.sublayers ?? [] { refresh(child, palette: palette, visitedLayers: &visitedLayers) }
    }

    /// Translate only semantic palette colors. User images and arbitrary content
    /// colors remain intact, and alpha tints retain their original opacity.
    static func remap(_ color: CGColor?, to appearance: NSAppearance) -> CGColor? {
        remap(color, palette: makePalette(for: appearance))
    }

    private struct PaletteEntry {
        let source: NSColor
        let target: NSColor
    }

    private static func makePalette(for appearance: NSAppearance) -> [PaletteEntry] {
        let appearances: [NSAppearance.Name] = [.aqua, .darkAqua, .accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua]
        return Theme.semanticColors.flatMap { role in
            let target = resolved(role, in: appearance)
            return appearances.compactMap { name in
                NSAppearance(named: name).map { PaletteEntry(source: resolved(role, in: $0), target: target) }
            }
        }
    }

    private static func remap(_ color: CGColor?, palette: [PaletteEntry]) -> CGColor? {
        guard let color, let rgb = NSColor(cgColor: color)?.usingColorSpace(.sRGB), rgb.alphaComponent > 0 else { return color }
        var fallback: (NSColor, CGFloat)?
        for entry in palette {
            let candidate = entry.source
            guard abs(rgb.redComponent - candidate.redComponent) < 0.0001,
                  abs(rgb.greenComponent - candidate.greenComponent) < 0.0001,
                  abs(rgb.blueComponent - candidate.blueComponent) < 0.0001 else { continue }
            if abs(rgb.alphaComponent - candidate.alphaComponent) < 0.0001 {
                return entry.target.cgColor
            }
            if rgb.alphaComponent < candidate.alphaComponent, fallback == nil {
                fallback = (entry.target, rgb.alphaComponent / candidate.alphaComponent)
            }
        }
        if let (replacement, factor) = fallback {
            return replacement.withAlphaComponent(replacement.alphaComponent * factor).cgColor
        }
        return color
    }

    static func resolved(_ color: NSColor, in appearance: NSAppearance) -> NSColor {
        var result = color
        appearance.performAsCurrentDrawingAppearance {
            result = color.usingColorSpace(.sRGB) ?? color
        }
        return result
    }
}

/// Native, accessible selection shared by Settings and optional window chrome.
final class AppearancePicker: NSPopUpButton {
    private var observation: NSObjectProtocol?

    init() {
        super.init(frame: .zero, pullsDown: false)
        addItems(withTitles: AppearancePreference.allCases.map(\.title))
        target = self
        action = #selector(selectionChanged)
        toolTip = "Appearance: follow macOS, or always use Light or Dark"
        setAccessibilityLabel("Appearance")
        syncSelection()
        observation = NotificationCenter.default.addObserver(forName: .agentWatcherAppearanceChanged,
                                                              object: nil, queue: .main) { [weak self] _ in
            self?.syncSelection()
        }
    }

    required init?(coder: NSCoder) { fatalError() }
    deinit { if let observation { NotificationCenter.default.removeObserver(observation) } }

    private func syncSelection() {
        selectItem(at: AppearancePreference.allCases.firstIndex(of: AppearanceController.preference) ?? 0)
    }

    @objc private func selectionChanged() {
        guard AppearancePreference.allCases.indices.contains(indexOfSelectedItem) else { return }
        AppearanceController.preference = AppearancePreference.allCases[indexOfSelectedItem]
    }
}
