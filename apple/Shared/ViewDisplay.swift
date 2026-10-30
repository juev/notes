import Foundation

/// R16: how a view sorts and shows its notes.
struct ViewDisplay: Equatable, Codable {
    var sortBy: String = "edited"
    var descending = true
    var showPreview = true
    var showDate = true

    init() {}

    init(sort: FolderSort, showPreview: Bool, showDate: Bool) {
        sortBy = sort.by == .created ? "created" : sort.by == .title ? "title" : "edited"
        descending = sort.descending
        self.showPreview = showPreview
        self.showDate = showDate
    }

    var sort: FolderSort {
        FolderSort(by: sortBy == "created" ? .created : sortBy == "title" ? .title : .edited, descending: descending)
    }

    /// The key a system view keeps its display under on this device.
    static func key(_ scope: Scope) -> String {
        switch scope {
        case .all: "all"
        case .starred: "starred"
        case .archived: "archived"
        case .trash: "trash"
        case .untagged: "untagged"
        case .noFolder: "noFolder"
        case .tag: "tag"
        case .filter(let id): "filter:\(id)"
        case .folder(let id): "folder:\(id)"
        }
    }

    static func stored() -> [String: ViewDisplay] {
        guard let data = Storage.defaults.data(forKey: "viewDisplay") else { return [:] }
        return (try? JSONDecoder().decode([String: ViewDisplay].self, from: data)) ?? [:]
    }

    static func store(_ all: [String: ViewDisplay]) {
        Storage.defaults.set(try? JSONEncoder().encode(all), forKey: "viewDisplay")
    }

    /// R16, R25: the pinned first, then by the sort of the view.
    static func sorted(_ notes: [Note], by sort: FolderSort) -> [Note] {
        notes.sorted { a, b in
            if a.pinned != b.pinned { return a.pinned }
            let order: ComparisonResult = switch sort.by {
            case .edited: a.editedMs == b.editedMs ? .orderedSame : (a.editedMs < b.editedMs ? .orderedAscending : .orderedDescending)
            case .created: a.createdMs == b.createdMs ? .orderedSame : (a.createdMs < b.createdMs ? .orderedAscending : .orderedDescending)
            case .title: a.shownTitle.localizedStandardCompare(b.shownTitle)
            }
            if order == .orderedSame { return a.id < b.id }
            return sort.descending ? order == .orderedDescending : order == .orderedAscending
        }
    }
}

extension Note {
    /// R3: the title a note shows: its title, its first line, or "Untitled".
    var shownTitle: String {
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !title.isEmpty { return title }
        let first = preview.trimmingCharacters(in: .whitespacesAndNewlines)
        return first.isEmpty ? L("Untitled") : String(first.prefix(80))
    }
}

/// The flags of a note (R25–R28).
enum NoteFlag: CaseIterable {
    case pinned, starred, archived, locked

    var actionName: String {
        switch self {
        case .pinned: L("Pin")
        case .starred: L("Star")
        case .archived: L("Archive")
        case .locked: L("Lock")
        }
    }

    func of(_ note: Note) -> Bool {
        switch self {
        case .pinned: note.pinned
        case .starred: note.starred
        case .archived: note.archived
        case .locked: note.locked
        }
    }
}
