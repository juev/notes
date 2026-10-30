import AppKit
import SwiftUI

/// The third column: the title, the tags and the text of the open note (R1, R20, R28).
struct Editor: View {
    @Environment(AppModel.self) private var model
    @FocusState private var titleFocused: Bool
    @State private var title = ""
    @State private var newTag = ""

    var body: some View {
        if let note = model.openNote {
            VStack(alignment: .leading, spacing: 0) {
                if note.locked { lockBar(note) }
                TextField(L("Title"), text: $title)
                    .textFieldStyle(.plain)
                    .font(.title.weight(.semibold))
                    .focused($titleFocused)
                    .disabled(note.locked || note.deleted)
                    .onSubmit { model.focusText() }
                    .onChange(of: title) { _, new in
                        if new != note.title, note.id == model.selectedNote { model.setTitle(note.id, new) }
                    }
                    .padding(.horizontal, 20)
                    .padding(.top, 16)
                tagRow(note)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 6)
                if !note.locked, !note.deleted { formatBar }
                Divider()
                MarkdownEditor(noteId: note.id, editable: !note.locked && !note.deleted, revision: model.textRevision, focus: model.textFocusRequests)
            }
            .id(note.id)
            .onAppear {
                title = note.title
                if model.takeTitleFocus() { titleFocused = true }
            }
            .onChange(of: note.title) { _, new in if !titleFocused { title = new } }
        } else {
            Text(model.startupError ?? L("No note selected")).foregroundStyle(.secondary)
        }
    }

    /// E20: the formats at hand.
    private var formatBar: some View {
        HStack(spacing: 10) {
            ForEach(Self.bar, id: \.0) { symbol, title, format in
                Button(title, systemImage: symbol) { model.activeEditor?.format(format) }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.plain)
            }
            Spacer()
        }
        .foregroundStyle(.secondary)
        .padding(.horizontal, 20)
        .padding(.bottom, 6)
    }

    private static let bar: [(String, String, MarkdownFormat)] = [
        ("bold", L("Bold"), .bold), ("italic", L("Italic"), .italic), ("strikethrough", L("Strikethrough"), .strikethrough),
        ("chevron.left.forwardslash.chevron.right", L("Code"), .code), ("highlighter", L("Highlight"), .highlight), ("link", L("Link"), .link),
        ("textformat.size", L("Heading 1"), .heading(level: 1)), ("list.bullet", L("Bulleted List"), .bulletList),
        ("list.number", L("Numbered List"), .numberedList), ("checklist", L("Task List"), .taskList), ("text.quote", L("Quote"), .quote),
    ]

    /// R28: a locked note opens read-only, with the way out.
    private func lockBar(_ note: Note) -> some View {
        HStack {
            Label(L("This note is locked"), systemImage: "lock.fill")
            Spacer()
            Button(L("Unlock")) { model.setFlag(.locked, false, ids: [note.id]) }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 6)
        .background(.quaternary)
    }

    /// R20: the tags as chips, and a field that ends a tag with Return or a comma.
    private func tagRow(_ note: Note) -> some View {
        HStack(spacing: 6) {
            ForEach(note.tags, id: \.self) { tag in
                HStack(spacing: 2) {
                    Text(tag)
                    if !note.locked {
                        Button(L("Remove Tag"), systemImage: "xmark") { model.removeTag(note.id, tag) }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.plain)
                            .imageScale(.small)
                    }
                }
                .font(.callout)
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .background(.quaternary, in: Capsule())
            }
            if !note.locked, !note.deleted {
                TextField(L("Add a tag"), text: $newTag)
                    .textFieldStyle(.plain)
                    .font(.callout)
                    .frame(maxWidth: 140)
                    .onSubmit { addTag(note) }
                    .onChange(of: newTag) { _, new in
                        if new.hasSuffix(",") { newTag = String(new.dropLast()); addTag(note) }
                    }
                // R20: completion from the existing tags.
                ForEach(completions(note), id: \.self) { tag in
                    Button(tag) { newTag = tag; addTag(note) }
                        .buttonStyle(.link)
                        .font(.callout)
                }
            }
            Spacer(minLength: 0)
        }
    }

    private func completions(_ note: Note) -> [String] {
        let typed = newTag.trimmingCharacters(in: .whitespaces).lowercased()
        guard !typed.isEmpty else { return [] }
        return model.tags.map(\.name).filter { $0.lowercased().hasPrefix(typed) && !note.tags.contains($0) }.prefix(3).map { $0 }
    }

    private func addTag(_ note: Note) {
        let tag = newTag.trimmingCharacters(in: .whitespaces)
        newTag = ""
        if !tag.isEmpty { model.addTag(note.id, tag) }
    }
}

/// The text of the note in a `MarkdownTextView`: edits go to the model, and what changed in the
/// core while no edit of this view was waiting comes in as an edit from outside (S40).
struct MarkdownEditor: NSViewRepresentable {
    @Environment(AppModel.self) private var model
    let noteId: String
    let editable: Bool
    let revision: Int
    let focus: Int

    final class Coordinator {
        var focus = 0
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let view = MarkdownTextView(frame: .zero)
        view.prepare()
        view.isVerticallyResizable = true
        view.isHorizontallyResizable = false
        view.autoresizingMask = [.width]
        view.textContainer?.widthTracksTextView = true
        view.textContainerInset = NSSize(width: 16, height: 12)
        view.isAutomaticQuoteSubstitutionEnabled = false
        view.isAutomaticDashSubstitutionEnabled = false
        view.isAutomaticTextReplacementEnabled = false
        // E25: the view keeps the history of typing itself.
        view.allowsUndo = false
        view.smartTypography = model.settings.smartTypography
        model.activeEditor = view
        view.sharedUndoManager = { [weak model] in MainActor.assumeIsolated { model?.undoManager } }
        view.baseFont = .systemFont(ofSize: NSFont.systemFontSize + 2)
        let model = model
        let noteId = noteId
        view.onEdit = { edit, text in MainActor.assumeIsolated { model.textEdited(noteId, edit, whole: text) } }
        view.openLink = { url in MainActor.assumeIsolated { model.openLink(url) } }
        view.setText(model.text(of: noteId))
        let scroll = NSScrollView()
        scroll.documentView = view
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        view.drawsBackground = false
        context.coordinator.focus = focus
        return scroll
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        (scroll.documentView as? MarkdownTextView)?.forgetHistory()
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? MarkdownTextView else { return }
        view.isEditable = editable
        view.smartTypography = model.settings.smartTypography
        if context.coordinator.focus != focus {
            context.coordinator.focus = focus
            view.window?.makeFirstResponder(view)
        }
        guard !model.hasPendingEdits else { return }
        if let edit = TextEdit.between(view.string, model.text(of: noteId)) { view.apply(outside: edit) }
    }
}
