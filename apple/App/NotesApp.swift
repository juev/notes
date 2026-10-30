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
        CommandGroup(replacing: .undoRedo) {
            // R75: the actions of the core, or the typing of the field that has the keyboard (E25).
            Button(L("Undo")) { model.undoManager?.undo() }
                .keyboardShortcut("z")
            Button(L("Redo")) { model.undoManager?.redo() }
                .keyboardShortcut("z", modifiers: [.command, .shift])
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        #if DEBUG
        MainActor.assumeIsolated { DebugScript.runIfAsked() }
        #endif
    }
}
