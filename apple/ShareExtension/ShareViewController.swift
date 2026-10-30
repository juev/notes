import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// What the host app handed over, reduced to what a note can hold (R85).
struct SharedContent {
    var title = ""
    var notes = ""
    var files: [URL] = []
    /// R95: why an item that was shared did not arrive, for the log.
    var failures: [String] = []
}

/// Share sheet entry: shows the note about to be created and saves it straight
/// into the shared database, so it works while the main app is not running. As in Lists.
final class ShareViewController: NSViewController {
    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 420, height: 190))
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        collect { [weak self] content in
            guard let self else { return }
            // Without the group the extension would open a database of its own, which Notes never reads.
            let store = Storage.groupContainer == nil ? nil : try? Storage.openStore()
            for failure in content.failures {
                store?.log(level: .error, source: "attachments", message: "a shared item was not received: \(failure)")
            }
            let form = ShareForm(
                content: content,
                notice: Storage.groupContainer == nil ? Self.noGroup : nil,
                // R2: the form starts with the folder for new notes; the folder used last is a setting
                // of the app, which the extension does not see.
                folders: (try? store?.folders()) ?? [],
                defaultFolderId: (try? store?.defaultFolderId(lastUsed: nil)) ?? nil,
                onCancel: { [weak self] in
                    self?.extensionContext?.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
                },
                onSave: { [weak self] title, text, folderId, done in
                    guard let self else { return done(L("The extension was closed")) }
                    self.save(title: title, text: text, folderId: folderId, files: content.files, done: done)
                })
            let host = NSHostingView(rootView: form)
            host.frame = self.view.bounds
            host.autoresizingMask = [.width, .height]
            self.view.addSubview(host)
        }
    }

    private static let noGroup = L("This build of Notes cannot save from Share: it is signed without a development team.")

    /// The note and its files are written off the main thread; `done` is called on the main one.
    private func save(
        title: String,
        text: String,
        folderId: String?,
        files: [URL],
        done: @escaping (String?) -> Void
    ) {
        guard let container = Storage.groupContainer else { return done(Self.noGroup) }
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let store = try Storage.openStore()
                let note = try store.createNote(new: NewNote(title: title, folderId: folderId, tags: [], text: text))
                var links: [String] = []
                for file in files {
                    // The database and the attachments of Notes do not leave through a note.
                    if Self.isInside(file, container) {
                        store.log(level: .error, source: "attachments", message: "a shared file from the data of Notes was not attached")
                        continue
                    }
                    do {
                        links.append(AttachmentLink.text(try store.addAttachment(noteId: note.id, path: file.path, name: nil)))
                    } catch {
                        // R95: the reason, never the name of the file.
                        store.log(level: .error, source: "attachments", message: "a shared file was not attached: \(logged(error))")
                    }
                }
                // R30: each file has its link in the text.
                if !links.isEmpty {
                    try store.setText(id: note.id, text: ([text] + links).filter { !$0.isEmpty }.joined(separator: "\n\n"))
                }
                Storage.announceChange()
                DispatchQueue.main.async {
                    self.extensionContext?.completeRequest(returningItems: nil)
                    done(nil)
                }
            } catch {
                DispatchQueue.main.async { done(describe(error)) }
            }
        }
    }

    private func collect(_ done: @escaping (SharedContent) -> Void) {
        var content = SharedContent()
        let group = DispatchGroup()
        let lock = NSLock()
        let items = (extensionContext?.inputItems as? [NSExtensionItem]) ?? []
        for item in items {
            if let text = item.attributedContentText?.string, !text.isEmpty { content.notes = text }
            for provider in item.attachments ?? [] {
                if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                    group.enter()
                    provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier) { value, error in
                        if let url = Self.url(from: value) {
                            lock.withLock { content.files.append(url) }
                        } else {
                            lock.withLock { content.failures.append("file: \(error.map(logged) ?? "no address")") }
                        }
                        group.leave()
                    }
                } else if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
                    group.enter()
                    provider.loadItem(forTypeIdentifier: UTType.url.identifier) { value, _ in
                        if let url = Self.url(from: value) {
                            lock.withLock { content.notes = [content.notes, url.absoluteString].filter { !$0.isEmpty }.joined(separator: "\n") }
                        }
                        group.leave()
                    }
                } else if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) {
                    group.enter()
                    provider.loadFileRepresentation(forTypeIdentifier: UTType.image.identifier) { url, error in
                        // The file is removed when this block returns: keep a copy.
                        if let url {
                            let copy = FileManager.default.temporaryDirectory.appendingPathComponent(url.lastPathComponent)
                            try? FileManager.default.removeItem(at: copy)
                            do {
                                try FileManager.default.copyItem(at: url, to: copy)
                                lock.withLock { content.files.append(copy) }
                            } catch {
                                lock.withLock { content.failures.append("image not copied: \(logged(error))") }
                            }
                        } else {
                            lock.withLock { content.failures.append("image: \(error.map(logged) ?? "no file")") }
                        }
                        group.leave()
                    }
                } else if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
                    group.enter()
                    provider.loadItem(forTypeIdentifier: UTType.plainText.identifier) { value, _ in
                        if let text = value as? String {
                            lock.withLock { content.notes = [content.notes, text].filter { !$0.isEmpty }.joined(separator: "\n") }
                        }
                        group.leave()
                    }
                }
            }
        }
        group.notify(queue: .main) {
            // The first line becomes the title; a long text stays whole in the note.
            let firstLine = content.notes.split(separator: "\n").first.map(String.init) ?? ""
            if !firstLine.isEmpty {
                content.title = String(firstLine.prefix(120))
                if content.notes == content.title { content.notes = "" }
            } else if let file = content.files.first {
                content.title = file.deletingPathExtension().lastPathComponent
            }
            done(content)
        }
    }

    /// Whether the file lies in `folder` or under it, links followed.
    static func isInside(_ file: URL, _ folder: URL) -> Bool {
        let path = file.resolvingSymlinksInPath().standardizedFileURL.path
        let root = folder.resolvingSymlinksInPath().standardizedFileURL.path
        return path == root || path.hasPrefix(root + "/")
    }

    private static func url(from value: NSSecureCoding?) -> URL? {
        if let url = value as? URL { return url }
        if let data = value as? Data { return URL(dataRepresentation: data, relativeTo: nil) }
        if let text = value as? String { return URL(string: text) }
        return nil
    }
}

