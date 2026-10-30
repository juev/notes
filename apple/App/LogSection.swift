import AppKit
import SwiftUI

/// R95: the level of the log and the way to its files.
struct LogSection: View {
    @Environment(AppModel.self) private var model
    @State private var level = LogLevel.error
    @State private var files: [String] = []

    var body: some View {
        Section(L("Log")) {
            Picker(L("Level"), selection: Binding(get: { level }, set: { chosen in
                level = chosen
                try? model.store?.setLogLevel(level: chosen)
                refresh()
            })) {
                Text(L("None")).tag(LogLevel.off)
                Text(L("Errors")).tag(LogLevel.error)
                Text(L("Events")).tag(LogLevel.info)
                Text(L("Detailed")).tag(LogLevel.debug)
            }
            HStack {
                Button(L("Show in Finder")) {
                    NSWorkspace.shared.activateFileViewerSelecting(files.map { URL(fileURLWithPath: $0) })
                }
                Button(L("Clear")) {
                    try? model.store?.clearLog()
                    refresh()
                }
                Spacer()
            }
            .disabled(files.isEmpty)
            Text(L("The log is a text file on this Mac. It holds what the app did and the text of errors, not what the notes say."))
                .font(.caption).foregroundStyle(.secondary)
        }
        .onAppear {
            level = model.store?.logLevel() ?? .error
            refresh()
        }
    }

    private func refresh() {
        files = model.store?.logFiles() ?? []
    }
}
