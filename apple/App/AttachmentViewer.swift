import AppKit
import PDFKit

/// R31: an image opens full-screen, a PDF as a whole inside the app, with the way to the system app.
@MainActor
enum AttachmentViewer {
    private static var windows: [NSWindow] = []

    static func show(_ file: Attachment) {
        guard let path = file.localPath else { return }
        let content: NSView
        if file.mime == "application/pdf" {
            let view = PDFView()
            view.document = PDFDocument(url: URL(fileURLWithPath: path))
            view.autoScales = true
            content = view
        } else {
            let view = NSImageView(image: NSImage(contentsOfFile: path) ?? NSImage())
            view.imageScaling = .scaleProportionallyUpOrDown
            content = view
        }
        let size = NSScreen.main?.visibleFrame.size ?? NSSize(width: 900, height: 700)
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: NSSize(width: size.width * 0.8, height: size.height * 0.85)),
                              styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = file.name
        window.isReleasedWhenClosed = false
        window.collectionBehavior = [.fullScreenPrimary]
        let open = NSButton(title: L("Open in Default App"), target: Opener.shared, action: #selector(Opener.open(_:)))
        open.identifier = NSUserInterfaceItemIdentifier(file.id)
        Opener.shared.files[file.id] = file
        let accessory = NSTitlebarAccessoryViewController()
        accessory.view = open
        accessory.layoutAttribute = .trailing
        window.addTitlebarAccessoryViewController(accessory)
        window.contentView = content
        window.center()
        window.makeKeyAndOrderFront(nil)
        if file.mime != "application/pdf" { window.toggleFullScreen(nil) }
        windows.removeAll { !$0.isVisible }
        windows.append(window)
    }

    #if DEBUG
    static var openCount: Int { windows.count }
    #endif
}

/// The target of the button of a viewer window.
@MainActor
final class Opener: NSObject {
    static let shared = Opener()
    var files: [String: Attachment] = [:]

    @objc func open(_ sender: NSButton) {
        guard let id = sender.identifier?.rawValue, let file = files[id], let url = AttachmentFiles.named(file) else { return }
        NSWorkspace.shared.open(url)
    }
}