struct ShareForm: View {
    let content: SharedContent
    /// Why nothing can be saved, shown in place of an error.
    let notice: String?
    let folders: [Folder]
    let defaultFolderId: String?
    let onCancel: () -> Void
    /// Returns an error text or nil; the answer comes on the main thread.
    let onSave: (String, String, String?, @escaping (String?) -> Void) -> Void

    @State private var title = ""
    @State private var folderId: String?
    @State private var error: String?
    @State private var saving = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            TextField(L("Title"), text: $title)
                .font(.title3)
                .onSubmit(save)
            if !content.notes.isEmpty {
                Text(content.notes).font(.callout).foregroundStyle(.secondary).lineLimit(3)
            }
            if !content.files.isEmpty {
                Label(content.files.map(\.lastPathComponent).joined(separator: ", "), systemImage: "paperclip")
                    .font(.callout).foregroundStyle(.secondary).lineLimit(1)
            }
            if let error = error ?? notice { Text(error).font(.caption).foregroundStyle(.red) }
            Spacer(minLength: 0)
            HStack {
                Picker("", selection: $folderId) {
                    Text(L("No folder")).tag(String?.none)
                    ForEach(folders, id: \.id) { Text($0.name).tag(String?.some($0.id)) }
                }
                .labelsHidden()
                .fixedSize()
                Spacer()
                Button(L("Cancel"), action: onCancel).keyboardShortcut(.cancelAction)
                Button(L("Add"), action: save)
                    .keyboardShortcut(.defaultAction)
                    .disabled(notice != nil || saving || (title.trimmingCharacters(in: .whitespaces).isEmpty && content.notes.isEmpty && content.files.isEmpty))
            }
        }
        .padding(16)
        .onAppear {
            title = content.title
            folderId = defaultFolderId
        }
    }

    private func save() {
        guard notice == nil, !saving else { return }
        saving = true
        error = nil
        onSave(title, content.notes, folderId) { failure in
            saving = false
            error = failure
        }
    }
}
