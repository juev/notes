import AppKit
import SwiftUI
import WebKit

/// R65: what to import and how; the backup comes from the panel of the menu File → Import….
struct ImportSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let backup: URL
    @State private var filesDir: URL?
    @State private var underOneFolder = false
    @State private var collectUnlinked = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(L("Import from Standard Notes")).font(.headline)
            Text(backup.lastPathComponent).foregroundStyle(.secondary)
            Toggle(L("Put everything under a folder “Standard Notes”"), isOn: $underOneFolder)
            HStack {
                Button(L("Folder of Files…")) { filesDir = Self.pickFolder() }
                Text(filesDir?.lastPathComponent ?? L("Optional: files downloaded from Standard Notes")).foregroundStyle(.secondary)
            }
            Toggle(L("Collect the files of no note into one note"), isOn: $collectUnlinked)
                .disabled(filesDir == nil)
            HStack {
                Spacer()
                Button(L("Cancel"), role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                Button(L("Import")) {
                    model.importBackup(backup, filesDir: filesDir, options: ImportOptions(underOneFolder: underOneFolder, collectUnlinkedFiles: collectUnlinked))
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    static func pickFolder() -> URL? {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        return panel.runModal() == .OK ? panel.url : nil
    }
}

/// I12: the report of an import, which can be copied as text.
struct ImportReportSheet: View {
    @Environment(\.dismiss) private var dismiss
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(L("Import finished")).font(.headline)
            ScrollView {
                Text(text).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Button(L("Copy")) {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                }
                Spacer()
                Button(L("Close")) { dismiss() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 520, height: 400)
    }
}

/// R61: PDF is the HTML of the core printed by the system, so it breaks as the editor does.
@MainActor
final class PDFExport: NSObject, WKNavigationDelegate {
    private static var running: [PDFExport] = []
    private let view = WKWebView(frame: NSRect(x: 0, y: 0, width: 595, height: 842))
    private let target: URL
    private let paper: NSSize
    private let done: (Error?) -> Void
    private let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 595, height: 842), styleMask: [.borderless], backing: .buffered, defer: false)

    private init(target: URL, paper: NSSize, done: @escaping (Error?) -> Void) {
        self.target = target
        self.paper = paper
        self.done = done
    }

    /// A4 or Letter, in points.
    static let a4 = NSSize(width: 595, height: 842)
    static let letter = NSSize(width: 612, height: 792)

    static func run(html: URL, to target: URL, paper: NSSize, done: @escaping (Error?) -> Void) {
        let export = PDFExport(target: target, paper: paper, done: done)
        running.append(export)
        export.view.navigationDelegate = export
        export.window.contentView = export.view
        export.view.loadFileURL(html, allowingReadAccessTo: html.deletingLastPathComponent())
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        let info = NSPrintInfo()
        info.paperSize = paper
        info.jobDisposition = .save
        info.dictionary()[NSPrintInfo.AttributeKey.jobSavingURL] = target
        info.horizontalPagination = .fit
        info.verticalPagination = .automatic
        let operation = webView.printOperation(with: info)
        operation.showsPrintPanel = false
        operation.showsProgressPanel = false
        operation.view?.frame = NSRect(origin: .zero, size: paper)
        operation.runModal(for: window, delegate: self, didRun: #selector(printed(_:success:context:)), contextInfo: nil)
    }

    /// Called on a thread of the print operation.
    @objc nonisolated private func printed(_ operation: NSPrintOperation, success: Bool, context: UnsafeMutableRawPointer?) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.done(success ? nil : CocoaError(.fileWriteUnknown))
                self.window.orderOut(nil)
                Self.running.removeAll { $0 === self }
            }
        }
    }
}
