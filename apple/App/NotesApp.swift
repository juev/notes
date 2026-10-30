import AppKit
import SwiftUI

@main
struct NotesApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel.shared

    var body: some Scene {
        Window("Notes", id: "main") {
            MainWindow()
                .environment(model)
        }
        .defaultSize(width: 1100, height: 700)
        .commands { AppCommands(model: model) }

        Settings {
            SettingsView()
                .environment(model)
        }
    }
}

/// The menus of the app; the formatting of the editor comes with the editor.
struct AppCommands: Commands {
    let model: AppModel

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button(L("New Note")) { model.newNote() }
                .keyboardShortcut("n")
            if model.folderOfScope?.daily == true {
                Button(L("New Entry for Today")) { model.newEntryForToday() }
            }
        }
        CommandGroup(after: .pasteboard) {
            // E23: as it is, without the conversion of rich text.
            Button(L("Paste as Plain Text")) { NSApp.sendAction(#selector(NSTextView.pasteAsPlainText(_:)), to: nil, from: nil) }
                .keyboardShortcut("v", modifiers: [.command, .option, .shift])
        }
        CommandGroup(after: .textEditing) {
            // R48: find with replace in the open note.
            Button(L("Find…")) { find(.showFindInterface) }
                .keyboardShortcut("f")
            Button(L("Find and Replace…")) { find(.showReplaceInterface) }
                .keyboardShortcut("f", modifiers: [.command, .option])
            Button(L("Find Next")) { find(.nextMatch) }
                .keyboardShortcut("g")
            Button(L("Find Previous")) { find(.previousMatch) }
                .keyboardShortcut("g", modifiers: [.command, .shift])
        }
        CommandMenu(L("Format")) {
            format(L("Bold"), .bold, "b")
            format(L("Italic"), .italic, "i")
            format(L("Strikethrough"), .strikethrough, "x", [.command, .shift])
            format(L("Code"), .code, "e")
            format(L("Highlight"), .highlight, "h", [.command, .shift])
            format(L("Link"), .link, "k")
            Divider()
            format(L("Heading 1"), .heading(level: 1), "1")
            format(L("Heading 2"), .heading(level: 2), "2")
            format(L("Heading 3"), .heading(level: 3), "3")
            format(L("Plain Paragraph"), .paragraph, "0")
            Divider()
            format(L("Bulleted List"), .bulletList, "8", [.command, .shift])
            format(L("Numbered List"), .numberedList, "7", [.command, .shift])
            format(L("Task List"), .taskList, "9", [.command, .shift])
            format(L("Quote"), .quote, ".", [.command, .shift])
        }
        CommandGroup(replacing: .undoRedo) {
            // R75: the actions of the core, or the typing of the field that has the keyboard (E25).
            Button(L("Undo")) { model.undoManager?.undo() }
                .keyboardShortcut("z")
            Button(L("Redo")) { model.undoManager?.redo() }
                .keyboardShortcut("z", modifiers: [.command, .shift])
        }
    }

    private func find(_ action: NSTextFinder.Action) {
        let sender = NSMenuItem()
        sender.tag = action.rawValue
        model.activeEditor?.performTextFinderAction(sender)
    }

    /// E20: a format with its shortcut.
    private func format(_ title: String, _ format: MarkdownFormat, _ key: KeyEquivalent, _ modifiers: EventModifiers = .command) -> some View {
        Button(title) { model.activeEditor?.format(format) }
            .keyboardShortcut(key, modifiers: modifiers)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        AttachmentFiles.clear()
        MainActor.assumeIsolated { AppModel.shared.applyTheme() }
        #if DEBUG
        MainActor.assumeIsolated { DebugScript.runIfAsked() }
        #endif
    }
}
