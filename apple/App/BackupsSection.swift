import AppKit
import SwiftUI

/// Where a backup to restore comes from.
private enum BackupSource: Sendable {
    case file(String)
    case storage(device: String, name: String)
}

extension StorageBackup {
    /// Where the copy lies in the storage, unique among all of them.
    var path: String { "\(device)/\(name)" }
}

/// The backups of this Mac in the settings (R70–R72): the schedule, the number to keep, the backups
/// themselves and restoring from one of them or from a file. With WebDAV or a folder, the copies in
/// the sync storage as well (S32). As in Lists.
struct BackupsSection: View {
    @Environment(AppModel.self) private var model

    /// What is to be restored: a backup of the list or a file picked outside the app,
    /// or a copy in the sync storage.
    private struct Restore: Sendable {
        let source: BackupSource
        let label: String
    }

    @State private var settings = BackupSettings(everyHours: 24, keep: 10)
    @State private var backups: [Backup] = []
    @State private var restoring: Restore?
    @State private var busy = false
    @State private var message: String?
    @State private var storage: StorageBackupStatus?
    /// `nil` while the storage is being asked.
    @State private var stored: [StorageBackup]?
    @State private var storageMessage: String?

    private static let sizes: ByteCountFormatter = {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter
    }()

    private var synced: Bool {
        guard let config = try? model.store?.syncConfig() else { return false }
        return config != .off
    }

    var body: some View {
        Group {
            local
            if let storage, storage.available { inStorage(storage) }
        }
        .onAppear(perform: load)
        .confirmationDialog(
            L("Replace everything with the backup?"),
            isPresented: Binding(get: { restoring != nil }, set: { if !$0 { restoring = nil } }),
            titleVisibility: .visible,
            presenting: restoring
        ) { what in
            // S38: with sync on, what the storage holds has to be settled in the same step.
            if synced {
                Button(L("Restore and merge with the storage"), role: .destructive) { restore(what, overStorage: false) }
                Button(L("Restore and replace the data of the storage"), role: .destructive) { restore(what, overStorage: true) }
            } else {
                Button(L("Restore"), role: .destructive) { restore(what, overStorage: false) }
            }
            Button(L("Cancel"), role: .cancel) {}
        } message: { what in
            Text(L("All notes, folders and filters on this Mac are replaced with what the backup %@ holds. The present state is saved as a backup first.", what.label))
        }
    }

