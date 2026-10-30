#if DEBUG
import AppKit
import SwiftUI

/// Drives the app from inside for checks that need key presses and for pictures of the window that
/// an agent looks at: macOS lets no outside process send key presses without the Accessibility
/// permission. Debug builds only. `NOTES_DEBUG_SCRIPT` holds steps separated by `;`:
/// `sleep:0.5`, `type:text`, `key:return` or `key:b+cmd`, `click:x,y` (points from the top left of
/// the window), `winshot:/path/to.png` (draws the main window into a file, on screen or not),
/// `sidebarshot:/path/to.png` (draws the table of the sidebar into a file and prints its rows),
/// `apphidden` (prints how many windows are on screen), `quit`,
/// `ghost` (puts the windows on screen transparent and deaf to the mouse: a window that is not on
/// screen draws no rows, and this one does not get in the way of whoever works at the Mac),
/// `mkfolder:Parent/Name`, `mknote:Title=text@Parent/Name` (the text and the folder may be left out; `\\n` is a line break),
/// `go:all`, `go:starred`, `go:archived`, `go:trash`, `go:untagged`, `go:nofolder`, `go:folder:Parent/Name`, `go:tag:name`, `go:filter:Name`,
/// `pick:Title` (opens that note), `list` (prints the rows of the list), `sidebar` (prints the views with their numbers and the folders),
/// `trash:Title`, `restore:Title`, `star:Title`, `pin:Title`, `archive:Title`, `lock:Title`, `tag:Title=name`,
/// `delfolder:Parent/Name` (prints the number the confirmation names, and deletes), `fold:Parent/Name`, `daily:Parent/Name`,
/// `sort:title` or `sort:created` or `sort:edited` (with `+asc`), `hide:preview`, `hide:date`,
/// `focus` (gives the text the keyboard), `caret:5`, `cursor` (prints the place of the cursor), `seen` (prints the text without what is hidden),
/// `typein:text` (types into the editor, `␠` for a space at an end of the text), `text` (prints the text in the core), `flush` (sends what is typed to the core),
/// `format:bold` (also `italic`, `strike`, `code`, `highlight`, `link`, `h1`, `h2`, `p`, `bullets`, `numbers`, `tasks`, `quote`), `select:2,6`,
/// `enter`, `tab`, `backtab` (as those keys in the text), `pastehtml:<b>x</b>` and `pasteurl:https://…` (paste from a pasteboard of the script),
/// `wait1` (a pause that ends a step of the history of typing), `redo`,
/// `files` (opens the view of files, `files:off` closes it), `attachments` (prints those of the open note, with whether the text has them),
/// `find:text` (searches, `find:` ends it), `hits`, `picker` (prints what the picker of links offers), `linked`, `fromselection`, `keycmd:insertNewline:`,
/// `versions` (prints them and the oldest with what differs), `restoreoldest`, `history` (opens the panel), `panelshot:/path/to.png` (draws the sheet on show),
/// `set:font=Literata` (also `mono`, `lineheight`, `spacing`, `align=justify`, `hyphenate=on`, `size`, `column`), `settingsshot:/path/to.png` (with the tab of `settingsTab`),
/// `importfull:/backup|/folder of files`, `exportall:/dir`, `exportjson:/dir`, `exporthtml:/dir`, `exportpdf:/file.pdf`,
/// `attach:/path` (attaches the file to the open note with a link at the cursor),
/// `corequiet:5,text` (the same without telling the window, as a sync still running),
/// `corewrite:5,text` (an edit of the core under the open editor, as a sync brings one),
/// `newnote` (the way ⌘N does), `newentry`, `undo`, `importsn:/path` (imports a backup of Standard Notes).
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
                // The undo manager closes its group when an event from the queue is done, and a step is
                // no such event: without one the steps of a script would be undone all at once, which
                // a person at the keyboard does not get. An empty event through the queue ends the step.
                if let done = NSEvent.otherEvent(
                    with: .applicationDefined, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: target?.windowNumber ?? 0, context: nil, subtype: 0, data1: 0, data2: 0) {
                    NSApp.postEvent(done, atStart: false)
                }
                try? await Task.sleep(for: .milliseconds(30))
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
        case "sidebarshot":
            // The table of the sidebar drawn on its own: the picture of the window leaves out what
            // lies on the glass of the sidebar.
            guard let content = target?.contentView, let table = tables(in: content).first,
                  let image = table.bitmapImageRepForCachingDisplay(in: table.bounds) else { return print("debug: no sidebar") }
            table.cacheDisplay(in: table.bounds, to: image)
            try? image.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: argument))
            print("debug: sidebar drawn, \(table.numberOfRows) rows")
        case "apphidden":
            let shown = NSApp.windows.filter { $0.canBecomeMain && $0.isVisible }.count
            print("debug: app windows on screen \(shown)")
        case "quit": NSApp.terminate(nil)
        case "ghost":
            NSApp.unhideWithoutActivation()
            // The window of the scene is made only now.
            for _ in 0..<40 where !NSApp.windows.contains(where: \.canBecomeMain) { try? await Task.sleep(for: .milliseconds(50)) }
            for window in NSApp.windows where window.canBecomeMain {
                window.alphaValue = 0
                window.ignoresMouseEvents = true
                window.orderFrontRegardless()
            }
        case "mkfolder":
            let parts = argument.split(separator: "/").map(String.init)
            model.createFolder(named: parts.last ?? argument, in: folderId(parts.dropLast().joined(separator: "/")))
        case "mknote":
            let (head, place) = cut(argument, "@")
            let (title, text) = cut(head, "=")
            model.perform { try $0.createNote(new: NewNote(title: title, folderId: folderId(place), tags: [], text: text.replacingOccurrences(of: "\\n", with: "\n"))) }
        case "go":
            let (kind, value) = cut(argument, ":")
            switch kind {
            case "all": model.scope = .all
            case "starred": model.scope = .starred
            case "archived": model.scope = .archived
            case "trash": model.scope = .trash
            case "untagged": model.scope = .untagged
            case "nofolder": model.scope = .noFolder
            case "folder": folderId(value).map { model.scope = .folder(id: $0) }
            case "tag": model.scope = .tag(name: value)
            case "filter": model.filters.first { $0.name == value }.map { model.scope = .filter(id: $0.id) }
            default: print("debug: no view \(argument)")
            }
        case "pick": note(argument).map { model.open($0.id) }
        case "list":
            print("debug: list \(model.notes.map { "\($0.shownTitle)\($0.pinned ? " pinned" : "")\($0.starred ? " starred" : "")" })")
        case "sidebar":
            let c = model.counts
            print("debug: views all \(c.all) nofolder \(c.noFolder) starred \(c.starred) archived \(c.archived) trash \(c.trash) untagged \(c.untagged)")
            print("debug: folders \(model.folders.map { "\(model.path(of: $0.id).map(\.name).joined(separator: "/")) \($0.noteCount)" })")
            print("debug: tags \(model.tags.map { "\($0.name) \($0.count)" }) filters \(model.filters.map { "\($0.name) \($0.count)" })")
        case "trash": note(argument).map { model.trash([$0.id]) }
        case "restore": note(argument).map { model.restore([$0.id]) }
        case "star": note(argument).map { model.setFlag(.starred, true, ids: [$0.id]) }
        case "pin": note(argument).map { model.setFlag(.pinned, true, ids: [$0.id]) }
        case "archive": note(argument).map { model.setFlag(.archived, true, ids: [$0.id]) }
        case "lock": note(argument).map { model.setFlag(.locked, true, ids: [$0.id]) }
        case "tag":
            let (title, tag) = cut(argument, "=")
            note(title).map { found in model.perform { try $0.addTag(id: found.id, tag: tag) } }
        case "delfolder":
            guard let id = folderId(argument) else { return print("debug: no folder \(argument)") }
            print("debug: the confirmation names \(model.deletionCount(id)) notes")
            model.deleteFolder(id)
        case "fold": folderId(argument).map { id in
            if model.foldedFolders.contains(id) { model.foldedFolders.remove(id) } else { model.foldedFolders.insert(id) }
        }
        case "daily": folderId(argument).map { model.setDaily($0, true) }
        case "sort":
            var display = model.display
            let (by, order) = cut(argument, "+")
            display.sortBy = by
            display.descending = order != "asc"
            model.setDisplay(display)
        case "hide":
            var display = model.display
            if argument == "preview" { display.showPreview = false } else { display.showDate = false }
            model.setDisplay(display)
        case "focus":
            // The keyboard to the text, in a window that is not the key one as well.
            if let view = editor { view.window?.makeFirstResponder(view) }
        case "caret":
            editor?.debugCaret(Int(argument) ?? 0)
            editor?.scrollRangeToVisible(editor?.selectedRange() ?? NSRange())
        case "seen": print("debug: seen \(editor?.debugSeen.debugDescription ?? "no editor")")
        case "text": print("debug: core text \(model.selectedNote.map { model.text(of: $0) }.debugDescription)")
        case "flush": model.flushEdits()
        case "typein":
            // Text into the editor the way typing does.
            editor?.insertText(argument.replacingOccurrences(of: "␠", with: " "), replacementRange: editor?.selectedRange() ?? NSRange())
        case "corewrite":
            // An edit of the core under the open editor, as a sync brings one: position,text.
            let (position, text) = cut(argument, ",")
            if let id = model.selectedNote {
                try? model.store?.spliceUtf16(id: id, pos: UInt32(position) ?? 0, deleteLen: 0, insert: text)
                model.reload()
            }
        case "format":
            let formats: [String: MarkdownFormat] = [
                "bold": .bold, "italic": .italic, "strike": .strikethrough, "code": .code, "highlight": .highlight, "link": .link,
                "h1": .heading(level: 1), "h2": .heading(level: 2), "p": .paragraph, "bullets": .bulletList, "numbers": .numberedList,
                "tasks": .taskList, "quote": .quote,
            ]
            formats[argument].map { editor?.format($0) }
        case "select":
            let (from, to) = cut(argument, ",")
            editor?.setSelectedRange(NSRange(location: Int(from) ?? 0, length: (Int(to) ?? 0) - (Int(from) ?? 0)))
        case "enter": editor?.insertNewline(nil)
        case "tab": editor?.insertTab(nil)
        case "backtab": editor?.insertBacktab(nil)
        case "pastehtml", "pasteurl":
            // A pasteboard of its own: the one of the person at the Mac stays as it is.
            let pasteboard = NSPasteboard(name: NSPasteboard.Name("org.evsyukov.notes.debug"))
            pasteboard.clearContents()
            pasteboard.setString(argument, forType: name == "pastehtml" ? .html : .string)
            print("debug: pasted \(editor?.paste(from: pasteboard) ?? false)")
        case "wait1": try? await Task.sleep(for: .seconds(TypingHistory.pause + 0.1))
        case "redo": model.undoManager?.redo()
        case "replaceall":
            // What Replace All of the find bar does: several ranges at once. Argument: from=to.
            let (from, to) = cut(argument, "=")
            guard let view = editor else { return }
            let text = view.string as NSString
            var ranges: [NSValue] = []
            var at = NSRange(location: 0, length: text.length)
            while true {
                let found = text.range(of: from, options: [], range: at)
                if found.location == NSNotFound { break }
                ranges.append(NSValue(range: found))
                at = NSRange(location: NSMaxRange(found), length: text.length - NSMaxRange(found))
            }
            if view.shouldChangeText(inRanges: ranges, replacementStrings: ranges.map { _ in to }) {
                for range in ranges.reversed() { view.textStorage?.replaceCharacters(in: range.rangeValue, with: to) }
                view.didChangeText()
            }
        case "find": model.search = argument
        case "hits": print("debug: hits \(model.hits.map { "\($0.note.shownTitle): \($0.snippet)" })")
        case "picker": print("debug: picker \(editor?.debugPicker ?? [])")
        case "linked": print("debug: linked from \(model.selectedNote.map { model.linkedFrom($0).map(\.shownTitle) } ?? [])")
        case "fromselection": model.noteFromSelection()
        case "keycmd":
            // A command of the text, as a key would send it: moveDown:, insertNewline: …
            editor?.doCommand(by: NSSelectorFromString(argument))
        case "versions":
            guard let id = model.selectedNote else { return }
            let versions = model.versions(of: id)
            print("debug: versions \(versions.map { "\($0.device) +\($0.added) -\($0.removed)" })")
            if let last = versions.last, let diff = model.versionDiff(id, last.id) {
                print("debug: oldest \(diff.text.debugDescription) changed \(diff.changed.map { "\($0.start)..<\($0.end)" }) removed at \(diff.removedAt)")
            }
        case "restoreoldest":
            if let id = model.selectedNote, let last = model.versions(of: id).last { model.restoreVersion(id, last.id) }
        case "history": model.historyOpen = true
        case "panelshot":
            // The sheet on show, drawn into a file.
            guard let sheet = NSApp.windows.first(where: { $0.isSheet }) ?? target?.attachedSheet,
                  let view = sheet.contentView, let image = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return print("debug: no sheet") }
            view.cacheDisplay(in: view.bounds, to: image)
            try? image.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: argument))
            print("debug: sheet drawn")
        case "set":
            // A setting, as the settings window changes it.
            let (key, value) = cut(argument, "=")
            var settings = model.settings
            switch key {
            case "font": settings.fontBody = value
            case "mono": settings.fontMono = value
            case "lineheight": settings.lineHeight = Double(value) ?? 1.5
            case "spacing": settings.paragraphSpacing = Double(value) ?? 0.5
            case "align": settings.align = value == "justify" ? .justify : .left
            case "hyphenate": settings.hyphenate = value == "on"
            case "size": model.textSize = Int(value) ?? 14
            case "column": model.columnWidth = ColumnWidth(rawValue: value) ?? .medium
            default: print("debug: no setting \(key)")
            }
            model.saveSettings(settings)
        case "settingsshot":
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 620), styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = NSHostingView(rootView: SettingsView().environment(model))
            window.alphaValue = 0
            window.orderFrontRegardless()
            try? await Task.sleep(for: .seconds(1))
            if let view = window.contentView, let image = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
                view.cacheDisplay(in: view.bounds, to: image)
                try? image.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: argument))
                print("debug: settings drawn")
            }
            window.close()
        case "importfull":
            // A backup the way File → Import… takes it: path|files folder.
            let (path, files) = cut(argument, "|")
            model.importBackup(URL(fileURLWithPath: path), filesDir: files.isEmpty ? nil : URL(fileURLWithPath: files), options: ImportOptions(underOneFolder: false, collectUnlinkedFiles: true))
            try? await Task.sleep(for: .seconds(1))
            print("debug: report \(model.importReport ?? "none")")
        case "exportall":
            print("debug: exported \(String(describing: try? model.store?.exportMarkdown(scope: .all, dir: argument)))")
        case "exportjson":
            print("debug: exported \(String(describing: try? model.store?.exportJson(dir: argument, withAttachments: true)))")
        case "exporthtml":
            if let id = model.selectedNote { print("debug: html \(String(describing: try? model.store?.exportHtml(id: id, dir: argument, embedAttachments: true)))") }
        case "exportpdf":
            guard let id = model.selectedNote, let store = model.store else { return }
            var finished = false
            model.savePdf(of: id, to: URL(fileURLWithPath: argument), paper: PDFExport.a4, store: store) { finished = true }
            for _ in 0..<100 where !finished { try? await Task.sleep(for: .milliseconds(100)) }
            print("debug: pdf \(finished), alert \(model.alert ?? "none")")
        case "files": model.filesOpen = argument != "off"
        case "attachments":
            print("debug: attachments \(model.attachments.map { "\($0.name) \(model.isInText($0) ? "in the text" : "not in the text")" })")
        case "attach": model.selectedNote.map { model.attach([URL(fileURLWithPath: argument)], to: $0) }
        case "corequiet":
            // An edit of the core the window has not heard of, as a sync in the background makes one.
            let (position, text) = cut(argument, ",")
            if let id = model.selectedNote { try? model.store?.spliceUtf16(id: id, pos: UInt32(position) ?? 0, deleteLen: 0, insert: text) }
        case "cursor": print("debug: cursor \(editor?.selectedRange().location ?? -1)")
        case "newnote": model.newNote()
        case "newentry": model.newEntryForToday()
        case "undo":
            print("debug: undo manager \(model.undoManager == nil ? "none" : "there"), can undo \(model.undoManager?.canUndo ?? false)")
            model.undoManager?.undo()
        case "coreundo": print("debug: core undo \(String(describing: try? model.store?.undo()))"); model.reload()
        case "importsn":
            model.perform { store in
                let report = try store.importStandardNotes(path: argument, filesDir: nil, options: ImportOptions(underOneFolder: false, collectUnlinkedFiles: false))
                print("debug: import \(report.text)")
            }
        default: print("debug: unknown step \(name)")
        }
    }

    private static var model: AppModel { AppModel.shared }

    /// The text view of the open note.
    private static var editor: MarkdownTextView? {
        func find(_ view: NSView) -> MarkdownTextView? {
            (view as? MarkdownTextView) ?? view.subviews.lazy.compactMap(find).first
        }
        return target?.contentView.flatMap(find)
    }

    /// The tables under a view, the leftmost first.
    private static func tables(in view: NSView) -> [NSTableView] {
        let own = (view as? NSTableView).map { [$0] } ?? []
        let found = own + view.subviews.flatMap(tables)
        return found.sorted { $0.convert($0.bounds, to: nil).minX < $1.convert($1.bounds, to: nil).minX }
    }

    /// The text before and after the first `separator`.
    private static func cut(_ text: String, _ separator: Character) -> (String, String) {
        guard let at = text.firstIndex(of: separator) else { return (text, "") }
        return (String(text[..<at]), String(text[text.index(after: at)...]))
    }

    private static func folderId(_ path: String) -> String? {
        model.folders.first { model.path(of: $0.id).map(\.name).joined(separator: "/") == path }?.id
    }

    /// The note of that title, wherever it is.
    private static func note(_ title: String) -> Note? {
        let scopes: [Scope] = [.all, .archived, .trash]
        return scopes.lazy.compactMap { try? model.store?.notes(scope: $0) }.joined().first { $0.title == title }
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
