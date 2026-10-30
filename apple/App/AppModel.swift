import AppKit
import Observation

/// A URL for a sheet.
struct IdentifiedURL: Identifiable {
    let id = UUID()
    let url: URL
}

/// The state of the app and its way to the core. Core calls are local SQLite queries and run on the
/// main thread; only sync, which talks to the network, runs in the background. As in Lists.
@MainActor
@Observable
final class AppModel {
    static let shared = AppModel()

    let store: Store?
    var startupError: String?

    var syncStatus = SyncStatus(configured: false, pending: 0, attachmentsWaiting: 0, lastOk: nil, lastError: nil, encrypted: false)
    var syncing = false
    /// What the main window shows as a message, dismissed by the user.
    var alert: String?

    // MARK: What the window shows

    /// The view open in the sidebar (R44).
    var scope: Scope = .all {
        didSet { if scope != oldValue { selectedNote = nil; reloadNotes() } }
    }
    var notes: [Note] = []
    var counts = Counts(all: 0, starred: 0, archived: 0, trash: 0, untagged: 0, noFolder: 0)
    var folders: [Folder] = []
    var tags: [TagCount] = []
    var filters: [SavedFilter] = []
    var settings = AppSettings(
        trashDays: 30, fontBody: "system", fontMono: "system", lineHeight: 1.5, paragraphSpacing: 0.5, align: .left,
        hyphenate: false, smartTypography: true, spellcheck: true, spellLanguage: "auto", newNoteFolder: "root")
    /// R52: the note in the Trash a link leads to, while the offer to restore it is on show.
    var restoreOffer: String?
    /// The attachments of the open note.
    var attachments: [Attachment] = []
    /// R37: the view "Files" is open in place of the list of notes.
    var filesOpen = false
    /// The note open in the editor.
    var selectedNote: String? {
        willSet { flushEdits() }
        didSet {
            if selectedNote != oldValue {
                attachments = selectedNote.flatMap { try? store?.attachments(noteId: $0) } ?? []
                if let left = oldValue { dropIfUntouched(left) }
            }
        }
    }
    /// The text view of the open note, for the formats of the menu.
    @ObservationIgnored weak var activeEditor: MarkdownTextView?
    /// Grows when the data was read again, so that an open text compares itself with the core.
    var textRevision = 0
    /// Grows to ask the text of the open note to take the keyboard.
    var textFocusRequests = 0
    /// R2: a new note opens with the cursor in its title.
    @ObservationIgnored private var titleFocusAsked = false
    /// What was typed and has not reached the core yet, with the note and the whole text it gives.
    @ObservationIgnored private var edits = EditCoalescer()
    @ObservationIgnored private var editedNote: String?
    @ObservationIgnored private var editedText = ""
    @ObservationIgnored private var editFlush: DispatchWorkItem?
    /// R3: a note made here and not edited yet; it is deleted without going to the Trash when left so.
    @ObservationIgnored private var freshNote: String?

    /// E41: the size of the text on this device.
    var textSize: Int = Int(Typography.size(
        saved: Storage.defaults.object(forKey: "textSize") as? Int, system: NSFont.systemFontSize + 2)) {
        didSet { Storage.defaults.set(textSize, forKey: "textSize"); typographyRevision += 1 }
    }
    /// E44: the width of the column on this device.
    var columnWidth = ColumnWidth(rawValue: Storage.defaults.string(forKey: "columnWidth") ?? "") ?? .medium {
        didSet { Storage.defaults.set(columnWidth.rawValue, forKey: "columnWidth"); typographyRevision += 1 }
    }
    /// R80: system, light or dark.
    var theme = Storage.defaults.string(forKey: "theme") ?? "system" {
        didSet { Storage.defaults.set(theme, forKey: "theme"); applyTheme() }
    }
    /// Grows when the look of the text changes.
    var typographyRevision = 0
    /// R12: the folders collapsed in the sidebar, a setting of this device.
    var foldedFolders: Set<String> = Set(Storage.defaults.stringArray(forKey: "foldedFolders") ?? []) {
        didSet { Storage.defaults.set(Array(foldedFolders), forKey: "foldedFolders") }
    }
    /// R16: the display of the system views, a setting of this device, by view.
    var viewDisplay: [String: ViewDisplay] = ViewDisplay.stored() {
        didSet { ViewDisplay.store(viewDisplay) }
    }

