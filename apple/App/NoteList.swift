import SwiftUI

/// The list of notes of the open view (R5, R16, R17, R46).
struct NoteList: View {
    @Environment(AppModel.self) private var model
    @State private var moving: [String]?
    @State private var emptying = false

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selectedNote) {
            if model.folderOfScope?.daily == true {
                ForEach(days, id: \.title) { day in
                    Section(day.title) { ForEach(day.notes, id: \.id, content: row) }
                }
            } else {
                ForEach(model.notes, id: \.id, content: row)
            }
        }
        .overlay {
            if model.notes.isEmpty { Text(L("No notes")).foregroundStyle(.secondary) }
        }
        .onDeleteCommand {
            if let id = model.selectedNote { model.trash([id]) }
        }
        .toolbar {
            ToolbarItemGroup {
                if model.scope == .trash, !model.notes.isEmpty {
                    Button(L("Empty Trash…")) { emptying = true }
                }
                if model.folderOfScope?.daily == true {
                    Button(L("New Entry for Today"), systemImage: "calendar.badge.plus") { model.newEntryForToday() }
                }
                displayMenu
                Button(L("New Note"), systemImage: "square.and.pencil") { model.newNote() }
            }
        }
        .confirmationDialog(L("Delete all notes in the Trash for good?"), isPresented: $emptying, titleVisibility: .visible) {
            Button(L("Empty Trash"), role: .destructive) { model.emptyTrash() }
        }
        .sheet(item: Binding(get: { moving.map(MoveTarget.init) }, set: { moving = $0?.ids })) { target in
            MoveSheet(ids: target.ids)
        }
    }

    /// R17: the notes of a folder in daily mode by the day they were made, the newest first.
    private var days: [(title: String, notes: [Note])] {
        let calendar = Calendar.current
        let grouped = Dictionary(grouping: model.notes) { note in
            calendar.startOfDay(for: Date(timeIntervalSince1970: Double(note.createdMs) / 1000))
        }
        return grouped.keys.sorted(by: >).map { day in
            (day.formatted(Date.FormatStyle().day().month(.wide).year()), grouped[day] ?? [])
        }
    }

    private func row(_ note: Note) -> some View {
        NoteRow(note: note, display: model.display, trash: model.scope == .trash, daysLeft: model.daysLeft(note))
            .tag(note.id)
            .draggable(Dragged.note(note.id))
            .contextMenu { menu(note) }
    }

    @ViewBuilder private func menu(_ note: Note) -> some View {
        if note.deleted {
            Button(L("Restore")) { model.restore([note.id]) }
            Button(L("Delete for Good")) { model.purge([note.id]) }
        } else {
            Button(note.pinned ? L("Unpin") : L("Pin")) { model.setFlag(.pinned, !note.pinned, ids: [note.id]) }
            Button(note.starred ? L("Remove Star") : L("Star")) { model.setFlag(.starred, !note.starred, ids: [note.id]) }
            Button(note.archived ? L("Unarchive") : L("Archive")) { model.setFlag(.archived, !note.archived, ids: [note.id]) }
            Button(note.locked ? L("Unlock") : L("Lock")) { model.setFlag(.locked, !note.locked, ids: [note.id]) }
            Divider()
            Button(L("Move to…")) { moving = [note.id] }
            Button(L("Duplicate")) { model.duplicate(note.id) }
            Divider()
            Button(L("Delete")) { model.trash([note.id]) }
        }
    }

    /// R16: the sort and what the rows show, for the open view.
    private var displayMenu: some View {
        let display = model.display
        return Menu {
            Picker(L("Sort by"), selection: Binding(get: { display.sortBy }, set: { var new = display; new.sortBy = $0; model.setDisplay(new) })) {
                Text(L("Last Edit")).tag("edited")
                Text(L("Creation")).tag("created")
                Text(L("Title")).tag("title")
            }
            Picker(L("Order"), selection: Binding(get: { display.descending }, set: { var new = display; new.descending = $0; model.setDisplay(new) })) {
                Text(L("Descending")).tag(true)
                Text(L("Ascending")).tag(false)
            }
            Divider()
            Toggle(L("Show Preview"), isOn: Binding(get: { display.showPreview }, set: { var new = display; new.showPreview = $0; model.setDisplay(new) }))
            Toggle(L("Show Date"), isOn: Binding(get: { display.showDate }, set: { var new = display; new.showDate = $0; model.setDisplay(new) }))
        } label: {
            Label(L("Display"), systemImage: "line.3.horizontal.decrease")
        }
    }
}

private struct MoveTarget: Identifiable {
    let ids: [String]
    var id: String { ids.joined() }
}

/// One note of the list (R5).
struct NoteRow: View {
    let note: Note
    let display: ViewDisplay
    let trash: Bool
    let daysLeft: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                if note.pinned { Image(systemName: "pin.fill").font(.caption2) }
                Text(note.shownTitle).fontWeight(.medium).lineLimit(1)
                Spacer(minLength: 0)
                if note.starred { Image(systemName: "star.fill").font(.caption2).foregroundStyle(.yellow) }
                if note.locked { Image(systemName: "lock.fill").font(.caption2) }
                if note.attachments > 0 { Image(systemName: "paperclip").font(.caption2) }
            }
            if display.showPreview, !note.preview.isEmpty {
                Text(note.preview).font(.callout).foregroundStyle(.secondary).lineLimit(2)
            }
            if trash {
                Text(daysLeft == 1 ? L("1 day left") : L("%d days left", daysLeft)).font(.caption).foregroundStyle(.secondary)
            } else if display.showDate {
                Text(date.formatted(date: .abbreviated, time: .shortened)).font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
    }

    private var date: Date {
        Date(timeIntervalSince1970: Double(display.sortBy == "created" ? note.createdMs : note.editedMs) / 1000)
    }
}

/// R11: "Move to…": the tree of folders, or no folder.
private struct MoveSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let ids: [String]

    var body: some View {
        VStack(alignment: .leading) {
            List {
                Button(L("No folder")) { move(nil) }.buttonStyle(.plain)
                ForEach(model.folders, id: \.id) { folder in
                    Button(model.path(of: folder.id).map(\.name).joined(separator: " / ")) { move(folder.id) }
                        .buttonStyle(.plain)
                }
            }
            .frame(minHeight: 240)
            HStack {
                Spacer()
                Button(L("Cancel")) { dismiss() }
            }
            .padding(8)
        }
        .frame(width: 320)
    }

    private func move(_ folder: String?) {
        model.moveNotes(ids, to: folder)
        dismiss()
    }
}
