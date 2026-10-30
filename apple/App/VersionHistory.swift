import SwiftUI

/// R40–R43: the versions of the text of the open note: the list, a version with what differs from
/// the current text marked, and the way back to it.
struct VersionHistory: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let noteId: String
    @State private var chosen: String?

    var body: some View {
        let versions = model.versions(of: noteId)
        NavigationSplitView {
            List(versions, id: \.id, selection: $chosen) { version in
                VStack(alignment: .leading, spacing: 2) {
                    Text(Date(timeIntervalSince1970: Double(version.timeMs) / 1000).formatted(date: .abbreviated, time: .shortened))
                    HStack(spacing: 6) {
                        Text(version.device.isEmpty ? (version.thisDevice ? L("This device") : L("Another device")) : version.device)
                        Text("+\(version.added)").foregroundStyle(.green)
                        Text("−\(version.removed)").foregroundStyle(.red)
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
                .tag(version.id)
            }
            .overlay {
                if versions.isEmpty { Text(L("No versions yet")).foregroundStyle(.secondary) }
            }
            .navigationSplitViewColumnWidth(220)
        } detail: {
            if let chosen, let diff = model.versionDiff(noteId, chosen) {
                VStack(alignment: .leading, spacing: 0) {
                    ScrollView {
                        Text(Self.marked(diff))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding()
                    }
                    Divider()
                    HStack {
                        Spacer()
                        Button(L("Restore This Version")) {
                            model.restoreVersion(noteId, chosen)
                            dismiss()
                        }
                        .disabled(model.openNote?.locked ?? false)
                    }
                    .padding(8)
                }
            } else {
                Text(L("Choose a version")).foregroundStyle(.secondary)
            }
        }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button(L("Close")) { dismiss() } }
        }
        .frame(width: 720, height: 480)
        .onAppear { chosen = chosen ?? versions.first?.id }
    }

    /// R41: read-only, with what the current text does not have on a coloured ground. A place where
    /// the current text has more is marked by the character before it.
    static func marked(_ diff: VersionDiff) -> AttributedString {
        var out = AttributedString(diff.text)
        let utf16 = diff.text.utf16
        func index(_ offset: UInt32) -> AttributedString.Index? {
            utf16.index(utf16.startIndex, offsetBy: Int(offset), limitedBy: utf16.endIndex).flatMap { AttributedString.Index($0, within: out) }
        }
        for range in diff.changed {
            if let start = index(range.start), let end = index(range.end) { out[start..<end].backgroundColor = .green.opacity(0.3) }
        }
        for at in diff.removedAt where at > 0 {
            if let start = index(at - 1), let end = index(at) { out[start..<end].underlineStyle = .single; out[start..<end].underlineColor = .red }
        }
        return out
    }
}
