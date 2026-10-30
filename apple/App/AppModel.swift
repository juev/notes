import AppKit
import Observation

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
    /// The note open in the editor.
    var selectedNote: String? {
        didSet { if selectedNote != oldValue, let left = oldValue { dropIfUntouched(left) } }
    }
    /// R3: a note made here and not edited yet; it is deleted without going to the Trash when left so.
    @ObservationIgnored private var freshNote: String?

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
