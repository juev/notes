import SwiftUI

/// A saved filter while it is edited (R45).
struct FilterDraft: Identifiable {
    let id = UUID()
    var filter: String?
    var name = ""
    var icon = ""
    var join: FilterJoin = .all
    var conditions: [Condition] = []

    init() {}

    init(_ filter: SavedFilter) {
        self.filter = filter.id
        name = filter.name
        icon = filter.icon
        join = filter.query.join
        conditions = filter.query.conditions
    }
}

/// The kinds of condition of R45, each with what the sheet asks for.
private enum ConditionKind: String, CaseIterable, Identifiable {
    case inFolder, hasTag, noTags, noneOfTags, pinned, starred, archived, hasAttachments, titleIs, titleContains, textContains, createdWithin, editedWithin

    var id: String { rawValue }

    var title: String {
        switch self {
        case .inFolder: L("In the folder")
        case .hasTag: L("Has the tag")
        case .noTags: L("Has no tags")
        case .noneOfTags: L("Has none of the tags")
        case .pinned: L("Is pinned")
        case .starred: L("Is starred")
        case .archived: L("Is archived")
        case .hasAttachments: L("Has attachments")
        case .titleIs: L("The title is")
        case .titleContains: L("The title contains")
        case .textContains: L("The text contains")
        case .createdWithin: L("Created within the last days")
        case .editedWithin: L("Edited within the last days")
        }
    }

    init(_ condition: Condition) {
        switch condition {
        case .inFolder: self = .inFolder
        case .hasTag: self = .hasTag
        case .noTags: self = .noTags
        case .noneOfTags: self = .noneOfTags
        case .pinned: self = .pinned
        case .starred: self = .starred
        case .archived: self = .archived
        case .hasAttachments: self = .hasAttachments
        case .titleIs: self = .titleIs
        case .titleContains: self = .titleContains
        case .textContains: self = .textContains
        case .created: self = .createdWithin
        case .edited: self = .editedWithin
        }
    }

    /// The condition of this kind with `value` as its text, or a default one.
    func condition(_ value: String, folders: [Folder]) -> Condition {
        switch self {
        case .inFolder: .inFolder(folderId: folders.first { $0.id == value }?.id ?? folders.first?.id ?? "", subfolders: true)
        case .hasTag: .hasTag(name: value)
        case .noTags: .noTags
        case .noneOfTags: .noneOfTags(names: value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) })
        case .pinned: .pinned
        case .starred: .starred
        case .archived: .archived
        case .hasAttachments: .hasAttachments
        case .titleIs: .titleIs(text: value)
        case .titleContains: .titleContains(text: value)
        case .textContains: .textContains(text: value)
        case .createdWithin: .created(when: .within(days: UInt32(value) ?? 7))
        case .editedWithin: .edited(when: .within(days: UInt32(value) ?? 7))
        }
    }

    static func value(of condition: Condition) -> String {
        switch condition {
        case .inFolder(let id, _): id
        case .hasTag(let name): name
        case .noneOfTags(let names): names.joined(separator: ", ")
        case .titleIs(let text), .titleContains(let text), .textContains(let text): text
        case .created(.within(let days)), .edited(.within(let days)): "\(days)"
        default: ""
        }
    }

    var takesText: Bool {
        switch self {
        case .hasTag, .noneOfTags, .titleIs, .titleContains, .textContains, .createdWithin, .editedWithin: true
        default: false
        }
    }
}

/// R45: the name, the icon and the conditions of a saved filter.
struct FilterEditor: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var draft: FilterDraft

    var body: some View {
        Form {
            TextField(L("Name"), text: $draft.name)
            TextField(L("Icon (an emoji)"), text: $draft.icon)
            Picker(L("Notes that meet"), selection: $draft.join) {
                Text(L("All of the conditions")).tag(FilterJoin.all)
                Text(L("Any of the conditions")).tag(FilterJoin.any)
            }
            Section {
                ForEach(draft.conditions.indices, id: \.self) { index in
                    row(index)
                }
                Button(L("Add Condition")) { draft.conditions.append(.titleContains(text: "")) }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520, height: 420)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button(L("Cancel")) { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                Button(L("Save")) {
                    model.saveFilter(id: draft.filter, name: draft.name, icon: draft.icon, query: FilterQuery(join: draft.join, conditions: draft.conditions))
                    dismiss()
                }
                .disabled(draft.name.trimmingCharacters(in: .whitespaces).isEmpty || draft.conditions.isEmpty)
            }
        }
    }

    private func row(_ index: Int) -> some View {
        let kind = ConditionKind(draft.conditions[index])
        return HStack {
            Picker("", selection: Binding(
                get: { kind },
                set: { draft.conditions[index] = $0.condition("", folders: model.folders) }
            )) {
                ForEach(ConditionKind.allCases) { Text($0.title).tag($0) }
            }
            .labelsHidden()
            if kind == .inFolder {
                Picker("", selection: Binding(
                    get: { ConditionKind.value(of: draft.conditions[index]) },
                    set: { draft.conditions[index] = kind.condition($0, folders: model.folders) }
                )) {
                    ForEach(model.folders, id: \.id) { folder in
                        Text(model.path(of: folder.id).map(\.name).joined(separator: " / ")).tag(folder.id)
                    }
                }
                .labelsHidden()
            } else if kind.takesText {
                TextField("", text: Binding(
                    get: { ConditionKind.value(of: draft.conditions[index]) },
                    set: { draft.conditions[index] = kind.condition($0, folders: model.folders) }
                ))
            }
            Button(L("Remove"), systemImage: "minus.circle") { draft.conditions.remove(at: index) }
                .labelStyle(.iconOnly)
                .buttonStyle(.plain)
        }
    }
}
