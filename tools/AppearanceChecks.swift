// Executed against actual application types by the regression-check harness.
func runAppearanceChecks() {
    func check(_ value: @autoclosure () -> Bool, _ label: String) {
        if !value() { fatalError("FAIL: " + label) }
    }
    func close(_ a: NSColor, _ b: NSColor) -> Bool {
        let a = a.usingColorSpace(.sRGB)!, b = b.usingColorSpace(.sRGB)!
        return abs(a.redComponent - b.redComponent) < 0.0001 && abs(a.greenComponent - b.greenComponent) < 0.0001 && abs(a.blueComponent - b.blueComponent) < 0.0001 && abs(a.alphaComponent - b.alphaComponent) < 0.0001
    }
    func luminance(_ color: NSColor) -> Double {
        let c = color.usingColorSpace(.sRGB)!
        let linear = [c.redComponent,c.greenComponent,c.blueComponent].map { x -> Double in
            let x = Double(x)
            return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4)
        }
        return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
    }
    let _ = NSApplication.shared
    let originalStore = AppearanceController.preferenceStore
    let originalApplicationAppearance = NSApp.appearance
    let originalWindowAppearances = NSApp.windows.map { ($0, $0.appearance) }
    let suiteName = "org.agentwatcher.AppearanceChecks." + UUID().uuidString
    let suite = UserDefaults(suiteName:suiteName)!
    AppearanceController.preferenceStore = suite
    defer {
        AppearanceController.preferenceStore = originalStore
        NSApp.appearance = originalApplicationAppearance
        for (window, appearance) in originalWindowAppearances { window.appearance = appearance }
        AppearanceController.refreshAll()
        suite.removePersistentDomain(forName:suiteName)
    }
    check(AppearanceController.preference == .system, "first launch follows System")
    suite.set("invalid",forKey:AppearanceController.preferenceKey)
    check(AppearanceController.preference == .system, "invalid preference falls back to System")
    let light = NSAppearance(named: .aqua)!, dark = NSAppearance(named: .darkAqua)!
    for (mode, appearance) in [("Light",light),("Dark",dark)] {
        var minimum = Double.infinity
        let surfaces = [Theme.base,Theme.deep,Theme.layer01,Theme.layer02,Theme.layer03]
        let text = [Theme.textBase,Theme.textMuted,Theme.textFaint,Theme.primary,Theme.accent,Theme.success,Theme.warning,Theme.danger,Theme.plan,Theme.explore,Theme.review,Theme.writer]
        for foreground in text {
            for background in surfaces {
                let f = luminance(AppearanceController.resolved(foreground,in:appearance))
                let b = luminance(AppearanceController.resolved(background,in:appearance))
                let contrast = (max(f,b) + 0.05) / (min(f,b) + 0.05)
                minimum = min(minimum,contrast)
                check(contrast >= 4.5, mode + " text contrast")
            }
        }
        print(String(format: "%@ minimum normal-text contrast %.2f:1", mode, minimum))
    }
    check(!close(AppearanceController.resolved(Theme.base,in:light),AppearanceController.resolved(Theme.base,in:dark)), "dynamic background variants")
    let _ = NSApplication.shared
    AppearanceController.preference = .light
    let window = NSWindow(contentRect: NSRect(x:0,y:0,width:600,height:400),styleMask:[.titled],backing:.buffered,defer:false)
    window.isReleasedWhenClosed = false
    defer { window.close() }
    let root = NSView(frame: NSRect(x:0,y:0,width:600,height:400))
    root.wantsLayer = true
    root.layer!.backgroundColor = AppearanceController.resolved(Theme.layer01,in:light).cgColor
    root.layer!.borderColor = AppearanceController.resolved(Theme.border,in:light).cgColor
    root.layer!.borderWidth = 1
    let tint = CALayer()
    tint.backgroundColor = AppearanceController.resolved(Theme.danger,in:light).withAlphaComponent(0.18).cgColor
    root.layer!.addSublayer(tint)
    let custom = CALayer()
    let customColor = NSColor(srgbRed:0.123,green:0.456,blue:0.789,alpha:0.67)
    custom.backgroundColor = customColor.cgColor
    root.layer!.addSublayer(custom)
    let text = NSTextView(frame: NSRect(x:10,y:10,width:300,height:100))
    text.string = "Selection stays where the user left it."
    text.setSelectedRange(NSRange(location:4,length:9))
    root.addSubview(text)
    let picker = AppearancePicker()
    root.addSubview(picker)
    window.contentView = root
    AppearanceController.apply(to:window)
    AppearanceController.preference = .dark
    check(close(NSColor(cgColor:Theme.layer01.themeCGColor)!,AppearanceController.resolved(Theme.layer01,in:dark)), "new layer colors resolve under current Dark app appearance")
    check(window.appearance?.name == .darkAqua, "secondary window follows Dark override")
    check(close(NSColor(cgColor:root.layer!.backgroundColor!)!,AppearanceController.resolved(Theme.layer01,in:dark)), "cached card background refresh")
    check(close(NSColor(cgColor:tint.backgroundColor!)!,AppearanceController.resolved(Theme.danger,in:dark).withAlphaComponent(0.18)), "cached status tint and alpha refresh")
    check(close(NSColor(cgColor:custom.backgroundColor!)!,customColor), "content-specific colors remain unchanged")
    check(text.selectedRange() == NSRange(location:4,length:9), "theme switch preserves text selection")
    check(picker.indexOfSelectedItem == 2, "appearance picker syncs external changes")
    AppearanceController.preference = .light
    check(close(NSColor(cgColor:root.layer!.backgroundColor!)!,AppearanceController.resolved(Theme.layer01,in:light)), "cached surface returns to Light")
    check(text.selectedRange() == NSRange(location:4,length:9), "roundtrip preserves selection")
    AppearanceController.preference = .dark
    suite.synchronize()
    let reopenedStore = UserDefaults(suiteName:suiteName)!
    check(reopenedStore.string(forKey:AppearanceController.preferenceKey) == "dark", "appearance preference stored in defaults")
    AppearanceController.preferenceStore = reopenedStore
    check(AppearanceController.preference == .dark, "reopened preferences restore Dark override")
    AppearanceController.preference = .system
    check(window.appearance == nil && NSApp.appearance == nil, "System clears app and window overrides")
    // A native effective-appearance change exercises the live observer path.
    NSApp.appearance = dark
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline && !close(NSColor(cgColor:root.layer!.backgroundColor!)!,AppearanceController.resolved(Theme.layer01,in:dark)) {
        RunLoop.main.run(until:Date().addingTimeInterval(0.01))
    }
    check(close(NSColor(cgColor:root.layer!.backgroundColor!)!,AppearanceController.resolved(Theme.layer01,in:dark)), "live effective appearance observation updates cached layers")
    window.appearance = NSAppearance(named:.accessibilityHighContrastDarkAqua)
    AppearanceController.refreshAll()
    check(close(NSColor(cgColor:root.layer!.borderColor!)!,AppearanceController.resolved(Theme.border,in:window.effectiveAppearance)), "Increase Contrast refreshes cached separator opacity")
    print("PASS: 120 contrast pairs, dynamic colors, cached layers and tints, content-color preservation, selection preservation, isolated defaults roundtrip, native effective appearance observation, System override reset, Increase Contrast, and picker synchronization")
}
