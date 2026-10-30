import SwiftUI

/// What a drag between the list and the sidebar carries: `note:<id>` or `folder:<id>` (R11).
enum Dragged {
    static func note(_ id: String) -> String { "note:\(id)" }
    static func folder(_ id: String) -> String { "folder:\(id)" }
}

/// The sidebar (R44): the system views, the saved filters, the folders and the tags.
struct Sidebar: View {
    @Environment(AppModel.self) private var model
    @State private var editing: FilterDraft?
    @State private var naming: FolderNaming?
    @State private var deleting: Folder?
    @State private var renamingTag: TagRenaming?

    var body: some View {
        @Bindable var model = model
        List(selection: Binding(get: { model.filesOpen ? nil : model.scope }, set: { scope in
            model.filesOpen = false
            if let scope { model.scope = scope }
        })) {
            Section {
                row(.all, L("All notes"), "tray.full", model.counts.all)
                Button { model.filesOpen = true } label: {
                    Label(L("Files"), systemImage: "paperclip").foregroundStyle(model.filesOpen ? Color.accentColor : Color.primary)
                }
                .buttonStyle(.plain)
                row(.noFolder, L("No folder"), "tray", model.counts.noFolder)
                    .dropDestination(for: String.self) { items, _ in drop(items, into: nil) }
                row(.starred, L("Starred"), "star", model.counts.starred)
                row(.archived, L("Archived"), "archivebox", model.counts.archived)
                row(.trash, L("Trash"), "trash", model.counts.trash)
                row(.untagged, L("Untagged"), "tag.slash", model.counts.untagged)
            }
            Section(L("Filters")) {
                ForEach(model.filters, id: \.id) { filter in
                    Label { counted(filter.name, filter.count) } icon: { icon(filter.icon, "line.3.horizontal.decrease.circle") }
                        .tag(Scope.filter(id: filter.id))
                        .contextMenu {
                            Button(L("Edit…")) { editing = FilterDraft(filter) }
                            Button(L("Delete")) { model.deleteFilter(filter.id) }
                        }
                }
                Button(L("New Filter…")) { editing = FilterDraft() }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
            }
            Section(L("Folders")) {
                ForEach(model.children(of: nil), id: \.id) { folder in
                    FolderRow(folder: folder, naming: $naming, deleting: $deleting, drop: drop)
                }
                Button(L("New Folder…")) { naming = FolderNaming(parent: nil) }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
            }
            Section(L("Tags")) {
                ForEach(model.tags, id: \.name) { tag in
                    Label { counted(tag.name, tag.count) } icon: { Image(systemName: "number") }
                        .tag(Scope.tag(name: tag.name))
                        .contextMenu {
                            Button(L("Rename…")) { renamingTag = TagRenaming(old: tag.name, new: tag.name) }
                            Button(L("Delete")) { model.deleteTag(tag.name) }
                        }
                }
            }
        }
        .listStyle(.sidebar)
        // R81: the state of sync; a tap starts one.
        .safeAreaInset(edge: .bottom) {
            if let line = model.syncLine {
                Button { model.syncNow() } label: {
                    Label(line, systemImage: model.syncStatus.lastError == nil ? "arrow.triangle.2.circlepath" : "exclamationmark.triangle")
                        .lineLimit(2)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
                .padding(10)
            }
        }
        .sheet(item: $editing) { draft in FilterEditor(draft: draft) }
        .sheet(item: $naming) { naming in FolderNameSheet(naming: naming) }
        .sheet(item: $renamingTag) { renaming in TagRenameSheet(renaming: renaming) }
        .confirmationDialog(
            deleting.map { L("Delete “%@” and the %d notes in it?", $0.name, model.deletionCount($0.id)) } ?? "",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            titleVisibility: .visible
        ) {
            Button(L("Move to Trash"), role: .destructive) {
                if let deleting { model.deleteFolder(deleting.id) }
            }
        }
    }

    private func row(_ scope: Scope, _ title: String, _ symbol: String, _ count: UInt32) -> some View {
        Label { counted(title, count) } icon: { Image(systemName: symbol) }.tag(scope)
    }

    private func counted(_ title: String, _ count: UInt32) -> some View {
        HStack {
            Text(title).lineLimit(1)
            Spacer()
            if count > 0 { Text("\(count)").foregroundStyle(.secondary).monospacedDigit() }
        }
    }

    /// R10: a symbol of the set or an emoji; a filter takes the same.
    @ViewBuilder private func icon(_ value: String, _ fallback: String) -> some View {
        if value.isEmpty { Image(systemName: fallback) } else { Text(value) }
    }

    /// R11: notes and folders dropped on a folder, or on "No folder".
    private func drop(_ items: [String], into folder: String?) -> Bool {
        var moved = false
        let notes = items.compactMap { $0.hasPrefix("note:") ? String($0.dropFirst(5)) : nil }
        if !notes.isEmpty { model.moveNotes(notes, to: folder); moved = true }
        for item in items where item.hasPrefix("folder:") {
            model.moveFolder(String(item.dropFirst(7)), into: folder)
            moved = true
        }
        return moved
    }
}

/// A folder of the tree with its subfolders (R10, R12).
private struct FolderRow: View {
    @Environment(AppModel.self) private var model
    let folder: Folder
    @Binding var naming: FolderNaming?
    @Binding var deleting: Folder?
    let drop: ([String], String?) -> Bool