    @ObservationIgnored private var syncDebounce: DispatchWorkItem?
    @ObservationIgnored private var timer: Timer?
    @ObservationIgnored private let folderWatcher = FolderWatcher()
    @ObservationIgnored weak var undoManager: UndoManager?

    private init() {
        #if DEBUG
        // The check of the launch (`scripts/check-launch.sh`): a task of the main actor that asks for
        // the model runs after the model is built, unless something on the way runs the event loop.
        if ProcessInfo.processInfo.environment["NOTES_DEBUG_LAUNCH_PROBE"] != nil {
            Task { @MainActor in
                _ = AppModel.shared
                print("debug: launch probe ran after the model was built")
            }
        }
        #endif
        do {
            let opened = try Storage.openStore()
            store = opened
            // R95: a setting that is not read leaves its secret out of the core, and sync or push fails later.
            func read<T>(_ source: String, _ what: String, _ value: () throws -> T) -> T? {
                do { return try value() } catch {
                    opened.log(level: .error, source: source, message: "the \(what) were not read at launch: \(logged(error))")
                    return nil
                }
            }
            let syncConfig = read("sync", "sync settings", opened.syncConfig)
            if case .webDav(let url, let user)? = syncConfig {
                opened.setSyncPassword(password: Keychain.load(account: Keychain.account(url: url, user: user)))
            }
            if let account = syncConfig.flatMap(Keychain.passphraseAccount) {
                opened.setSyncPassphrase(passphrase: Keychain.load(account: account))
            }
            if let server = read("push", "push settings", opened.pushServer) ?? nil {
                opened.setPushToken(token: Keychain.load(account: Keychain.pushAccount(server: server)))
            }
            // S71: the other devices name this one in the history of versions.
            read("app", "name of the device") { try opened.setDeviceName(name: Host.current().localizedName ?? "Mac") }
            // R95: where a log of events begins for this run.
            let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
            let system = ProcessInfo.processInfo.operatingSystemVersion
            opened.log(level: .info, source: "app", message: "started \(version) on macOS \(system.majorVersion).\(system.minorVersion).\(system.patchVersion)")
        } catch {
            store = nil
            startupError = describe(error)
        }
        reload()
        DistributedNotificationCenter.default().addObserver(
            forName: Storage.changedNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.reload(); self?.scheduleSync() }
        }
        NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.reload(); self?.syncNow() }
        }
        for name in [NSApplication.willResignActiveNotification, NSApplication.willTerminateNotification] {
            NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.flushEdits() }
            }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.syncNow(); self?.backupIfDue() }
        }
        backupIfDue()
        watchSyncFolder()
        listenForNudges()
    }

    /// Reads again what the window shows.
    func reload() {
        guard let store else { return }
        flushEdits()
        syncStatus = (try? store.syncStatus()) ?? syncStatus
        counts = (try? store.counts()) ?? counts
        folders = (try? store.folders()) ?? folders
        tags = (try? store.tags()) ?? tags
        filters = (try? store.filters()) ?? filters
        settings = (try? store.settings()) ?? settings
        // A view whose folder, tag or filter is gone falls back to all notes.
        switch scope {
        case .folder(let id) where !folders.contains { $0.id == id }: scope = .all
        case .tag(let name) where !tags.contains { $0.name.lowercased() == name.lowercased() }: scope = .all
        case .filter(let id) where !filters.contains { $0.id == id }: scope = .all
        default: break
        }
        reloadNotes()
        if isSearching { reloadSearch() }
        let files = selectedNote.flatMap { try? store.attachments(noteId: $0) } ?? []
        if files != attachments { attachments = files; activeEditor?.reloadAttachments() }
        textRevision += 1
    }

    /// The notes of the open view, in the order of its display (R16, R25).
    func reloadNotes() {
        guard let store else { return }
        var notes = (try? store.notes(scope: scope)) ?? []
        if scope != .trash, case nil = folderOfScope {
            notes = ViewDisplay.sorted(notes, by: display.sort)
        }
        self.notes = notes
        if let selected = selectedNote, !notes.contains(where: { $0.id == selected }), (try? store.getNote(id: selected)) == nil {
            selectedNote = nil
        }
    }

    // MARK: The open note

    var openNote: Note? {
        guard let id = selectedNote else { return nil }
        return notes.first { $0.id == id } ?? (try? store?.getNote(id: id))
    }

    func text(of id: String) -> String { (try? store?.getText(id: id)) ?? "" }

    var hasPendingEdits: Bool { edits.pending != nil }

    func focusText() { textFocusRequests += 1 }

    /// Whether a note was opened new, once: its title takes the cursor (R2).
    func takeTitleFocus() -> Bool {
        defer { titleFocusAsked = false }
        return titleFocusAsked
    }

    /// R4: what is typed is saved as it is typed. The edits of a word reach the core together: it
    /// opens the document for each edit, and a large note takes it tens of milliseconds.
    func textEdited(_ id: String, _ edit: TextEdit, whole: String) {
        touched(id)
        if editedNote != id { flushEdits() }
        editedNote = id
        editedText = whole
        if let earlier = edits.add(edit) { apply(earlier, to: id) }
        editFlush?.cancel()
        let work = DispatchWorkItem { [weak self] in MainActor.assumeIsolated { self?.flushEdits() } }
        editFlush = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4, execute: work)
    }

    /// Everything typed so far goes to the core: before another note opens, before the data is read
    /// again, and when the app leaves the foreground.
    func flushEdits() {
        editFlush?.cancel()
        guard let id = editedNote, let edit = edits.take() else { return }
        apply(edit, to: id)
        // S3: the core must hold what the editor shows; whatever got past the edits is put right.
        if let store, (try? store.getText(id: id)) != editedText {
            do { try store.setText(id: id, text: editedText) } catch { alert = describe(error) }
        }
        reloadNotes()
        scheduleSync()
    }

    private func apply(_ edit: TextEdit, to id: String) {
        guard let store else { return }
        do {
            try store.spliceUtf16(id: id, pos: UInt32(edit.position), deleteLen: UInt32(edit.deleted), insert: edit.inserted)
        } catch {
            alert = describe(error)
        }
    }

    func setTitle(_ id: String, _ title: String) {
        touched(id)
        perform(named: L("Typing")) { try $0.setTitle(id: id, title: title) }
    }

    func addTag(_ id: String, _ tag: String) {
        touched(id)
        perform(named: L("Add Tag")) { try $0.addTag(id: id, tag: tag) }
    }

    func removeTag(_ id: String, _ tag: String) {
        perform(named: L("Remove Tag")) { try $0.removeTag(id: id, tag: tag) }
    }

    /// E13: a link clicked with ⌘: a note opens here, an address in the system.
    func openLink(_ url: URL) {
        if url.scheme == "note" {
            let id = String(url.absoluteString.dropFirst("note:".count))
            switch linkState(id) {
            case .live: open(id)
            // R52: a tap on a link to a note in the Trash offers to restore it.
            case .inTrash: restoreOffer = id
            case .gone: break
            }
        } else {
            NSWorkspace.shared.open(url)
        }
    }

    // MARK: Import and export

    /// R65: the backup chosen, while its sheet is on show.
    var importing: IdentifiedURL?
    /// I12: the report of the last import, while it is on show.
    var importReport: String?
    /// An import or an export is running.
    var working = false

    func chooseBackup() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.message = L("Choose a decrypted backup of Standard Notes: a zip, its folder or its top file")
        if panel.runModal() == .OK, let url = panel.url { importing = IdentifiedURL(url: url) }
    }

    /// I2: one action of undo. It runs off the main thread: the files of a backup may be large.
    func importBackup(_ url: URL, filesDir: URL?, options: ImportOptions) {
        guard let store else { return }
        flushEdits()
        working = true
        let before = store.undoRevision()
        Task.detached(priority: .userInitiated) {
            let result = Result { try store.importStandardNotes(path: url.path, filesDir: filesDir?.path, options: options) }
            await MainActor.run {
                self.working = false
                switch result {
                case .success(let report):
                    self.importReport = report.text
                    if store.undoRevision() != before {
                        self.undoManager?.registerUndo(withTarget: self) { model in model.perform { _ = try $0.undo() } }
                        self.undoManager?.setActionName(L("Import"))
                    }
                case .failure(let error): self.alert = describe(error)
                }
                self.reload()
                self.scheduleSync()
            }
        }
    }

    /// R60: the open note, the open folder or all notes, into a chosen folder.
    func exportMarkdown(_ scope: ExportScope) {
        guard let store, let dir = ImportSheet.pickFolder() else { return }
        flushEdits()
        run { try store.exportMarkdown(scope: scope, dir: dir.path) }
    }

    /// R62: everything as JSON, with or without the attachments.
    func exportJson(withAttachments: Bool) {
        guard let store, let dir = ImportSheet.pickFolder() else { return }
        flushEdits()
        run { try store.exportJson(dir: dir.path, withAttachments: withAttachments) }
    }

    private func run(_ export: @escaping @Sendable () throws -> ExportReport) {
        working = true
        Task.detached(priority: .userInitiated) {
            let result = Result { try export() }
            await MainActor.run {
                self.working = false
                switch result {
                case .success(let report):
                    self.alert = L("Exported %d notes and %d attachments.", Int(report.notes), Int(report.attachments))
                        + (report.missing.isEmpty ? "" : "\n" + L("Not on this device: %@", report.missing.joined(separator: ", ")))
                case .failure(let error): self.alert = describe(error)
                }
            }
        }
    }

    /// R61: the open note as HTML, its attachments embedded or next to it.
    func exportHtml(embed: Bool) {
        guard let store, let id = selectedNote, let dir = ImportSheet.pickFolder() else { return }
        flushEdits()
        do {
            alert = L("Saved as %@.", URL(fileURLWithPath: try store.exportHtml(id: id, dir: dir.path, embedAttachments: embed)).lastPathComponent)
        } catch {
            alert = describe(error)
        }
    }

    /// R61: the open note as a PDF of this paper.
    func exportPdf(paper: NSSize) {
        guard let store, let id = selectedNote, let note = openNote else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = note.shownTitle + ".pdf"
        guard panel.runModal() == .OK, let target = panel.url else { return }
        savePdf(of: id, to: target, paper: paper, store: store)
    }

    func savePdf(of id: String, to target: URL, paper: NSSize, store: Store, done: @escaping () -> Void = {}) {
        flushEdits()
        do {
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
            let html = try store.exportHtml(id: id, dir: dir.path, embedAttachments: true)
            PDFExport.run(html: URL(fileURLWithPath: html), to: target, paper: paper) { [weak self] error in
                try? FileManager.default.removeItem(at: dir)
                if let error { self?.alert = describe(error) }
                done()
            }
        } catch {
            alert = describe(error)
            done()
        }
    }

    /// E3, E40: what the licenses of the syntaxes and the fonts ask to show.
    func showAbout() {
        let fonts = ["iawriter", "inter", "jetbrainsmono", "literata"].compactMap { name in
            Bundle.main.url(forResource: "OFL-\(name)", withExtension: "txt", subdirectory: "Fonts").flatMap { try? String(contentsOf: $0, encoding: .utf8) }
        }
        let text = ([L("Fonts")] + fonts + [L("Syntaxes of code"), codeHighlightAcknowledgements()]).joined(separator: "\n\n")
        NSApp.orderFrontStandardAboutPanel(options: [
            .credits: NSAttributedString(string: text, attributes: [.font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize), .foregroundColor: NSColor.labelColor]),
        ])
    }

    // MARK: Settings

    func saveSettings(_ new: AppSettings) {
        perform { try $0.setSettings(settings: new) }
        typographyRevision += 1
    }

    func applyTheme() {
        NSApp.appearance = theme == "dark" ? NSAppearance(named: .darkAqua) : theme == "light" ? NSAppearance(named: .aqua) : nil
    }

    // MARK: Links between notes

    func linkState(_ id: String) -> NoteLinkState {
        (try? store?.linkTargets(ids: [id]))?.first?.state ?? .gone
    }

    func titlesMatching(_ query: String) -> [Note] {
        ((try? store?.notesMatchingTitle(query: query, limit: 8)) ?? []).filter { $0.id != selectedNote }
    }

    func linkedFrom(_ id: String) -> [Note] { (try? store?.linkedFrom(id: id)) ?? [] }

    /// R53: the selection moves into a new note, and a link stays in its place.
    func noteFromSelection() {
        guard let id = selectedNote, let range = activeEditor?.selectedOffsets else { return }
        flushEdits()
        perform(named: L("New Note from Selection")) { _ = try $0.noteFromSelection(id: id, start: range.start, end: range.end) }
    }

    // MARK: Version history (R40–R43)

    /// Whether the panel of versions of the open note is on show.
    var historyOpen = false

    func versions(of id: String) -> [Version] {
        flushEdits()
        return (try? store?.versions(note: id)) ?? []
    }

    func versionDiff(_ id: String, _ version: String) -> VersionDiff? {
        try? store?.versionDiff(note: id, version: version)
    }

    /// R42: an ordinary edit, one action of undo; the later versions stay.
    func restoreVersion(_ id: String, _ version: String) {
        perform(named: L("Restore Version")) { try $0.restoreVersion(note: id, version: version) }
    }

    // MARK: Search (R47)

    var search = "" { didSet { if search != oldValue { reloadSearch() } } }
    var searchArchived = false { didSet { reloadSearch() } }
    var searchTrash = false { didSet { reloadSearch() } }
    var hits: [SearchHit] = []

    var isSearching: Bool { !search.trimmingCharacters(in: .whitespaces).isEmpty }

    func reloadSearch() {
        hits = isSearching ? ((try? store?.search(query: search, includeArchived: searchArchived, includeTrash: searchTrash)) ?? []) : []
    }

    // MARK: Attachments

    /// R30: the files go into the note, each with a link at the cursor.
    func attach(_ urls: [URL], to id: String) {
        touched(id)
        for url in urls {
            guard let file = perform(named: L("Attach File"), { try $0.addAttachment(noteId: id, path: url.path, name: nil) }) else { continue }
            activeEditor?.insert(link: AttachmentLink.text(file))
        }
        reloadAttachments()
    }

    func reloadAttachments() {
        attachments = selectedNote.flatMap { try? store?.attachments(noteId: $0) } ?? []
        activeEditor?.reloadAttachments()
    }

    func attachment(_ id: String) -> Attachment? { attachments.first { $0.id == id } }

    /// R33: whether the text still has the link of an attachment.
    func isInText(_ file: Attachment) -> Bool {
        selectedNote.map { text(of: $0).contains("attachment:\(file.id)") } ?? false
    }

    /// R33: deleted by hand from the panel.
    func removeAttachment(_ id: String) {
        perform(named: L("Delete Attachment")) { try $0.removeAttachment(id: id) }
        reloadAttachments()
    }

    /// R31, R32: a click shows the file; R35: one that has not arrived is fetched ahead of the queue.
    func openAttachment(_ id: String) {
        guard let file = attachment(id) else { return }
        guard file.localPath != nil else { return fetch(file) }
        if file.mime.hasPrefix("image/") || file.mime == "application/pdf" {
            AttachmentViewer.show(file)
        } else if let url = AttachmentFiles.named(file) {
            NSWorkspace.shared.open(url)
        }
    }

    private func fetch(_ file: Attachment) {
        guard let store else { return }
        let id = file.id
        Task.detached(priority: .userInitiated) {
            let result = Result { try store.fetchAttachment(id: id) }
            await MainActor.run {
                if case .failure(let error) = result { self.alert = describe(error) }
                self.reloadAttachments()
            }
        }
    }

    /// R37: a tap opens the file; "Show in note" opens the note at the place of the link.
    func showInNote(_ note: String, then file: String, open openFile: Bool) {
        if openFile, let found = (try? store?.attachments(noteId: note))?.first(where: { $0.id == file }) {
            if let url = AttachmentFiles.named(found) { NSWorkspace.shared.open(url) }
            return
        }
        filesOpen = false
        scope = .all
        open(note)
        DispatchQueue.main.async { self.activeEditor?.reveal("attachment:\(file)") }
    }

    /// R37: every attachment of the notes outside the Trash, the newest first.
    func allFiles() -> [(file: Attachment, note: Note)] {
        guard let store else { return [] }
        let notes = ((try? store.notes(scope: .all)) ?? []) + ((try? store.notes(scope: .archived)) ?? [])
        return notes.filter { $0.attachments > 0 }
            .flatMap { note in ((try? store.attachments(noteId: note.id)) ?? []).map { (file: $0, note: note) } }
            .sorted { $0.note.editedMs > $1.note.editedMs }
    }

    // MARK: Reading

    var folderOfScope: Folder? {
        guard case .folder(let id) = scope else { return nil }
        return folders.first { $0.id == id }
    }

    /// R16: how the open view shows its notes: a folder by its own settings, a system view by this device's.
    var display: ViewDisplay {
        if let folder = folderOfScope {
            return ViewDisplay(sort: folder.sort, showPreview: folder.showPreview, showDate: folder.showDate)
        }
        return viewDisplay[ViewDisplay.key(scope)] ?? ViewDisplay()
    }

    func setDisplay(_ new: ViewDisplay) {
        if let folder = folderOfScope {
            perform(named: L("Change Display")) { store in
                try store.setFolderSort(id: folder.id, sort: new.sort)
                try store.setFolderDisplay(id: folder.id, showPreview: new.showPreview, showDate: new.showDate)
            }
        } else {
            viewDisplay[ViewDisplay.key(scope)] = new
            reloadNotes()
        }
    }

    func children(of parent: String?) -> [Folder] { folders.filter { $0.parentId == parent } }

    func folder(_ id: String?) -> Folder? { id.flatMap { id in folders.first { $0.id == id } } }

    /// The folders from the root to this one, for its full name.
    func path(of id: String) -> [Folder] {
        var out: [Folder] = []
        var next: String? = id
        while let id = next, let folder = folder(id), !out.contains(where: { $0.id == id }) {
            out.insert(folder, at: 0)
            next = folder.parentId
        }
        return out
    }

    /// R46: the days left before a note in the Trash is deleted for good.
    func daysLeft(_ note: Note) -> Int {
        guard let deleted = note.deletedMs else { return Int(settings.trashDays) }
        let gone = Double(Int64(Date().timeIntervalSince1970 * 1000) - Int64(deleted)) / 86_400_000
        return max(0, Int(settings.trashDays) - Int(gone.rounded(.down)))
    }

    // MARK: Notes

    /// R2: a new note in the open folder, or where `new_note_folder` says; R17: in a folder of daily
    /// mode, today's note is opened instead when there is one.
    func newNote() {
        guard let store else { return }
        let folderId: String? = {
            if let folder = folderOfScope { return folder.id }
            return (try? store.defaultFolderId(lastUsed: Storage.defaults.string(forKey: "lastFolder"))) ?? nil
        }()
        if let daily = folder(folderId), daily.daily, let today = todaysNote(in: daily.id) {
            open(today.id)
            return
        }
        createNote(in: folderId, daily: folder(folderId)?.daily ?? false)
    }

    /// R17: "New entry for today" makes a second note of the day.
    func newEntryForToday() {
        guard let folder = folderOfScope else { return newNote() }
        createNote(in: folder.id, daily: folder.daily)
    }

    private func createNote(in folderId: String?, daily: Bool) {
        let title = daily ? Date().formatted(Date.FormatStyle().day().month(.wide).year()) : ""
        guard let note = perform(named: L("New Note"), { try $0.createNote(new: NewNote(title: title, folderId: folderId, tags: [], text: "")) }) else { return }
        if let folderId { Storage.defaults.set(folderId, forKey: "lastFolder") }
        freshNote = daily ? nil : note.id
        titleFocusAsked = !daily
        if !notes.contains(where: { $0.id == note.id }) { scope = folderId.map { .folder(id: $0) } ?? .all }
        open(note.id)
    }

    /// R17: the note of the folder made today, if there is one.
    func todaysNote(in folderId: String) -> Note? {
        let notes = (try? store?.notes(scope: .folder(id: folderId))) ?? []
        return notes.filter { Calendar.current.isDateInToday(Date(timeIntervalSince1970: Double($0.createdMs) / 1000)) }
            .max { $0.createdMs < $1.createdMs }
    }

    func open(_ id: String) {
        selectedNote = id
    }

    /// The note was edited: it is no longer one to drop (R3).
    func touched(_ id: String) {
        if freshNote == id { freshNote = nil }
    }

    /// R3: an empty note left without any edit is deleted without going to the Trash.
    private func dropIfUntouched(_ id: String) {
        guard freshNote == id, let store else { return }
        freshNote = nil
        guard let note = try? store.getNote(id: id), note.title.isEmpty, (try? store.getText(id: id))?.isEmpty ?? false else { return }
        try? store.purgeNote(id: id)
        reload()
    }

    func duplicate(_ id: String) {
        if let copy = perform(named: L("Duplicate"), { try $0.duplicateNote(id: id) }) { open(copy.id) }
    }

    func moveNotes(_ ids: [String], to folderId: String?) {
        perform(named: L("Move")) { store in for id in ids { try store.moveNote(id: id, folderId: folderId) } }
    }

    func setFlag(_ flag: NoteFlag, _ on: Bool, ids: [String]) {
        perform(named: flag.actionName) { store in
            for id in ids {
                switch flag {
                case .pinned: try store.setPinned(id: id, pinned: on)
                case .starred: try store.setStarred(id: id, starred: on)
                case .archived: try store.setArchived(id: id, archived: on)
                case .locked: try store.setLocked(id: id, locked: on)
                }
            }
        }
    }

    func trash(_ ids: [String]) {
        if let selected = selectedNote, ids.contains(selected) { selectedNote = nil }
        perform(named: L("Delete")) { store in for id in ids { try store.deleteNote(id: id) } }
    }

    func restore(_ ids: [String]) {
        perform(named: L("Restore")) { store in for id in ids { try store.restoreNote(id: id) } }
    }

    /// R46: for good, with no undo: the core keeps nothing to bring back.
    func purge(_ ids: [String]) {
        guard let store else { return }
        if let selected = selectedNote, ids.contains(selected) { selectedNote = nil }
        do { for id in ids { try store.purgeNote(id: id) } } catch { alert = describe(error) }
        reload()
        scheduleSync()
    }

    func emptyTrash() {
        guard let store else { return }
        selectedNote = nil
        do { _ = try store.emptyTrash() } catch { alert = describe(error) }
        reload()
        scheduleSync()
    }

    // MARK: Folders

    func createFolder(named name: String, in parent: String?) {
        if let folder = perform(named: L("New Folder"), { try $0.createFolder(name: name, parentId: parent) }) {
            if let parent { foldedFolders.remove(parent) }
            scope = .folder(id: folder.id)
        }
    }

    func renameFolder(_ id: String, to name: String) {
        perform(named: L("Rename Folder")) { try $0.renameFolder(id: id, name: name) }
    }

    func setFolderIcon(_ id: String, _ icon: String) {
        perform(named: L("Change Icon")) { try $0.setFolderIcon(id: id, icon: icon) }
    }

    func setDaily(_ id: String, _ on: Bool) {
        perform(named: L("Daily Mode")) { try $0.setFolderDaily(id: id, daily: on) }
    }

    /// R11: a folder into another, or to the root; never into itself or below itself.
    func moveFolder(_ id: String, into parent: String?) {
        if let parent, path(of: parent).contains(where: { $0.id == id }) { return }
        perform(named: L("Move Folder")) { try $0.moveFolder(id: id, parentId: parent, after: nil) }
    }

    /// R14: how many notes a deletion takes to the Trash, for its confirmation.
    func deletionCount(_ id: String) -> Int { Int((try? store?.folderDeletionCount(id: id)) ?? 0) }

    func deleteFolder(_ id: String) {
        perform(named: L("Delete Folder")) { _ = try $0.deleteFolder(id: id) }
    }

    // MARK: Tags and filters

    func renameTag(_ old: String, to new: String) {
        perform(named: L("Rename Tag")) { _ = try $0.renameTag(old: old, new: new) }
        if case .tag(let name) = scope, name.lowercased() == old.lowercased() { scope = .tag(name: new) }
    }

    func deleteTag(_ name: String) {
        perform(named: L("Delete Tag")) { _ = try $0.deleteTag(name: name) }
    }

    func saveFilter(id: String?, name: String, icon: String, query: FilterQuery) {
        if let id {
            perform(named: L("Edit Filter")) { try $0.updateFilter(id: id, name: name, icon: icon, query: query) }
        } else if let made = perform(named: L("New Filter"), { try $0.createFilter(name: name, icon: icon, query: query) }) {
            scope = .filter(id: made.id)
        }
    }

    func deleteFilter(_ id: String) {
        perform(named: L("Delete Filter")) { try $0.deleteFilter(id: id) }
    }

    /// Runs a change as one action of undo (R75): the core keeps the history, and the undo manager of
    /// the window gets a step that asks the core to take it back. A change that fails is shown.
    @discardableResult
    func perform<T>(named name: String? = nil, _ change: (Store) throws -> T) -> T? {
        guard let store else { return nil }
        let before = store.undoRevision()
        store.beginGroup()
        defer {
            store.endGroup()
            if store.undoRevision() != before {
                undoManager?.registerUndo(withTarget: self) { model in
                    model.perform { _ = try $0.undo() }
                }
                if let name { undoManager?.setActionName(name) }
            }
            reload()
            scheduleSync()
        }
        do {
            return try change(store)
        } catch {
            alert = describe(error)
            return nil
        }
    }

    // MARK: Sync

    /// R81: what the sidebar says about sync.
    var syncLine: String? {
        if syncing { return L("Syncing…") }
        if let error = syncStatus.lastError { return L("The last attempt failed: %@", error) }
        if syncStatus.attachmentsWaiting > 0 { return L("Attachments waiting to sync: %@", "\(syncStatus.attachmentsWaiting)") }
        return syncStatus.configured ? L("Synced") : nil
    }

    /// A change syncs shortly after it is made; changes made in a row go together.
    func scheduleSync() {
        syncDebounce?.cancel()
        let work = DispatchWorkItem { [weak self] in MainActor.assumeIsolated { self?.syncNow() } }
        syncDebounce = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: work)
    }

    /// S39: a sync starts when the log folder of a storage on this Mac changes.
    func watchSyncFolder() {
        let folder: URL? = {
            guard case .folder(let path)? = try? store?.syncConfig() else { return nil }
            return URL(fileURLWithPath: path).appendingPathComponent("notes/log", isDirectory: true)
        }()
        folderWatcher.watch(folder) { [weak self] in MainActor.assumeIsolated { self?.syncNow() } }
    }

    func syncNow() {
        flushEdits()
        guard let store, !syncing, syncStatus.configured || (try? store.syncConfig()) != .off else { return }
        syncing = true
        Task.detached(priority: .utility) {
            let result = Result { try store.syncNow() }
            await MainActor.run {
                self.syncing = false
                if case .success(let report) = result, report.pulled > 0 || report.blobsDownloaded > 0 {
                    self.reload()
                } else {
                    self.syncStatus = (try? store.syncStatus()) ?? self.syncStatus
                }
                if case .success = result { self.syncAttachments() }
            }
        }
    }

    /// S24: the content of attachments goes after the registers, in the background.
    private func syncAttachments() {
        guard let store else { return }
        Task.detached(priority: .utility) {
            let report = try? store.syncAttachments()
            await MainActor.run {
                if (report?.downloaded ?? 0) > 0 { self.reload() }
                self.syncStatus = (try? store.syncStatus()) ?? self.syncStatus
            }
        }
    }

    /// Another device that uploaded a change asks this one to sync. The call blocks on a thread of
    /// its own until that happens, and paces itself while no push server is set.
    private func listenForNudges() {
        guard let store else { return }
        Thread.detachNewThread { [weak self] in
            while true {
                if store.waitForNudge() {
                    DispatchQueue.main.async { MainActor.assumeIsolated { self?.syncNow() } }
                }
            }
        }
    }

    /// R70: a backup at the first opportunity after its time.
    private func backupIfDue() {
        guard let store else { return }
        Task.detached(priority: .background) { _ = try? store.backupIfDue() }
    }
}
