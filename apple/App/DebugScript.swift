#if DEBUG
import AppKit
import SwiftUI

/// Drives the app from inside for checks that need key presses and for pictures of the window that
/// an agent looks at: macOS lets no outside process send key presses without the Accessibility
/// permission. Debug builds only. `NOTES_DEBUG_SCRIPT` holds steps separated by `;`:
/// `sleep:0.5`, `type:text`, `key:return` or `key:b+cmd`, `click:x,y` (points from the top left of
/// the window), `winshot:/path/to.png` (draws the main window into a file, on screen or not),
/// `apphidden` (prints how many windows are on screen), `quit`.
/// With `NOTES_DEBUG_DIR=/folder` the run has its data and settings in that folder. With
/// `NOTES_DEBUG_QUIET` the app stays in the background; key presses then go straight to the main
/// window. As in Lists.
@MainActor
enum DebugScript {
    private static let codes: [String: (UInt16, String)] = [
        "return": (36, "\r"), "esc": (53, "\u{1b}"), "space": (49, " "), "down": (125, "\u{F701}"), "up": (126, "\u{F700}"),
        "left": (123, "\u{F702}"), "right": (124, "\u{F703}"), "tab": (48, "\t"), "backtab": (48, "\u{19}"), "delete": (51, "\u{7f}"),
    ]

    static func runIfAsked() {
        guard let script = ProcessInfo.processInfo.environment["NOTES_DEBUG_SCRIPT"] else { return }
        setlinebuf(stdout)
        let steps = script.split(separator: ";").map { String($0).trimmingCharacters(in: .whitespaces) }
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            let quiet = ProcessInfo.processInfo.environment["NOTES_DEBUG_QUIET"] != nil
            for _ in 0..<30 where !quiet && NSApp.keyWindow == nil {
                NSApp.activate(ignoringOtherApps: true)
                NSApp.windows.first { $0.canBecomeMain }?.makeKeyAndOrderFront(nil)
                try? await Task.sleep(for: .milliseconds(150))
            }
            for step in steps {
                let name = step.split(separator: ":", maxSplits: 1).first.map(String.init) ?? step
                let argument = step.contains(":") ? String(step.dropFirst(name.count + 1)) : ""
                await run(name, argument)
            }
        }
    }

    private static func run(_ name: String, _ argument: String) async {
        switch name {
        case "sleep": try? await Task.sleep(for: .seconds(Double(argument) ?? 0.5))
        case "type": for character in argument { press(String(character), code: 0, flags: []) }
        case "key":
            let parts = argument.split(separator: "+").map(String.init)
            var flags: NSEvent.ModifierFlags = []
            if parts.contains("cmd") { flags.insert(.command) }
            if parts.contains("opt") { flags.insert(.option) }
            if parts.contains("shift") { flags.insert(.shift) }
            let key = codes[parts[0]] ?? (0, parts[0])
            press(key.1, code: key.0, flags: flags)
        case "click":
            let xy = argument.split(separator: ",").compactMap { Double($0) }
            guard xy.count >= 2, let window = target else { return }
            let point = NSPoint(x: xy[0], y: Double(window.frame.height) - xy[1])
            for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                if let event = NSEvent.mouseEvent(
                    with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1) {
                    NSApp.postEvent(event, atStart: false)
                }
            }
        case "winshot":
            // The whole main window drawn into a file, hidden or not.
            guard let view = target?.contentView?.superview ?? target?.contentView,
                  let image = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return print("debug: no window to draw") }
            view.cacheDisplay(in: view.bounds, to: image)
            try? image.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: argument))
            print("debug: window drawn, \(image.pixelsWide)x\(image.pixelsHigh) pixels")
        case "apphidden":
            let shown = NSApp.windows.filter { $0.canBecomeMain && $0.isVisible }.count
            print("debug: app windows on screen \(shown)")
        case "quit": NSApp.terminate(nil)
        default: print("debug: unknown step \(name)")
        }
    }

    /// The window the script types into: the key window, or the main window of an app in the background.
    static var target: NSWindow? {
        NSApp.keyWindow ?? NSApp.windows.first { $0.canBecomeMain && $0.isVisible }
    }

    private static func press(_ characters: String, code: UInt16, flags: NSEvent.ModifierFlags) {
        guard let window = target else { return print("debug: no window for \(characters)") }
        for type in [NSEvent.EventType.keyDown, .keyUp] {
            if let event = NSEvent.keyEvent(
                with: type, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, characters: characters,
                charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code) {
                // The application hands key events to its key window only; a window in the
                // background takes them directly, shortcuts as key equivalents.
                if window.isKeyWindow {
                    NSApp.sendEvent(event)
                } else if type != .keyDown || !flags.contains(.command) || !window.performKeyEquivalent(with: event) {
                    window.sendEvent(event)
                }
            }
        }
    }
}
#endif
