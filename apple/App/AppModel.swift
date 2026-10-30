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
