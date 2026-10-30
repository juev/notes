import SwiftUI

/// The settings of sync (R80–R82): the storage, push and the encryption of the storage. As in Lists.
struct SyncSettings: View {
    @Environment(AppModel.self) private var model

    enum Kind { case off, webdav, folder }
    enum Sealing: Identifiable {
        case encrypt, change
        var id: Self { self }
    }
    private struct Replacement {
        let config: SyncConfig
        let side: SyncSide
    }

    @State private var kind = Kind.off
    @State private var url = ""
    @State private var user = ""
    @State private var password = ""
    @State private var folder = ""
    @State private var pushServer = ""
    @State private var pushToken = ""
    @State private var pushSignIn = false
    @State private var pushRefused = false
    @State private var message: String?
    @State private var testing = false
    /// The storage being joined while the person chooses a side (S36).
    @State private var joining: SyncConfig?
    @State private var replacing: Replacement?
    /// The passphrase of an encrypted storage, and the window that sets a new one.
    @State private var passphrase = ""
    @State private var sealing: Sealing?
    @State private var sealingBusy: Sealing?

    var body: some View {
        Form {
            Section(L("Sync")) {
                    Picker(L("Storage"), selection: $kind) {
                        Text(L("Off")).tag(Kind.off)
                        Text("WebDAV").tag(Kind.webdav)
                        Text(L("Folder")).tag(Kind.folder)
                    }
                    switch kind {
                    case .off:
                        Text(L("Data is kept on this Mac only.")).foregroundStyle(.secondary)
                    case .webdav:
                        TextField(L("Address"), text: $url, prompt: Text("https://example.org/remote.php/dav/files/me"))
                        TextField(L("User name"), text: $user)
                        SecureField(L("Password"), text: $password)
                        Text(L("For Nextcloud and similar servers use an app password."))
                            .font(.caption).foregroundStyle(.secondary)
                        passphraseField
                    case .folder:
                        HStack {
                            TextField(L("Path"), text: $folder)
                            Button(L("Choose…"), action: chooseFolder)
                        }
                        Text(L("A folder synced by another tool or a network drive will do."))
                            .font(.caption).foregroundStyle(.secondary)
                        passphraseField
                    }
                    if kind != .off {
                        TextField(L("Push server"), text: $pushServer, prompt: Text("https://ntfy.sh"))
                        Text(L("Optional. Through an ntfy server other devices ask this Mac to sync at once. No data passes through it."))
                            .font(.caption).foregroundStyle(.secondary)
                        Toggle(L("The push server requires sign-in"), isOn: $pushSignIn)
                        if pushSignIn {
                            SecureField(L("Push token"), text: $pushToken, prompt: Text("tk_…"))
                            Text(L("An access token of the ntfy server. It is sent to this server and to no other."))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        if pushRefused {
                            Text(L("The push server refused access. Check the push token."))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    HStack {
                        Button(L("Save and sync"), action: save)
                        if kind == .webdav {
                            Button(L("Test connection"), action: test).disabled(testing)
                        }
                        if model.syncing || testing { ProgressView().controlSize(.small) }
                        Spacer()
                    }
                    // S29: the storage that is saved, not the one being typed, is encrypted.
                    if savedFiles {
                        HStack {
                            Button(model.syncStatus.encrypted ? L("Change passphrase…") : L("Encrypt the storage…")) {
                                sealing = model.syncStatus.encrypted ? .change : .encrypt
                            }
                            .disabled(sealingBusy != nil)
                            if let busy = sealingBusy {
                                ProgressView().controlSize(.small)
                                Text(busy == .encrypt ? L("Encrypting the storage…") : L("Changing the passphrase…"))
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                        }
                    }
                    if let text = message ?? status {
                        Text(text).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                }
        }
        .formStyle(.grouped)
        .onAppear(perform: load)
        .sheet(item: $sealing) { what in
            PassphraseSheet(what: what) { chosen in seal(what, with: chosen) }
        }
        .confirmationDialog(
            L("This Mac and the storage both hold notes"),
            isPresented: Binding(get: { joining != nil }, set: { if !$0 { joining = nil } }),
            titleVisibility: .visible,
            presenting: joining
        ) { config in
            // The buttons carry the storage themselves: the dialog forgets it as it closes.
            Button(L("Merge")) { apply(config, side: .merge) }
            Button(L("Use the data of the storage…")) { replacing = Replacement(config: config, side: .storage) }
            Button(L("Use the data of this Mac…")) { replacing = Replacement(config: config, side: .device) }
            Button(L("Cancel"), role: .cancel) {}
        } message: { _ in
            Text(L("Merge keeps the notes of both sides. The other two choices keep one side and replace the other."))
        }
        .alert(
            replacing?.side == .storage ? L("Replace the data of this Mac?") : L("Replace the data of the storage?"),
            isPresented: Binding(get: { replacing != nil }, set: { if !$0 { replacing = nil } }),
            presenting: replacing
        ) { choice in
            Button(L("Replace"), role: .destructive) { apply(choice.config, side: choice.side) }
            Button(L("Cancel"), role: .cancel) {}
        } message: { choice in
            Text(choice.side == .storage
                ? L("The notes, folders and filters of this Mac are erased and read again from the storage. Changes that have not been synced are lost.")
                : L("The storage and every other device get the data of this Mac. What this Mac does not have goes to the Trash on all devices, and edits made elsewhere are undone."))
        }
        // The subscription is retried in the background, so the answer comes later than the save.
        .task {
            while !Task.isCancelled {
                pushRefused = model.store?.pushRefused() ?? false
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    @ViewBuilder private var passphraseField: some View {
        SecureField(L("Storage passphrase"), text: $passphrase)
        Text(L("Only for a storage encrypted with a passphrase."))
            .font(.caption).foregroundStyle(.secondary)
    }

    /// Whether the saved settings are a storage that can be encrypted (S29).
    private var savedFiles: Bool {
        switch try? model.store?.syncConfig() {
        case .webDav?, .folder?: return true
        default: return false
        }
    }

    /// S29: encrypts the saved storage or gives it a new passphrase, off the main thread;
    /// the passphrase goes where the password goes once the core has taken it.
    private func seal(_ what: Sealing, with chosen: String) {
        guard let store = model.store, let config = try? store.syncConfig(),
              let account = Keychain.passphraseAccount(config) else { return }
        message = nil
        sealingBusy = what
        Task {
            let done = await Task.detached {
                Result {
                    switch what {
                    case .encrypt: try store.encryptStorage(passphrase: chosen)
                    case .change: try store.changeStoragePassphrase(passphrase: chosen)
                    }
                }
            }.value
            sealingBusy = nil
            switch done {
            case .success:
                passphrase = chosen
                do {
                    try Keychain.save(chosen, account: account)
                } catch {
                    message = L("The passphrase could not be saved in the system keychain: %@", error.localizedDescription)
                    store.log(level: .error, source: "sync", message: "the passphrase was not saved in the keychain: \(logged(error))")
                }
                model.reload()
                model.syncNow()
            case .failure(let error):
                message = describe(error)
            }
        }
    }

    private var status: String? {
        let s = model.syncStatus
        if let error = s.lastError { return L("The last attempt failed: %@", "\(error)") }
        let waiting = s.attachmentsWaiting > 0 ? L("Attachments waiting to sync: %@", "\(s.attachmentsWaiting)") : nil
        if let ok = s.lastOk { return [L("Synced: %@", "\(Stamp.label(ok))"), waiting].compactMap { $0 }.joined(separator: "\n") }
        return waiting
    }

    private func load() {
        pushServer = (try? model.store?.pushServer()) ?? ""
        pushToken = pushServer.isEmpty ? "" : Keychain.load(account: Keychain.pushAccount(server: pushServer)) ?? ""
        // Off until a token is set: a public ntfy server needs none.
        pushSignIn = !pushToken.isEmpty
        guard let config = try? model.store?.syncConfig() else { return }
        switch config {
        case .off: kind = .off
        case .folder(let path): kind = .folder; folder = path
        case .webDav(let u, let name):
            kind = .webdav
            url = u
            user = name
            password = Keychain.load(account: Keychain.account(url: u, user: name)) ?? ""
        }
        passphrase = Keychain.passphraseAccount(config).flatMap { Keychain.load(account: $0) } ?? ""
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        if panel.runModal() == .OK, let picked = panel.url { folder = picked.path }
    }

    /// Asks the server with what the fields hold now; nothing is saved (S26).
    private func test() {
        let config: SyncConfig = .webDav(url: url, user: user)
        let password = password
        message = nil
        testing = true
        Task {
            let result = await Task.detached { Result { try checkSyncConnection(config: config, password: password) } }.value
            testing = false
            switch result {
            case .success(.ready): message = L("Connected.")
            case .success(.willCreate): message = L("Connected. The folder does not exist yet; the first sync creates it.")
            case .failure(let error): message = L("No connection: %@", describe(error))
            }
        }
    }

    private func save() {
        let config: SyncConfig
        switch kind {
        case .off: config = .off
        case .webdav: config = .webDav(url: url, user: user)
        case .folder: config = .folder(path: folder)
        }
        message = nil
        guard let store = model.store else { return }
        // S36: a storage that is new to this Mac may hold data of its own.
        guard config != .off, (try? store.syncConfig()) != config else { return apply(config, side: .merge) }
        let password = password
        testing = true
        Task {
            let both = await Task.detached { Result { try store.syncConflict(config: config, password: password) } }.value
            testing = false
            switch both {
            case .success(true): joining = config
            case .success(false): apply(config, side: .merge)
            case .failure(let error): message = L("No connection: %@", describe(error))
            }
        }
    }

    private func apply(_ config: SyncConfig, side: SyncSide) {
        guard let store = model.store else { return }
        guard side != .merge else { return finish(Result { try store.joinStorage(config: config, side: side) }) }
        // Replacing a side waits for a run that is under way: not on the main thread.
        testing = true
        Task {
            let joined = await Task.detached { Result { try store.joinStorage(config: config, side: side) } }.value
            testing = false
            finish(joined)
        }
    }

    /// The rest of saving, once the storage is stored.
    private func finish(_ joined: Result<Void, Error>) {
        guard let store = model.store else { return }
        do {
            try joined.get()
            try store.setPushServer(server: kind == .off ? nil : pushServer)
            // The core keeps the address without the trailing slash; the token is filed under that.
            if let server = try store.pushServer() {
                let token = pushSignIn ? pushToken.trimmingCharacters(in: .whitespacesAndNewlines) : ""
                let account = Keychain.pushAccount(server: server)
                if token.isEmpty {
                    Keychain.delete(account: account)
                } else {
                    do {
                        try Keychain.save(token, account: account)
                    } catch {
                        message = L("The push token could not be saved in the system keychain: %@", error.localizedDescription)
                        store.log(level: .error, source: "push", message: "the token was not saved in the keychain: \(logged(error))")
                    }
                }
                store.setPushToken(token: token)
            } else {
                store.setPushToken(token: nil)
            }
            pushRefused = false
            if kind == .webdav {
                do {
                    try Keychain.save(password, account: Keychain.account(url: url, user: user))
                } catch {
                    // Sync still works until the app quits; say why it will ask again.
                    message = L("The password could not be saved in the system keychain: %@", error.localizedDescription)
                    store.log(level: .error, source: "sync", message: "the password was not saved in the keychain: \(logged(error))")
                }
                store.setSyncPassword(password: password)
            }
            // S58: kept like the password, one per storage.
            if let account = Keychain.passphraseAccount(try store.syncConfig()) {
                if passphrase.isEmpty {
                    Keychain.delete(account: account)
                } else {
                    do {
                        try Keychain.save(passphrase, account: account)
                    } catch {
                        message = L("The passphrase could not be saved in the system keychain: %@", error.localizedDescription)
                        store.log(level: .error, source: "sync", message: "the passphrase was not saved in the keychain: \(logged(error))")
                    }
                }
                store.setSyncPassphrase(passphrase: passphrase)
            }
            model.reload()
            model.watchSyncFolder()
            model.syncNow()
        } catch {
            message = describe(error)
        }
    }
}

/// A new passphrase, typed twice, with the warning that a forgotten one loses the storage.
struct PassphraseSheet: View {
    let what: SyncSettings.Sealing
    let done: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var first = ""
    @State private var second = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(what == .encrypt ? L("Encrypt the storage") : L("Change passphrase")).font(.headline)
            Text(what == .encrypt
                ? L("This Mac syncs first, then writes the storage again, encrypted. This takes as long as downloading the attachments it does not have yet. Other devices ask for the passphrase.")
                : L("Other devices ask for the new passphrase the next time they start."))
                .fixedSize(horizontal: false, vertical: true)
            SecureField(L("Passphrase"), text: $first)
            SecureField(L("Passphrase again"), text: $second)
            Text(L("A forgotten passphrase cannot be recovered, and the data in the storage is lost with it. The copies on your devices stay."))
                .font(.caption).foregroundStyle(.orange)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button(L("Cancel"), role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                Button(what == .encrypt ? L("Encrypt") : L("Change")) {
                    done(first)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(first.isEmpty || first != second)
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}
