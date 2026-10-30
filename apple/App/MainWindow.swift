import SwiftUI

/// The main window: the sidebar, the list of notes and the editor (docs/specs/product.md, "Interface").
struct MainWindow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.undoManager) private var undoManager

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 180, ideal: 230)
        } content: {
            Group {
                if model.isSearching { SearchList() } else if model.filesOpen { FileList() } else { NoteList() }
            }
            .navigationSplitViewColumnWidth(min: 240, ideal: 300)
        } detail: {
            Editor()
        }
        .navigationTitle(title)
        .searchable(text: $model.search, placement: .toolbar, prompt: L("Search"))
        .alert(L("This note is in the Trash"), isPresented: Binding(get: { model.restoreOffer != nil }, set: { if !$0 { model.restoreOffer = nil } })) {
            Button(L("Restore")) {
                if let id = model.restoreOffer {
                    model.restore([id])
                    model.open(id)
                }
            }
            Button(L("Cancel"), role: .cancel) {}
        }
        .onAppear { model.undoManager = undoManager }
        .onChange(of: undoManager) { _, manager in model.undoManager = manager }
        .alert(model.alert ?? "", isPresented: Binding(get: { model.alert != nil }, set: { if !$0 { model.alert = nil } })) {}
    }

    private var title: String {
        if model.filesOpen { return L("Files") }
        return switch model.scope {
        case .all: L("All notes")
        case .starred: L("Starred")
        case .archived: L("Archived")
        case .trash: L("Trash")
        case .untagged: L("Untagged")
        case .noFolder: L("No folder")
        case .folder: model.folderOfScope?.name ?? "Notes"
        case .tag(let name): "#\(name)"
        case .filter(let id): model.filters.first { $0.id == id }?.name ?? "Notes"
        }
    }
}
