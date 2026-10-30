import SwiftUI

/// The main window: the sidebar, the list of notes and the editor (docs/specs/product.md, "Interface").
struct MainWindow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.undoManager) private var undoManager

    var body: some View {
        NavigationSplitView {
            List {}
                .navigationSplitViewColumnWidth(min: 180, ideal: 220)
        } content: {
            Text(model.startupError ?? "")
                .foregroundStyle(.secondary)
                .navigationSplitViewColumnWidth(min: 220, ideal: 300)
        } detail: {
            Text("")
        }
        .onAppear { model.undoManager = undoManager }
        .onChange(of: undoManager) { _, manager in model.undoManager = manager }
    }
}