    var body: some View {
        let children = model.children(of: folder.id)
        if children.isEmpty {
            label
        } else {
            DisclosureGroup(isExpanded: Binding(
                get: { !model.foldedFolders.contains(folder.id) },
                set: { open in
                    if open { model.foldedFolders.remove(folder.id) } else { model.foldedFolders.insert(folder.id) }
                }
            )) {
                ForEach(children, id: \.id) { child in
                    FolderRow(folder: child, naming: $naming, deleting: $deleting, drop: drop)
                }
            } label: {
                label
            }
        }
    }

    private var label: some View {
        Label {
            HStack {
                Text(folder.name).lineLimit(1)
                Spacer()
                if folder.noteCount > 0 { Text("\(folder.noteCount)").foregroundStyle(.secondary).monospacedDigit() }
            }
        } icon: {
            if folder.icon.isEmpty {
                Image(systemName: folder.daily ? "calendar" : "folder")
            } else if folder.icon.unicodeScalars.allSatisfy(\.isASCII) {
                Image(systemName: folder.icon)
            } else {
                Text(folder.icon)
            }
        }
        .tag(Scope.folder(id: folder.id))
        .draggable(Dragged.folder(folder.id))
        .dropDestination(for: String.self) { items, _ in drop(items, folder.id) }
        .contextMenu {
            Button(L("New Subfolder…")) { naming = FolderNaming(parent: folder.id) }
            Button(L("Rename…")) { naming = FolderNaming(parent: folder.parentId, renaming: folder) }
            Button(folder.daily ? L("Turn Off Daily Mode") : L("Daily Mode")) { model.setDaily(folder.id, !folder.daily) }
            Divider()
            Button(L("Delete…")) { deleting = folder }
        }
    }
}

struct FolderNaming: Identifiable {
    let id = UUID()
    let parent: String?
    var renaming: Folder?
}

/// R10, R15: the name of a new folder, or the name and the icon of one that exists.
private struct FolderNameSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let naming: FolderNaming
    @State private var name = ""
    @State private var icon = ""

    var body: some View {
        Form {
            TextField(L("Name"), text: $name)
            if naming.renaming != nil { TextField(L("Icon (an emoji or a symbol name)"), text: $icon) }
        }
        .padding()
        .frame(width: 320)
        .onAppear {
            name = naming.renaming?.name ?? ""
            icon = naming.renaming?.icon ?? ""
        }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button(L("Cancel")) { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                Button(L("Save")) {
                    if let folder = naming.renaming {
                        if name != folder.name { model.renameFolder(folder.id, to: name) }
                        if icon != folder.icon { model.setFolderIcon(folder.id, icon) }
                    } else {
                        model.createFolder(named: name, in: naming.parent)
                    }
                    dismiss()
                }
                .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
    }
}

struct TagRenaming: Identifiable {
    let id = UUID()
    let old: String
    var new: String
}

/// R22: renaming into an existing tag merges the two.
private struct TagRenameSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var renaming: TagRenaming

    var body: some View {
        Form { TextField(L("Name"), text: $renaming.new) }
            .padding()
            .frame(width: 320)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(L("Cancel")) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(L("Rename")) {
                        model.renameTag(renaming.old, to: renaming.new)
                        dismiss()
                    }
                }
            }
    }
}