    /// S32: the switch, the last copy or why it failed, and the copies of every device.
    private func inStorage(_ status: StorageBackupStatus) -> some View {
        Section(L("In the Sync Storage")) {
            Toggle(L("Keep copies in the sync storage"), isOn: Binding(
                get: { status.enabled },
                set: { on in
                    do {
                        try model.store?.setStorageBackups(enabled: on)
                    } catch {
                        storageMessage = describe(error)
                    }
                    load()
                }))
            Text(L("A copy in the storage holds the notes and the folders; the attachments are in the storage already. It is made on the schedule above, with the next sync."))
                .font(.caption).foregroundStyle(.secondary)
            if status.enabled {
                if let error = status.error {
                    Text(L("The last copy did not reach the storage: %@", error))
                        .font(.caption).foregroundStyle(.red).textSelection(.enabled)
                } else if let last = status.last {
                    Text(L("Last copy in the storage: %@", Stamp.label(last))).font(.caption)
                } else {
                    Text(L("No copy in the storage yet.")).font(.caption).foregroundStyle(.secondary)
                }
            }
            if let storageMessage {
                Text(storageMessage).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
            if let stored {
                if stored.isEmpty {
                    Text(L("No copies in the storage.")).foregroundStyle(.secondary)
                }
                // Two devices may name a copy alike: the folder tells them apart.
                ForEach(stored, id: \.path) { copy in
                    HStack {
                        Text(Stamp.label(copy.created))
                        Text(copy.thisDevice ? L("this Mac") : L("another device")).foregroundStyle(.secondary)
                        Spacer()
                        Button(L("Restore…")) {
                            restoring = Restore(source: .storage(device: copy.device, name: copy.name), label: Stamp.label(copy.created))
                        }
                        .disabled(busy)
                    }
                }
            } else {
                ProgressView().controlSize(.small)
            }
        }
    }

    private var local: some View {
        Section(L("Backups")) {
            Picker(L("Create backups"), selection: Binding(
                get: { settings.everyHours },
                set: { save(BackupSettings(everyHours: $0, keep: settings.keep)) })) {
                Text(L("Do not create")).tag(UInt32(0))
                Text(L("Daily")).tag(UInt32(24))
                Text(L("Weekly")).tag(UInt32(168))
            }
            Picker(L("Keep at most"), selection: Binding(
                get: { settings.keep },
                set: { save(BackupSettings(everyHours: settings.everyHours, keep: $0)) })) {
                ForEach([UInt32(5), 10, 20, 30], id: \.self) { Text("\($0)").tag($0) }
            }
            Text(L("A backup holds the notes, the folders and the attachments that are on this Mac. Backups are kept in the data folder of the app: save a copy elsewhere from time to time."))
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(L("Create Backup Now")) { run { _ = try $0.createBackup() } }
                Button(L("Restore from File…"), action: chooseFile)
                if busy { ProgressView().controlSize(.small) }
                Spacer()
            }
            .disabled(busy)
            if let message {
                Text(message).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
            if backups.isEmpty {
                Text(L("No backups yet.")).foregroundStyle(.secondary)
            }
            ForEach(backups, id: \.name) { backup in
                HStack {
                    Text(Stamp.label(backup.created))
                    Spacer()
                    Text(Self.sizes.string(fromByteCount: Int64(backup.size))).foregroundStyle(.secondary)
                    Menu {
                        Button(L("Restore…")) { restoring = Restore(source: .file(backup.path), label: Stamp.label(backup.created)) }
                        Button(L("Save a Copy…")) { saveCopy(backup) }
                        Button(L("Delete"), role: .destructive) { run { try $0.deleteBackup(name: backup.name) } }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                    .disabled(busy)
                    .accessibilityLabel(L("Actions for the backup"))
                }
            }
        }
    }

    private func load() {
        guard let store = model.store else { return }
        settings = (try? store.backupSettings()) ?? settings
        backups = (try? store.backups()) ?? []
        storage = try? store.storageBackupStatus()
        guard storage?.available == true else { return }
        // The list asks the storage, which may be slow or away.
        Task {
            let result = await Task.detached { Result { try store.storageBackups() } }.value
            switch result {
            case .success(let found): stored = found
            case .failure(let error):
                stored = []
                storageMessage = describe(error)
            }
        }
    }

    private func save(_ changed: BackupSettings) {
        do {
            try model.store?.setBackupSettings(settings: changed)
            settings = changed
        } catch {
            message = describe(error)
        }
    }

    /// Runs `work` off the main thread, then shows the backups as they are and `done` or the error.
    private func run(done: String? = nil, then: @escaping () -> Void = {}, _ work: @escaping @Sendable (Store) throws -> Void) {
        guard let store = model.store else { return }
        busy = true
        Task {
            let result = await Task.detached { Result { try work(store) } }.value
            busy = false
            switch result {
            case .success:
                message = done
                then()
            case .failure(let error): message = describe(error)
            }
            load()
        }
    }

    private func restore(_ what: Restore, overStorage: Bool) {
        let source = what.source
        run(done: L("Restored from %@.", what.label), then: {
            // The window and the next sync start from the restored data.
            model.reload()
            model.syncNow()
        }) { store in
            switch source {
            case .file(let path): try store.restoreBackup(path: path, overStorage: overStorage)
            case .storage(let device, let name):
                try store.restoreStorageBackup(device: device, name: name, overStorage: overStorage)
            }
        }
    }

    private func chooseFile() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let picked = panel.url else { return }
        restoring = Restore(source: .file(picked.path), label: picked.lastPathComponent)
    }

    private func saveCopy(_ backup: Backup) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = backup.name
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let target = panel.url else { return }
        let source = URL(fileURLWithPath: backup.path)
        run(done: L("The copy is saved.")) { _ in
            // The panel has asked about replacing a file that is there.
            try? FileManager.default.removeItem(at: target)
            try FileManager.default.copyItem(at: source, to: target)
        }
    }
}
